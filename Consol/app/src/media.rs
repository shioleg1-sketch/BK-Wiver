use std::{
    env,
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use crossbeam_channel::Sender;
use tungstenite::{Message, connect};
use url::Url;

use crate::logging;

const MEDIA_PACKET_MAGIC: &[u8; 4] = b"BKWM";
const MEDIA_PACKET_HEADER_LEN: usize = 8;
const IVF_HEADER_LEN: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaCodec {
    Vp8,
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

struct Vp8DecoderSession {
    child: Child,
    stdin: ChildStdin,
}

struct Vp8SampleCapture {
    bytes: Vec<u8>,
    dumped: bool,
}

impl Vp8DecoderSession {
    fn new(
        width: u32,
        height: u32,
        session_id: String,
        event_tx: Sender<MediaEvent>,
    ) -> Result<Self, String> {
        let ffmpeg = ffmpeg_executable_path();
        logging::append_log(
            "INFO",
            "media.vp8_decoder",
            format!(
                "starting decoder=ffmpeg path={} width={} height={} session_id={}",
                ffmpeg.display(),
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
            .arg("-probesize")
            .arg("32")
            .arg("-analyzeduration")
            .arg("0")
            .arg("-f")
            .arg("ivf")
            .arg("-i")
            .arg("pipe:0")
            .arg("-an")
            .arg("-sn")
            .arg("-f")
            .arg("rawvideo")
            .arg("-pix_fmt")
            .arg("rgba")
            .arg("pipe:1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ffmpeg stdin is not available".to_owned())?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| "ffmpeg stdout is not available".to_owned())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "ffmpeg stderr is not available".to_owned())?;

        let frame_size = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        let session_id_for_stdout = session_id.clone();
        thread::spawn(move || {
            let mut decoded_frame_count = 0_u64;
            let mut frame = vec![0_u8; frame_size];
            loop {
                match stdout.read_exact(&mut frame) {
                    Ok(()) => {
                        decoded_frame_count = decoded_frame_count.saturating_add(1);
                        if decoded_frame_count == 1 || decoded_frame_count % 120 == 0 {
                            logging::append_log(
                                "INFO",
                                "media.vp8_decoder",
                                format!(
                                    "decoded_frames={} session_id={}",
                                    decoded_frame_count, session_id_for_stdout
                                ),
                            );
                        }
                        let _ = event_tx.send(MediaEvent::Frame {
                            session_id: session_id_for_stdout.clone(),
                            codec: MediaCodec::Vp8,
                            bytes: frame.clone(),
                            width: Some(width),
                            height: Some(height),
                        });
                    }
                    Err(error) => {
                        logging::append_log(
                            "WARN",
                            "media.vp8_decoder",
                            format!(
                                "decoder stdout ended session_id={} decoded_frames={} error={}",
                                session_id_for_stdout, decoded_frame_count, error
                            ),
                        );
                        break;
                    }
                }
            }
        });
        let session_id_for_stderr = session_id.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(line) if !line.trim().is_empty() => {
                        logging::append_log(
                            "WARN",
                            "media.vp8_decoder",
                            format!("ffmpeg stderr session_id={} {}", session_id_for_stderr, line),
                        );
                    }
                    Ok(_) => {}
                    Err(error) => {
                        logging::append_log(
                            "WARN",
                            "media.vp8_decoder",
                            format!(
                                "stderr read failed session_id={} error={}",
                                session_id_for_stderr, error
                            ),
                        );
                        break;
                    }
                }
            }
        });

        Ok(Self { child, stdin })
    }

    fn push_bytes(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.stdin
            .write_all(bytes)
            .map_err(|error| error.to_string())
    }
}

