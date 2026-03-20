use std::{
    env,
    io::{Read, Write},
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
use serde::Deserialize;
use tungstenite::{Message, connect};
use url::Url;

const MEDIA_PACKET_MAGIC: &[u8; 4] = b"BKWM";
const MEDIA_PACKET_HEADER_LEN: usize = 8;

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
}

#[derive(Deserialize)]
struct H264Config {
    width: u32,
    height: u32,
}

impl H264DecoderSession {
    fn new(width: u32, height: u32, session_id: String, event_tx: Sender<MediaEvent>) -> Result<Self, String> {
        let mut child = Command::new(ffmpeg_executable_path())
            .arg("-loglevel")
            .arg("error")
            .arg("-fflags")
            .arg("nobuffer")
            .arg("-flags")
            .arg("low_delay")
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
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| error.to_string())?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ffmpeg decoder stdin is not available".to_owned())?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| "ffmpeg decoder stdout is not available".to_owned())?;

        thread::spawn(move || {
            let frame_len = (width as usize)
                .saturating_mul(height as usize)
                .saturating_mul(4);
            let mut frame = vec![0_u8; frame_len];

            loop {
                if stdout.read_exact(&mut frame).is_err() {
                    break;
                }
                let _ = event_tx.send(MediaEvent::Frame {
                    session_id: session_id.clone(),
                    codec: MediaCodec::H264,
                    bytes: frame.clone(),
                    width: Some(width),
                    height: Some(height),
                });
            }
        });

        Ok(Self { child, stdin })
    }

    fn push_packet(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.stdin.write_all(bytes).map_err(|error| error.to_string())
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
                                                h264_decoder = H264DecoderSession::new(
                                                    config.width,
                                                    config.height,
                                                    session_id.clone(),
                                                    event_tx.clone(),
                                                )
                                                .ok();
                                            }
                                        }
                                        (MediaCodec::H264, MediaPacketKind::Frame) => {
                                            if let Some(decoder) = &mut h264_decoder {
                                                let _ = decoder.push_packet(&payload);
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
                Err(_) => {}
            }

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
