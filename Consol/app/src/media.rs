use std::{
    fs::File,
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
use openh264::{
    NalParser,
    decoder::Decoder,
    formats::YUVSource,
};
use serde::Deserialize;
use tungstenite::{Message, connect};
use url::Url;

use crate::logging;

// Протокол v2: 16-байт заголовок с PTS (обратная совместимость с v1: 8 байт)
const MEDIA_PACKET_MAGIC: &[u8; 4] = b"BKWM";
const MEDIA_PACKET_HEADER_LEN_V1: usize = 8;
const MEDIA_PACKET_HEADER_LEN_V2: usize = 16;
const IVF_HEADER_LEN: usize = 32;
const H264_DUMP_LIMIT_BYTES: usize = 256 * 1024;
// improvement 9: VP8 chunking removed, but keep constants for backward compatibility
const VP8_FRAME_CHUNK_MAGIC: &[u8; 4] = b"BKWC";
const VP8_FRAME_CHUNK_HEADER_LEN: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaCodec {
    H264,
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

struct H264DecoderSession {
    decoder: Decoder,
    parser: NalParser,
    session_id: String,
    event_tx: Sender<MediaEvent>,
    packet_write_count: u64,
    decoded_frame_count: u64,
}

struct H264DumpSession {
    file: File,
    bytes_written: usize,
    path: PathBuf,
}

impl H264DumpSession {
    fn new(session_id: &str) -> Result<Self, String> {
        let path = h264_dump_path(session_id);
        let file = File::create(&path).map_err(|error| error.to_string())?;
        Ok(Self {
            file,
            bytes_written: 0,
            path,
        })
    }

    fn write_packet(&mut self, bytes: &[u8], session_id: &str) {
        if self.bytes_written >= H264_DUMP_LIMIT_BYTES {
            return;
        }
        let remaining = H264_DUMP_LIMIT_BYTES.saturating_sub(self.bytes_written);
        let chunk = &bytes[..bytes.len().min(remaining)];
        if self.file.write_all(chunk).is_err() {
            self.bytes_written = H264_DUMP_LIMIT_BYTES;
            return;
        }
        self.bytes_written = self.bytes_written.saturating_add(chunk.len());
        if self.bytes_written >= H264_DUMP_LIMIT_BYTES {
            logging::append_log(
                "INFO",
                "media.h264_dump",
                format!(
                    "capture complete session_id={} bytes={} path={}",
                    session_id,
                    self.bytes_written,
                    self.path.display()
                ),
            );
        }
    }
}

fn h264_dump_path(session_id: &str) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        return home
            .join("Library")
            .join("Application Support")
            .join("BK-Wiver")
            .join("state")
            .join(format!("h264-dump-{session_id}.h264"));
    }

    #[cfg(not(target_os = "macos"))]
    {
        let local_app_data = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        local_app_data
            .join("BK-Wiver")
            .join("state")
            .join(format!("h264-dump-{session_id}.h264"))
    }
}

#[derive(Clone, Copy, Deserialize)]
struct H264Config {
    width: u32,
    height: u32,
}

struct Vp8DecoderSession {
    child: Child,
    stdin: ChildStdin,
    session_id: String,
    pushed_calls: u64,
    pushed_bytes: usize,
}

struct Vp8SampleCapture {
    bytes: Vec<u8>,
    dumped: bool,
}