impl Drop for Vp8DecoderSession {
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
                    let mut vp8_decoder: Option<Vp8DecoderSession> = None;
                    let mut vp8_frame_packet_count = 0_u64;
                    let mut vp8_sample_capture: Option<Vp8SampleCapture> = None;
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
                                        (MediaCodec::Vp8, MediaPacketKind::Config) => {
                                            let Some((width, height)) = parse_ivf_dimensions(&payload)
                                            else {
                                                logging::append_log(
                                                    "WARN",
                                                    "media.vp8_decoder",
                                                    format!(
                                                        "invalid ivf header session_id={} bytes={}",
                                                        session_id,
                                                        payload.len()
                                                    ),
                                                );
                                                continue;
                                            };
                                            logging::append_log(
                                                "INFO",
                                                "media.vp8_decoder",
                                                format!(
                                                    "config received session_id={} width={} height={}",
                                                    session_id, width, height
                                                ),
                                            );
                                            vp8_sample_capture = Some(Vp8SampleCapture {
                                                bytes: payload.clone(),
                                                dumped: false,
                                            });
                                            match Vp8DecoderSession::new(
                                                width,
                                                height,
                                                session_id.clone(),
                                                event_tx.clone(),
                                            ) {
                                                Ok(mut decoder) => {
                                                    if let Err(error) = decoder.push_bytes(&payload) {
                                                        logging::append_log(
                                                            "ERROR",
                                                            "media.vp8_decoder",
                                                            format!(
                                                                "failed to push ivf header: {}",
                                                                error
                                                            ),
                                                        );
                                                    } else {
                                                        vp8_decoder = Some(decoder);
                                                    }
                                                }
                                                Err(error) => {
                                                    logging::append_log(
                                                        "ERROR",
                                                        "media.vp8_decoder",
                                                        format!(
                                                            "failed to start decoder: {}",
                                                            error
                                                        ),
                                                    );
                                                }
                                            }
                                        }
                                        (MediaCodec::Vp8, MediaPacketKind::Frame) => {
                                            vp8_frame_packet_count =
                                                vp8_frame_packet_count.saturating_add(1);
                                            if vp8_frame_packet_count == 1
                                                || vp8_frame_packet_count % 120 == 0
                                            {
                                                logging::append_log(
                                                    "INFO",
                                                    "media.vp8_decoder",
                                                    format!(
                                                        "frame packets received={} session_id={} bytes={}",
                                                        vp8_frame_packet_count,
                                                        session_id,
                                                        payload.len()
                                                    ),
                                                );
                                            }
                                            if let Some(sample) = &mut vp8_sample_capture
                                                && !sample.dumped
                                            {
                                                sample.bytes.extend_from_slice(&payload);
                                                match logging::write_state_bytes(
                                                    &format!("vp8-sample-{}.ivf", session_id),
                                                    &sample.bytes,
                                                ) {
                                                    Ok(path) => {
                                                        logging::append_log(
                                                            "INFO",
                                                            "media.vp8_decoder",
                                                            format!(
                                                                "sample dumped session_id={} path={}",
                                                                session_id,
                                                                path.display()
                                                            ),
                                                        );
                                                        sample.dumped = true;
                                                    }
                                                    Err(error) => {
                                                        logging::append_log(
                                                            "WARN",
                                                            "media.vp8_decoder",
                                                            format!(
                                                                "sample dump failed session_id={} error={}",
                                                                session_id, error
                                                            ),
                                                        );
                                                    }
                                                }
                                            }
                                            if let Some(decoder) = &mut vp8_decoder {
                                                if let Err(error) = decoder.push_bytes(&payload) {
                                                    logging::append_log(
                                                        "ERROR",
                                                        "media.vp8_decoder",
                                                        format!("packet decode failed: {}", error),
                                                    );
                                                    vp8_decoder = None;
                                                }
                                            }
                                        }
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
    if bytes.len() < MEDIA_PACKET_HEADER_LEN || &bytes[..4] != MEDIA_PACKET_MAGIC {
        return None;
    }

    let codec = match bytes[5] {
        1 => MediaCodec::Vp8,
        _ => return None,
    };
    let kind = match bytes[6] {
        1 => MediaPacketKind::Config,
        2 => MediaPacketKind::Frame,
        _ => return None,
    };

    Some((codec, kind, bytes[MEDIA_PACKET_HEADER_LEN..].to_vec()))
}

fn parse_ivf_dimensions(header: &[u8]) -> Option<(u32, u32)> {
    if header.len() < IVF_HEADER_LEN || &header[..4] != b"DKIF" || &header[8..12] != b"VP80" {
        return None;
    }
    let width = u16::from_le_bytes([header[12], header[13]]) as u32;
    let height = u16::from_le_bytes([header[14], header[15]]) as u32;
    Some((width, height))
}

fn ffmpeg_executable_path() -> PathBuf {
    if let Ok(current_exe) = env::current_exe()
        && let Some(parent) = current_exe.parent()
    {
        if cfg!(windows) {
            let bundled = parent.join("ffmpeg.exe");
            if bundled.exists() {
                return bundled;
            }
        } else if cfg!(target_os = "macos") {
            let bundled_near_exe = parent.join("ffmpeg");
            if bundled_near_exe.exists() {
                return bundled_near_exe;
            }
            if let Some(contents_dir) = parent.parent() {
                let bundled_in_resources = contents_dir.join("Resources").join("ffmpeg");
                if bundled_in_resources.exists() {
                    return bundled_in_resources;
                }
            }
        }
    }

    PathBuf::from(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" })
}
