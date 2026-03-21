use std::{
    sync::{Arc, atomic::{AtomicBool, Ordering}},
    thread,
    time::Duration,
};

use crossbeam_channel::Sender;
use openh264::{
    NalParser,
    decoder::Decoder,
    formats::YUVSource,
};
use serde::Deserialize;
use tungstenite::{Message, connect};
use url::Url;

use crate::logging;

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
    decoder: Decoder,
    parser: NalParser,
    session_id: String,
    event_tx: Sender<MediaEvent>,
    decoded_frame_count: u64,
}

#[derive(Clone, Copy, Deserialize)]
struct H264Config {
    width: u32,
    height: u32,
}

impl H264DecoderSession {
    fn new(
        width: u32,
        height: u32,
        session_id: String,
        event_tx: Sender<MediaEvent>,
    ) -> Result<Self, String> {
        logging::append_log(
            "INFO",
            "media.h264_decoder",
            format!(
                "starting decoder=openh264 width={} height={} session_id={}",
                width,
                height,
                session_id
            ),
        );
        Ok(Self {
            decoder: Decoder::new().map_err(|error| error.to_string())?,
            parser: NalParser::new(),
            session_id,
            event_tx,
            decoded_frame_count: 0,
        })
    }

    fn push_packet(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.parser.feed(bytes);
        while let Some(nal) = self.parser.next() {
            match self.decoder.decode(&nal) {
                Ok(Some(yuv)) => {
                    let (width, height) = yuv.dimensions();
                    let mut rgba = vec![0_u8; width.saturating_mul(height).saturating_mul(4)];
                    yuv.write_rgba8(&mut rgba);
                    self.decoded_frame_count = self.decoded_frame_count.saturating_add(1);
                    if self.decoded_frame_count == 1 || self.decoded_frame_count % 120 == 0 {
                        logging::append_log(
                            "INFO",
                            "media.h264_decoder",
                            format!("decoded_frames={}", self.decoded_frame_count),
                        );
                    }
                    let _ = self.event_tx.send(MediaEvent::Frame {
                        session_id: self.session_id.clone(),
                        codec: MediaCodec::H264,
                        bytes: rgba,
                        width: Some(width as u32),
                        height: Some(height as u32),
                    });
                }
                Ok(None) => {}
                Err(error) => {
                    logging::append_log(
                        "WARN",
                        "media.h264_decoder",
                        format!("openh264 decode error: {}", error),
                    );
                }
            }
        }
        Ok(())
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
                                                    config.width,
                                                    config.height,
                                                    session_id.clone(),
                                                    event_tx.clone(),
                                                )
                                                .ok();
                                            }
                                        }
                                        (MediaCodec::H264, MediaPacketKind::Frame) => {
                                            if h264_decoder.is_none()
                                                && let Some(config) = h264_config
                                            {
                                                h264_decoder = ensure_h264_decoder(
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
                                                        format!("packet decode failed: {}", error),
                                                    );
                                                    h264_decoder = None;
                                                }
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

fn ensure_h264_decoder(
    width: u32,
    height: u32,
    session_id: String,
    event_tx: Sender<MediaEvent>,
) -> Result<H264DecoderSession, String> {
    H264DecoderSession::new(width, height, session_id, event_tx)
}
