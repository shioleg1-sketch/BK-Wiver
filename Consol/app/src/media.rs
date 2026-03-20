use std::{
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
    },
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
                    let _ = event_tx.send(MediaEvent::Connected {
                        session_id: session_id.clone(),
                    });

                    while !stop_flag.load(Ordering::Relaxed) {
                        match socket.read() {
                            Ok(Message::Binary(bytes)) => {
                                if let Some((codec, payload)) = decode_media_packet(bytes.as_ref()) {
                                    let _ = event_tx.send(MediaEvent::Frame {
                                        session_id: session_id.clone(),
                                        codec,
                                        bytes: payload,
                                    });
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

fn decode_media_packet(bytes: &[u8]) -> Option<(MediaCodec, Vec<u8>)> {
    if bytes.len() < MEDIA_PACKET_HEADER_LEN {
        return Some((MediaCodec::Jpeg, bytes.to_vec()));
    }
    if &bytes[..4] != MEDIA_PACKET_MAGIC {
        return Some((MediaCodec::Jpeg, bytes.to_vec()));
    }

    let codec = match bytes[5] {
        1 => MediaCodec::Jpeg,
        2 => MediaCodec::H264,
        _ => return None,
    };
    let kind = bytes[6];
    if kind != 2 {
        return None;
    }

    Some((codec, bytes[MEDIA_PACKET_HEADER_LEN..].to_vec()))
}