struct Vp8FrameAssembler {
    buffer: Vec<u8>,
    expected_len: usize,
    received_len: usize,
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
                width, height, session_id
            ),
        );
        Ok(Self {
            decoder: Decoder::new().map_err(|error| error.to_string())?,
            parser: NalParser::new(),
            session_id,
            event_tx,
            packet_write_count: 0,
            decoded_frame_count: 0,
        })
    }

    fn push_packet(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.packet_write_count = self.packet_write_count.saturating_add(1);
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
                            format!(
                                "decoded_frames={} session_id={}",
                                self.decoded_frame_count, self.session_id
                            ),
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
                        format!("openh264 decode error session_id={} {}", self.session_id, error),
                    );
                }
            }
        }
        Ok(())
    }
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
            logging::append_log(
                "INFO",
                "media.vp8_decoder",
                format!(
                    "decoder stdout reader started session_id={} frame_size={}",
                    session_id_for_stdout, frame_size
                ),
            );
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

        Ok(Self {
            child,
            stdin,
            session_id,
            pushed_calls: 0,
            pushed_bytes: 0,
        })
    }

    fn push_bytes(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.pushed_calls = self.pushed_calls.saturating_add(1);
        self.pushed_bytes = self.pushed_bytes.saturating_add(bytes.len());
        if self.pushed_calls <= 3 || self.pushed_calls % 120 == 0 {
            logging::append_log(
                "INFO",
                "media.vp8_decoder",
                format!(
                    "stdin push session_id={} call={} bytes={} total_bytes={}",
                    self.session_id,
                    self.pushed_calls,
                    bytes.len(),
                    self.pushed_bytes
                ),
            );
        }
        self.stdin
            .write_all(bytes)
            .map_err(|error| error.to_string())?;
        self.stdin.flush().map_err(|error| error.to_string())
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
                    let mut h264_decoder: Option<H264DecoderSession> = None;
                    let mut h264_dump: Option<H264DumpSession> = None;
                    let mut h264_config: Option<H264Config> = None;
                    let mut h264_packet_count = 0_u64;
                    let mut vp8_decoder: Option<Vp8DecoderSession> = None;
                    let mut vp8_frame_packet_count = 0_u64;
                    let mut vp8_sample_capture: Option<Vp8SampleCapture> = None;
                    // improvement 9: VP8 chunking removed — frames arrive as single message
                    let _vp8_frame_assembler: Option<Vp8FrameAssembler> = None;
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
                                        (MediaCodec::H264, MediaPacketKind::Config) => {
                                            if let Ok(config) =
                                                serde_json::from_slice::<H264Config>(&payload)
                                            {
                                                h264_config = Some(config);
                                                h264_dump = H264DumpSession::new(&session_id).ok();
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
                                            h264_packet_count = h264_packet_count.saturating_add(1);
                                            if let Some(dump) = &mut h264_dump {
                                                dump.write_packet(&payload, &session_id);
                                            }
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
                                                        format!(
                                                            "packet decode failed session_id={} error={}",
                                                            session_id, error
                                                        ),
                                                    );
                                                    h264_decoder = None;
                                                }
                                            }
                                        }
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
                                            // improvement 9: no chunking — payload is the complete frame
                                            let frame_packet = Some(payload.clone());

                                            if let Some(frame_packet) = frame_packet {
                                                if let Some(sample) = &mut vp8_sample_capture
                                                    && !sample.dumped
                                                {
                                                    sample.bytes.extend_from_slice(&frame_packet);
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
                                                    if let Err(error) = decoder.push_bytes(&frame_packet)
                                                    {
                                                        logging::append_log(
                                                            "ERROR",
                                                            "media.vp8_decoder",
                                                            format!(
                                                                "packet decode failed: {}",
                                                                error
                                                            ),
                                                        );
                                                        vp8_decoder = None;
                                                    }
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
    if bytes.len() < MEDIA_PACKET_HEADER_LEN_V1 || &bytes[..4] != MEDIA_PACKET_MAGIC {
        return None;
    }

    let version = bytes[4];
    let header_len = match version {
        1 => MEDIA_PACKET_HEADER_LEN_V1,
        2 => MEDIA_PACKET_HEADER_LEN_V2,
        _ => return None,
    };

    if bytes.len() < header_len {
        return None;
    }

    let codec = match bytes[5] {
        1 => MediaCodec::Vp8,
        2 => MediaCodec::H264,
        _ => return None,
    };
    let kind = match bytes[6] {
        1 => MediaPacketKind::Config,
        2 => MediaPacketKind::Frame,
        _ => return None,
    };

    // v2: flags в bytes[7], PTS в bytes[8..16]
    // improvement 7: I-frame flag можно использовать для приоритета декодирования
    let _is_i_frame = version == 2 && (bytes[7] & 0x01) != 0;
    let _pts_us = if version == 2 && bytes.len() >= MEDIA_PACKET_HEADER_LEN_V2 {
        i64::from_le_bytes(bytes[8..16].try_into().ok()?)
    } else {
        0
    };

    Some((codec, kind, bytes[header_len..].to_vec()))
}

fn ensure_h264_decoder(
    width: u32,
    height: u32,
    session_id: String,
    event_tx: Sender<MediaEvent>,
) -> Result<H264DecoderSession, String> {
    H264DecoderSession::new(width, height, session_id, event_tx)
}

fn parse_ivf_dimensions(header: &[u8]) -> Option<(u32, u32)> {
    if header.len() < IVF_HEADER_LEN || &header[..4] != b"DKIF" || &header[8..12] != b"VP80" {
        return None;
    }
    let width = u16::from_le_bytes([header[12], header[13]]) as u32;
    let height = u16::from_le_bytes([header[14], header[15]]) as u32;
    Some((width, height))
}

fn decode_vp8_frame_chunk(
    payload: &[u8],
    assembler: &mut Option<Vp8FrameAssembler>,
) -> Result<Option<Vec<u8>>, String> {
    if payload.len() < VP8_FRAME_CHUNK_HEADER_LEN || &payload[..4] != VP8_FRAME_CHUNK_MAGIC {
        return Ok(Some(payload.to_vec()));
    }

    let total_len = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;
    let offset = u32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]) as usize;
    let chunk_len =
        u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]) as usize;
    let chunk = &payload[VP8_FRAME_CHUNK_HEADER_LEN..];

    if chunk_len != chunk.len() {
        return Err(format!(
            "chunk size mismatch declared={} actual={}",
            chunk_len,
            chunk.len()
        ));
    }
    if total_len == 0 {
        return Err("empty frame payload".to_owned());
    }
    if offset > total_len || chunk_len > total_len.saturating_sub(offset) {
        return Err(format!(
            "invalid chunk bounds total={} offset={} chunk={}",
            total_len, offset, chunk_len
        ));
    }

    if offset == 0 || assembler.as_ref().map(|active| active.expected_len) != Some(total_len) {
        *assembler = Some(Vp8FrameAssembler {
            buffer: Vec::with_capacity(total_len),
            expected_len: total_len,
            received_len: 0,
        });
    }

    let Some(active) = assembler.as_mut() else {
        return Err("frame assembler is unavailable".to_owned());
    };

    if active.received_len != offset {
        return Err(format!(
            "unexpected chunk offset expected={} actual={}",
            active.received_len, offset
        ));
    }

    active.buffer.extend_from_slice(chunk);
    active.received_len = active.received_len.saturating_add(chunk_len);

    if active.received_len < active.expected_len {
        return Ok(None);
    }
    if active.received_len > active.expected_len {
        return Err(format!(
            "frame overflow received={} expected={}",
            active.received_len, active.expected_len
        ));
    }

    let completed = std::mem::take(&mut active.buffer);
    *assembler = None;
    Ok(Some(completed))
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

    #[cfg(target_os = "macos")]
    {
        for candidate in [
            "/opt/homebrew/bin/ffmpeg",
            "/usr/local/bin/ffmpeg",
            "/opt/local/bin/ffmpeg",
        ] {
            let path = PathBuf::from(candidate);
            if path.exists() {
                return path;
            }
        }
    }

    PathBuf::from(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" })
}
