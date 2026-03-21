use std::{
    env,
    io::{Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Duration,
};

use crossbeam_channel::Sender;
use serde::Deserialize;
use tungstenite::{Message, connect};
use url::Url;

use crate::logging;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const MEDIA_PACKET_MAGIC: &[u8; 4] = b"BKWM";
const MEDIA_PACKET_HEADER_LEN: usize = 8;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaCodec {
    Jpeg,
    H264,
}

#[derive(Clone)]
pub enum MediaEvent {
    Connected { session_id: String },
    Disconnected { session_id: String },
    Frame {
        session_id: String,
        codec: MediaCodec,
        bytes: Vec<u8>,
        width: Option<u32>,
        height: Option<u32>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MediaPacketKind {
    Config,
    Frame,
}

struct H264DecoderSession {
    child: Child,
    stdin: ChildStdin,
    flavor: H264DecoderFlavor,
    exit_rx: Receiver<()>,
}

#[derive(Clone, Copy, Deserialize)]
struct H264Config {
    width: u32,
    height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum H264DecoderFlavor {
    #[cfg(windows)]
    D3d11va,
    #[cfg(windows)]
    Dxva2,
    #[cfg(windows)]
    Qsv,
    #[cfg(target_os = "macos")]
    VideoToolbox,
    Software,
}

impl H264DecoderFlavor {
    fn label(self) -> &'static str {
        match self {
            #[cfg(windows)]
            Self::D3d11va => "d3d11va",
            #[cfg(windows)]
            Self::Dxva2 => "dxva2",
            #[cfg(windows)]
            Self::Qsv => "qsv",
            #[cfg(target_os = "macos")]
            Self::VideoToolbox => "videotoolbox",
            Self::Software => "software",
        }
    }

    fn append_ffmpeg_args(self, command: &mut Command) {
        match self {
            #[cfg(windows)]
            Self::D3d11va => {
                command
                    .arg("-hwaccel")
                    .arg("d3d11va")
                    .arg("-hwaccel_output_format")
                    .arg("d3d11");
            }
            #[cfg(windows)]
            Self::Dxva2 => {
                command
                    .arg("-hwaccel")
                    .arg("dxva2")
                    .arg("-hwaccel_output_format")
                    .arg("dxva2_vld");
            }
            #[cfg(windows)]
            Self::Qsv => {
                command
                    .arg("-hwaccel")
                    .arg("qsv")
                    .arg("-c:v")
                    .arg("h264_qsv");
            }
            #[cfg(target_os = "macos")]
            Self::VideoToolbox => {
                command.arg("-hwaccel").arg("videotoolbox");
            }
            Self::Software => {}
        }
    }
}

fn h264_decoder_candidates() -> Vec<H264DecoderFlavor> {
    let mut candidates = Vec::new();
    #[cfg(windows)]
    {
        candidates.push(H264DecoderFlavor::D3d11va);
        candidates.push(H264DecoderFlavor::Dxva2);
        candidates.push(H264DecoderFlavor::Qsv);
        candidates.push(H264DecoderFlavor::Software);
    }
    #[cfg(target_os = "macos")]
    {
        candidates.push(H264DecoderFlavor::Software);
        candidates.push(H264DecoderFlavor::VideoToolbox);
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    candidates.push(H264DecoderFlavor::Software);
    candidates
}

impl H264DecoderSession {
    fn new(
        width: u32,
        height: u32,
        session_id: String,
        event_tx: Sender<MediaEvent>,
        flavor: H264DecoderFlavor,
    ) -> Result<Self, String> {
        let ffmpeg = ffmpeg_executable_path();
        logging::append_log(
            "INFO",
            "media.h264_decoder",
            format!(
                "starting ffmpeg={} decoder={} width={} height={} session_id={}",
                ffmpeg.display(),
                flavor.label(),
                width,
                height,
                session_id
            ),
        );
        let mut command = Command::new(ffmpeg);
        command
            .arg("-loglevel")
            .arg("error")
            .arg("-fflags")
            .arg("nobuffer")
            .arg("-flags")
            .arg("low_delay");
        flavor.append_ffmpeg_args(&mut command);
        command
            .arg("-f")
            .arg("h264")
            .arg("-i")
            .arg("pipe:0")
            .arg("-f")
            .arg("rawvideo")
            .arg("-pix_fmt")
            .arg("rgba")
            .arg("pipe:1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_hidden_process(&mut command);
        let mut child = command.spawn().map_err(|error| error.to_string())?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ffmpeg decoder stdin is not available".to_owned())?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| "ffmpeg decoder stdout is not available".to_owned())?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| "ffmpeg decoder stderr is not available".to_owned())?;
        let (exit_tx, exit_rx) = mpsc::channel();
        let (stderr_tx, stderr_rx) = mpsc::channel();

        thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = stderr.read_to_end(&mut buffer);
            let stderr_text = String::from_utf8_lossy(&buffer).trim().to_owned();
            let _ = stderr_tx.send(stderr_text);
        });

        thread::spawn(move || {
            let frame_len = (width as usize)
                .saturating_mul(height as usize)
                .saturating_mul(4);
            let mut frame = vec![0_u8; frame_len];
            let mut frame_count = 0_u64;

            loop {
                if stdout.read_exact(&mut frame).is_err() {
                    let stderr_text = stderr_rx.try_recv().unwrap_or_default();
                    logging::append_log(
                        "WARN",
                        "media.h264_decoder",
                        if stderr_text.is_empty() {
                            "decoder stdout ended or frame read failed".to_owned()
                        } else {
                            format!(
                                "decoder stdout ended or frame read failed: {}",
                                stderr_text
                            )
                        },
                    );
                    break;
                }
                frame_count = frame_count.saturating_add(1);
                if frame_count == 1 || frame_count % 120 == 0 {
                    logging::append_log(
                        "INFO",
                        "media.h264_decoder",
                        format!("decoded_frames={}", frame_count),
                    );
                }
                let _ = event_tx.send(MediaEvent::Frame {
                    session_id: session_id.clone(),
                    codec: MediaCodec::H264,
                    bytes: frame.clone(),
                    width: Some(width),
                    height: Some(height),
                });
            }
            let _ = exit_tx.send(());
        });

        Ok(Self {
            child,
            stdin,
            flavor,
            exit_rx,
        })
    }

    fn push_packet(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.exit_rx.try_recv().is_ok() {
            return Err(format!("decoder {} exited", self.flavor.label()));
        }
        self.stdin.write_all(bytes).map_err(|error| error.to_string())
    }

    fn flavor(&self) -> H264DecoderFlavor {
        self.flavor
    }
}

impl Drop for H264DecoderSession {
    fn drop(&mut self) {
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn spawn_listener(
    server_url: String,
    token: String,
    session_id: String,
    stop_flag: Arc<AtomicBool>,
    event_tx: Sender<MediaEvent>,
) {
    thread::spawn(move || {
        let Ok(url) = media_url(&server_url, &token, &session_id) else {
            let _ = event_tx.send(MediaEvent::Disconnected {
                session_id: session_id.clone(),
            });
            return;
        };

        while !stop_flag.load(Ordering::Relaxed) {
            match connect(url.as_str()) {
                Ok((mut socket, _)) => {
                    let mut h264_decoder: Option<H264DecoderSession> = None;
                    let mut h264_config: Option<H264Config> = None;
                    let mut h264_disabled_flavors: Vec<H264DecoderFlavor> = Vec::new();
                    let mut h264_packet_count = 0_u64;
                    logging::append_log(
                        "INFO",
                        "media",
                        format!("connected session_id={}", session_id),
                    );
                    let _ = event_tx.send(MediaEvent::Connected {
                        session_id: session_id.clone(),
                    });

                    while !stop_flag.load(Ordering::Relaxed) {
                        match socket.read() {
                            Ok(Message::Binary(bytes)) => {
                                if let Some((codec, kind, payload)) =
                                    decode_media_packet(bytes.as_ref())
                                {
                                    match (codec, kind) {
                                        (MediaCodec::Jpeg, MediaPacketKind::Frame) => {
                                            logging::append_log(
                                                "DEBUG",
                                                "media.jpeg",
                                                format!("jpeg frame session_id={}", session_id),
                                            );
                                            let _ = event_tx.send(MediaEvent::Frame {
                                                session_id: session_id.clone(),
                                                codec,
                                                bytes: payload,
                                                width: None,
                                                height: None,
                                            });
                                        }
                                        (MediaCodec::H264, MediaPacketKind::Config) => {
                                            if let Ok(config) =
                                                serde_json::from_slice::<H264Config>(&payload)
                                            {
                                                h264_config = Some(config);
                                                logging::append_log(
                                                    "INFO",
                                                    "media.h264_decoder",
                                                    format!(
                                                        "config received session_id={} width={} height={}",
                                                        session_id, config.width, config.height
                                                    ),
                                                );
                                                h264_decoder = ensure_h264_decoder(
                                                    &h264_disabled_flavors,
                                                    config.width,
                                                    config.height,
                                                    session_id.clone(),
                                                    event_tx.clone(),
                                                )
                                                .ok();
                                            }
                                        }
                                        (MediaCodec::H264, MediaPacketKind::Frame) => {
                                            h264_packet_count = h264_packet_count.saturating_add(1);
                                            if h264_packet_count == 1 || h264_packet_count % 120 == 0
                                            {
                                                logging::append_log(
                                                    "INFO",
                                                    "media.h264_decoder",
                                                    format!(
                                                        "packet received session_id={} bytes={} count={}",
                                                        session_id,
                                                        payload.len(),
                                                        h264_packet_count
                                                    ),
                                                );
                                            }
                                            if h264_decoder.is_none()
                                                && let Some(config) = h264_config
                                            {
                                                h264_decoder = ensure_h264_decoder(
                                                    &h264_disabled_flavors,
                                                    config.width,
                                                    config.height,
                                                    session_id.clone(),
                                                    event_tx.clone(),
                                                )
                                                .ok();
                                            }
                                            if let Some(decoder) = &mut h264_decoder {
                                                if let Err(error) = decoder.push_packet(&payload) {
                                                    logging::append_log(
                                                        "ERROR",
                                                        "media.h264_decoder",
                                                        format!(
                                                            "packet write failed on {}: {}",
                                                            decoder.flavor().label(),
                                                            error
                                                        ),
                                                    );
                                                    h264_disabled_flavors.push(decoder.flavor());
                                                    h264_decoder = None;
                                                }
                                            } else if h264_config.is_some() {
                                                logging::append_log(
                                                    "WARN",
                                                    "media.h264_decoder",
                                                    format!(
                                                        "packet dropped session_id={} bytes={} because decoder is unavailable",
                                                        session_id,
                                                        payload.len()
                                                    ),
                                                );
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            Ok(Message::Ping(payload)) => {
                                let _ = socket.send(Message::Pong(payload));
                            }
                            Ok(Message::Close(_)) => break,
                            Ok(_) => {}
                            Err(_) => break,
                        }
                    }

                    let _ = socket.close(None);
                }
                Err(error) => {
                    logging::append_log(
                        "WARN",
                        "media",
                        format!("connect failed session_id={} error={}", session_id, error),
                    );
                }
            }

            logging::append_log(
                "WARN",
                "media",
                format!("disconnected session_id={}", session_id),
            );
            let _ = event_tx.send(MediaEvent::Disconnected {
                session_id: session_id.clone(),
            });

            if stop_flag.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_secs(2));
        }
    });
}

fn media_url(server_url: &str, token: &str, session_id: &str) -> Result<Url, String> {
    let normalized = server_url.trim().trim_end_matches('/');
    let ws_base = if let Some(rest) = normalized.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = normalized.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if normalized.starts_with("ws://") || normalized.starts_with("wss://") {
        normalized.to_owned()
    } else {
        format!("ws://{normalized}")
    };

    Url::parse(&format!(
        "{ws_base}/ws/v1/media?token={token}&sessionId={session_id}"
    ))
    .map_err(|error| error.to_string())
}

fn decode_media_packet(bytes: &[u8]) -> Option<(MediaCodec, MediaPacketKind, Vec<u8>)> {
    if bytes.len() < MEDIA_PACKET_HEADER_LEN {
        return Some((MediaCodec::Jpeg, MediaPacketKind::Frame, bytes.to_vec()));
    }
    if &bytes[..4] != MEDIA_PACKET_MAGIC {
        return Some((MediaCodec::Jpeg, MediaPacketKind::Frame, bytes.to_vec()));
    }

    let codec = match bytes[5] {
        1 => MediaCodec::Jpeg,
        2 => MediaCodec::H264,
        _ => return None,
    };
    let kind = match bytes[6] {
        1 => MediaPacketKind::Config,
        2 => MediaPacketKind::Frame,
        _ => return None,
    };

    Some((codec, kind, bytes[MEDIA_PACKET_HEADER_LEN..].to_vec()))
}

fn ffmpeg_executable_path() -> PathBuf {
    if let Ok(current_exe) = env::current_exe()
        && let Some(parent) = current_exe.parent()
    {
        let bundled = parent.join("ffmpeg");
        if bundled.exists() {
            return bundled;
        }
        let bundled_exe = parent.join("ffmpeg.exe");
        if bundled_exe.exists() {
            return bundled_exe;
        }
    }

    #[cfg(target_os = "macos")]
    for candidate in ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg"] {
        let path = PathBuf::from(candidate);
        if path.exists() {
            return path;
        }
    }

    PathBuf::from("ffmpeg")
}

fn ensure_h264_decoder(
    disabled_flavors: &[H264DecoderFlavor],
    width: u32,
    height: u32,
    session_id: String,
    event_tx: Sender<MediaEvent>,
) -> Result<H264DecoderSession, String> {
    let mut last_error = None;
    for flavor in h264_decoder_candidates()
        .into_iter()
        .filter(|candidate| !disabled_flavors.contains(candidate))
    {
        match H264DecoderSession::new(width, height, session_id.clone(), event_tx.clone(), flavor) {
            Ok(session) => return Ok(session),
            Err(error) => {
                logging::append_log(
                    "WARN",
                    "media.h264_decoder",
                    format!("failed to start decoder {}: {}", flavor.label(), error),
                );
                last_error = Some(format!("{}: {}", flavor.label(), error));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| "no available h264 decoders after fallback attempts".to_owned()))
}

fn configure_hidden_process(_command: &mut Command) {
    #[cfg(windows)]
    {
        _command.creation_flags(CREATE_NO_WINDOW);
    }
}
