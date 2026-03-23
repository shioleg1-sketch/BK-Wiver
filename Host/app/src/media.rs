use std::{
    env,
    io::{Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::json;
use tungstenite::{Message, connect};
use url::Url;

use crate::{capture::CaptureEngine, logging};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const MEDIA_PACKET_MAGIC: &[u8; 4] = b"BKWM";
const MEDIA_PACKET_VERSION: u8 = 1;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const IVF_HEADER_LEN: usize = 32;
const IVF_FRAME_HEADER_LEN: usize = 12;
const VP8_FRAME_CHUNK_MAGIC: &[u8; 4] = b"BKWC";
const VP8_FRAME_CHUNK_HEADER_LEN: usize = 16;
const VP8_FRAME_CHUNK_DATA_LEN: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamCodec {
    H264,
    Vp8,
}

impl StreamCodec {
    pub fn from_wire(value: &str) -> Self {
        match value {
            "vp8" => Self::Vp8,
            _ => Self::H264,
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::H264 => 2,
            Self::Vp8 => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MediaPacketKind {
    Config,
    Frame,
}

impl MediaPacketKind {
    fn code(self) -> u8 {
        match self {
            Self::Config => 1,
            Self::Frame => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamProfile {
    Fast,
    Balanced,
    Sharp,
}

impl StreamProfile {
    pub fn from_wire(value: &str) -> Self {
        match value {
            "fast" => Self::Fast,
            "sharp" => Self::Sharp,
            _ => Self::Balanced,
        }
    }

    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Balanced => "balanced",
            Self::Sharp => "sharp",
        }
    }

    fn max_dimensions(self) -> (u32, u32) {
        match self {
            Self::Fast => (960, 540),
            Self::Balanced => (1280, 720),
            Self::Sharp => (1600, 900),
        }
    }

    fn target_frame_interval(self) -> Duration {
        Duration::from_secs_f64(1.0 / self.target_fps() as f64)
    }

    fn target_fps(self) -> u32 {
        match self {
            Self::Fast => 36,
            Self::Balanced => 30,
            Self::Sharp => 30,
        }
    }

    fn target_crf(self) -> &'static str {
        match self {
            Self::Fast => "35",
            Self::Balanced => "31",
            Self::Sharp => "27",
        }
    }

    fn target_bitrate(self) -> &'static str {
        match self {
            Self::Fast => "2600k",
            Self::Balanced => "5200k",
            Self::Sharp => "9000k",
        }
    }

    fn target_deadline(self) -> &'static str {
        match self {
            Self::Fast => "realtime",
            Self::Balanced => "realtime",
            Self::Sharp => "realtime",
        }
    }

    fn target_cpu_used(self) -> &'static str {
        match self {
            Self::Fast => "9",
            Self::Balanced => "7",
            Self::Sharp => "6",
        }
    }
}

struct Vp8EncoderSession {
    child: Child,
    stdin: ChildStdin,
    packet_rx: std::sync::mpsc::Receiver<Vec<u8>>,
    width: u32,
    height: u32,
    profile: StreamProfile,
}

struct H264EncoderSession {
    child: Child,
    stdin: ChildStdin,
    packet_rx: Receiver<Vec<u8>>,
    width: u32,
    height: u32,
}

impl H264EncoderSession {
    fn new(width: u32, height: u32, profile: StreamProfile) -> Result<Self, String> {
        let ffmpeg = ffmpeg_executable_path();
        logging::append_log(
            "INFO",
            "media.h264_encoder",
            format!(
                "starting ffmpeg={} encoder=libx264 width={} height={} fps={}",
                ffmpeg.display(),
                width,
                height,
                profile.target_fps()
            ),
        );

        let mut command = Command::new(ffmpeg);
        command
            .arg("-loglevel")
            .arg("error")
            .arg("-f")
            .arg("rawvideo")
            .arg("-pix_fmt")
            .arg("rgba")
            .arg("-s")
            .arg(format!("{width}x{height}"))
            .arg("-r")
            .arg(profile.target_fps().to_string())
            .arg("-i")
            .arg("pipe:0")
            .arg("-an")
            .arg("-c:v")
            .arg("libx264")
            .arg("-preset")
            .arg("ultrafast")
            .arg("-tune")
            .arg("zerolatency")
            .arg("-profile:v")
            .arg("baseline")
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg("-bf")
            .arg("0")
            .arg("-refs")
            .arg("1")
            .arg("-crf")
            .arg(profile.target_crf())
            .arg("-maxrate")
            .arg(profile.target_bitrate())
            .arg("-bufsize")
            .arg(profile.target_bitrate())
            .arg("-g")
            .arg(profile.target_fps().to_string())
            .arg("-keyint_min")
            .arg(profile.target_fps().to_string())
            .arg("-x264-params")
            .arg("scenecut=0:repeat-headers=1:bframes=0:sync-lookahead=0:rc-lookahead=0:sliced-threads=1")
            .arg("-f")
            .arg("h264")
            .arg("pipe:1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        configure_hidden_process(&mut command);

        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ffmpeg stdin is not available".to_owned())?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| "ffmpeg stdout is not available".to_owned())?;
        let (packet_tx, packet_rx) = mpsc::channel();

        thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match stdout.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        if packet_tx.send(buffer[..read].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            child,
            stdin,
            packet_rx,
            width,
            height,
        })
    }

    fn push_frame(&mut self, image: &image::RgbaImage) -> Result<(), String> {
        self.stdin
            .write_all(image.as_raw())
            .map_err(|error| error.to_string())
    }

    fn drain_packets(&self) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        while let Ok(packet) = self.packet_rx.try_recv() {
            packets.push(packet);
        }
        packets
    }

    fn matches(&self, width: u32, height: u32) -> bool {
        self.width == width && self.height == height
    }

    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

impl Drop for H264EncoderSession {
    fn drop(&mut self) {
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Vp8EncoderSession {
    fn new(width: u32, height: u32, profile: StreamProfile) -> Result<Self, String> {
        let ffmpeg = ffmpeg_executable_path();
        logging::append_log(
            "INFO",
            "media.vp8_encoder",
            format!(
                "starting ffmpeg={} encoder=libvpx width={} height={} fps={}",
                ffmpeg.display(),
                width,
                height,
                profile.target_fps()
            ),
        );

        let mut command = Command::new(ffmpeg);
        command
            .arg("-loglevel")
            .arg("error")
            .arg("-f")
            .arg("rawvideo")
            .arg("-pix_fmt")
            .arg("rgba")
            .arg("-s")
            .arg(format!("{width}x{height}"))
            .arg("-r")
            .arg(profile.target_fps().to_string())
            .arg("-i")
            .arg("pipe:0")
            .arg("-an")
            .arg("-c:v")
            .arg("libvpx")
            .arg("-deadline")
            .arg(profile.target_deadline())
            .arg("-cpu-used")
            .arg(profile.target_cpu_used())
            .arg("-lag-in-frames")
            .arg("0")
            .arg("-error-resilient")
            .arg("1")
            .arg("-auto-alt-ref")
            .arg("0")
            .arg("-g")
            .arg(profile.target_fps().to_string())
            .arg("-keyint_min")
            .arg(profile.target_fps().to_string())
            .arg("-crf")
            .arg(profile.target_crf())
            .arg("-b:v")
            .arg(profile.target_bitrate())
            .arg("-threads")
            .arg("4")
            .arg("-f")
            .arg("ivf")
            .arg("pipe:1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ffmpeg stdin is not available".to_owned())?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| "ffmpeg stdout is not available".to_owned())?;
        let (packet_tx, packet_rx) = std::sync::mpsc::channel();

        thread::spawn(move || {
            let mut ivf_header = vec![0_u8; IVF_HEADER_LEN];
            if stdout.read_exact(&mut ivf_header).is_err() {
                return;
            }
            if packet_tx.send(ivf_header).is_err() {
                return;
            }

            loop {
                let mut frame_header = [0_u8; IVF_FRAME_HEADER_LEN];
                if stdout.read_exact(&mut frame_header).is_err() {
                    break;
                }

                let frame_len = u32::from_le_bytes([
                    frame_header[0],
                    frame_header[1],
                    frame_header[2],
                    frame_header[3],
                ]) as usize;
                let mut packet = Vec::with_capacity(IVF_FRAME_HEADER_LEN + frame_len);
                packet.extend_from_slice(&frame_header);

                let mut frame_payload = vec![0_u8; frame_len];
                if stdout.read_exact(&mut frame_payload).is_err() {
                    break;
                }
                packet.extend_from_slice(&frame_payload);

                if packet_tx.send(packet).is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            child,
            stdin,
            packet_rx,
            width,
            height,
            profile,
        })
    }

    fn push_frame(&mut self, image: &image::RgbaImage) -> Result<(), String> {
        self.stdin
            .write_all(image.as_raw())
            .map_err(|error| error.to_string())
    }

    fn drain_packets(&self) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        while let Ok(packet) = self.packet_rx.try_recv() {
            packets.push(packet);
        }
        packets
    }

    fn matches(&self, width: u32, height: u32, profile: StreamProfile) -> bool {
        self.width == width && self.height == height && self.profile == profile
    }

    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

impl Drop for Vp8EncoderSession {
    fn drop(&mut self) {
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn spawn_stream(
    server_url: String,
    token: String,
    session_id: String,
    stop_flag: Arc<AtomicBool>,
    profile: Arc<Mutex<StreamProfile>>,
    codec_preference: Arc<Mutex<StreamCodec>>,
) {
    thread::spawn(move || {
        let Ok(url) = media_url(&server_url, &token, &session_id) else {
            return;
        };

        let mut frame_index = 0_u32;
        let mut capture_engine = CaptureEngine::new();

        while !stop_flag.load(Ordering::Relaxed) {
            match connect(url.as_str()) {
                Ok((mut socket, _)) => {
                    let mut h264_encoder: Option<H264EncoderSession> = None;
                    let mut h264_config_sent = false;
                    let mut vp8_encoder: Option<Vp8EncoderSession> = None;
                    let mut vp8_config_sent = false;
                    let mut vp8_header_buffer = Vec::new();
                    let mut vp8_chunks_sent = 0_u64;
                    let mut h264_packets_sent = 0_u64;
                    let mut sent_frames = 0_u64;

                    while !stop_flag.load(Ordering::Relaxed) {
                        let stream_profile =
                            profile.lock().map(|guard| *guard).unwrap_or(StreamProfile::Balanced);
                        let preferred_codec = codec_preference
                            .lock()
                            .map(|guard| *guard)
                            .unwrap_or(StreamCodec::H264);
                        let capture_started_at = Instant::now();
                        let captured =
                            capture_engine.capture(stream_profile.max_dimensions(), frame_index);
                        let capture_ms = capture_started_at.elapsed().as_millis();
                        let frame_image = captured.image;
                        frame_index = frame_index.wrapping_add(1);

                        if captured.used_fallback && frame_index % 60 == 1 {
                            logging::append_log(
                                "WARN",
                                "capture",
                                format!("fallback frame active backend={}", captured.backend),
                            );
                        }

                        match preferred_codec {
                            StreamCodec::H264 => {
                                let mut sent_packets_this_frame = 0_u64;
                                if ensure_h264_encoder(
                                    &mut h264_encoder,
                                    frame_image.width(),
                                    frame_image.height(),
                                    stream_profile,
                                )
                                .is_ok()
                                {
                                    if let Some(encoder) = &mut h264_encoder {
                                        if encoder.push_frame(&frame_image).is_ok() {
                                            let packets = encoder.drain_packets();
                                            if !packets.is_empty() && !h264_config_sent {
                                                let (width, height) = encoder.dimensions();
                                                let config = json!({
                                                    "width": width,
                                                    "height": height,
                                                });
                                                let payload = serde_json::to_vec(&config)
                                                    .map_err(|error| error.to_string())
                                                    .unwrap_or_default();
                                                if payload.is_empty() {
                                                    break;
                                                }
                                                let packet = encode_media_packet(
                                                    StreamCodec::H264,
                                                    MediaPacketKind::Config,
                                                    &payload,
                                                );
                                                if socket.send(Message::Binary(packet.into())).is_err()
                                                {
                                                    break;
                                                }
                                                h264_config_sent = true;
                                                logging::append_log(
                                                    "INFO",
                                                    "media.h264_encoder",
                                                    format!(
                                                        "config sent encoder=libx264 width={} height={}",
                                                        width, height
                                                    ),
                                                );
                                            }

                                            for packet in packets {
                                                let packet = encode_media_packet(
                                                    StreamCodec::H264,
                                                    MediaPacketKind::Frame,
                                                    &packet,
                                                );
                                                if socket.send(Message::Binary(packet.into())).is_err()
                                                {
                                                    break;
                                                }
                                                sent_packets_this_frame =
                                                    sent_packets_this_frame.saturating_add(1);
                                                h264_packets_sent =
                                                    h264_packets_sent.saturating_add(1);
                                                if h264_packets_sent == 1
                                                    || h264_packets_sent % 120 == 0
                                                {
                                                    logging::append_log(
                                                        "INFO",
                                                        "media.h264_encoder",
                                                        format!(
                                                            "sent_h264_packets={}",
                                                            h264_packets_sent
                                                        ),
                                                    );
                                                }
                                            }
                                        } else {
                                            logging::append_log(
                                                "WARN",
                                                "media.h264_encoder",
                                                "ffmpeg stdin write failed, restarting encoder",
                                            );
                                            h264_encoder = None;
                                            h264_config_sent = false;
                                        }
                                    }
                                } else {
                                    logging::append_log(
                                        "WARN",
                                        "media.h264_encoder",
                                        "failed to start encoder",
                                    );
                                    h264_encoder = None;
                                    h264_config_sent = false;
                                }

                                let total_ms = capture_started_at.elapsed().as_millis();
                                let encode_ms = total_ms.saturating_sub(capture_ms);
                                sent_frames = sent_frames.saturating_add(1);
                                if sent_frames <= 5 || sent_frames % 60 == 0 {
                                    logging::append_log(
                                        "INFO",
                                        "media.perf",
                                        format!(
                                            "session_id={} codec=h264 profile={} backend={} frame={} capture_ms={} encode_ms={} send_ms={} packets={} total_ms={}",
                                            session_id,
                                            stream_profile.wire_name(),
                                            captured.backend,
                                            sent_frames,
                                            capture_ms,
                                            encode_ms,
                                            0,
                                            sent_packets_this_frame,
                                            total_ms
                                        ),
                                    );
                                }
                            }
                            StreamCodec::Vp8 => {
                                let mut sent_packets_this_frame = 0_u64;
                                match ensure_vp8_encoder(
                                    &mut vp8_encoder,
                                    frame_image.width(),
                                    frame_image.height(),
                                    stream_profile,
                                ) {
                                    Ok(()) => {
                                        if let Some(encoder) = &mut vp8_encoder {
                                            if encoder.push_frame(&frame_image).is_ok() {
                                                let chunks = encoder.drain_packets();
                                                for chunk in chunks {
                                                    if !vp8_config_sent {
                                                        vp8_header_buffer.extend_from_slice(&chunk);
                                                        if vp8_header_buffer.len() < IVF_HEADER_LEN {
                                                            continue;
                                                        }

                                                        let header = vp8_header_buffer[..IVF_HEADER_LEN].to_vec();
                                                        let config_packet = encode_media_packet(
                                                            StreamCodec::Vp8,
                                                            MediaPacketKind::Config,
                                                            &header,
                                                        );
                                                        if socket
                                                            .send(Message::Binary(config_packet.into()))
                                                            .is_err()
                                                        {
                                                            break;
                                                        }
                                                        let (width, height) = encoder.dimensions();
                                                        logging::append_log(
                                                            "INFO",
                                                            "media.vp8_encoder",
                                                            format!(
                                                                "config sent encoder=libvpx width={} height={}",
                                                                width, height
                                                            ),
                                                        );
                                                        vp8_config_sent = true;

                                                        if vp8_header_buffer.len() > IVF_HEADER_LEN {
                                                            let remainder =
                                                                vp8_header_buffer[IVF_HEADER_LEN..]
                                                                    .to_vec();
                                                            if send_vp8_frame_chunks(
                                                                &mut socket,
                                                                &remainder,
                                                                &mut vp8_chunks_sent,
                                                            )
                                                            .is_err()
                                                            {
                                                                break;
                                                            }
                                                            sent_packets_this_frame =
                                                                sent_packets_this_frame.saturating_add(1);
                                                        }
                                                        vp8_header_buffer.clear();
                                                    } else if send_vp8_frame_chunks(
                                                        &mut socket,
                                                        &chunk,
                                                        &mut vp8_chunks_sent,
                                                    )
                                                    .is_err()
                                                    {
                                                        break;
                                                    } else {
                                                        sent_packets_this_frame =
                                                            sent_packets_this_frame.saturating_add(1);
                                                    }
                                                }
                                            } else {
                                                vp8_encoder = None;
                                                vp8_config_sent = false;
                                                vp8_header_buffer.clear();
                                            }
                                        }
                                    }
                                    Err(_) => {
                                        vp8_encoder = None;
                                        vp8_config_sent = false;
                                        vp8_header_buffer.clear();
                                    }
                                }

                                let total_ms = capture_started_at.elapsed().as_millis();
                                let encode_ms = total_ms.saturating_sub(capture_ms);
                                sent_frames = sent_frames.saturating_add(1);
                                if sent_frames <= 5 || sent_frames % 60 == 0 {
                                    logging::append_log(
                                        "INFO",
                                        "media.perf",
                                        format!(
                                            "session_id={} codec=vp8 profile={} backend={} frame={} capture_ms={} encode_ms={} send_ms={} packets={} total_ms={}",
                                            session_id,
                                            stream_profile.wire_name(),
                                            captured.backend,
                                            sent_frames,
                                            capture_ms,
                                            encode_ms,
                                            0,
                                            sent_packets_this_frame,
                                            total_ms
                                        ),
                                    );
                                }
                            }
                        };

                        let frame_elapsed = capture_started_at.elapsed();
                        if let Some(remaining) =
                            stream_profile.target_frame_interval().checked_sub(frame_elapsed)
                        {
                            thread::sleep(remaining);
                        }
                    }

                    let _ = socket.close(None);
                }
                Err(_) => {
                    thread::sleep(Duration::from_secs(2));
                }
            }
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

pub fn ffmpeg_executable_path() -> PathBuf {
    if cfg!(windows)
        && let Ok(current_exe) = env::current_exe()
        && let Some(parent) = current_exe.parent()
    {
        let bundled = parent.join("ffmpeg.exe");
        if bundled.exists() {
            return bundled;
        }
    }

    PathBuf::from(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" })
}

fn ensure_h264_encoder(
    encoder: &mut Option<H264EncoderSession>,
    width: u32,
    height: u32,
    profile: StreamProfile,
) -> Result<(), String> {
    let needs_restart = encoder
        .as_ref()
        .map(|active| !active.matches(width, height))
        .unwrap_or(true);

    if !needs_restart {
        return Ok(());
    }

    match H264EncoderSession::new(width, height, profile) {
        Ok(session) => {
            *encoder = Some(session);
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn configure_hidden_process(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_hidden_process(_command: &mut Command) {
}

fn ensure_vp8_encoder(
    encoder: &mut Option<Vp8EncoderSession>,
    width: u32,
    height: u32,
    profile: StreamProfile,
) -> Result<(), String> {
    let needs_restart = encoder
        .as_ref()
        .map(|active| !active.matches(width, height, profile))
        .unwrap_or(true);

    if !needs_restart {
        return Ok(());
    }

    match Vp8EncoderSession::new(width, height, profile) {
        Ok(session) => {
            *encoder = Some(session);
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn encode_media_packet(codec: StreamCodec, kind: MediaPacketKind, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8 + payload.len());
    bytes.extend_from_slice(MEDIA_PACKET_MAGIC);
    bytes.push(MEDIA_PACKET_VERSION);
    bytes.push(codec.code());
    bytes.push(kind.code());
    bytes.push(0);
    bytes.extend_from_slice(payload);
    bytes
}

fn send_vp8_frame_chunks(
    socket: &mut tungstenite::WebSocket<
        tungstenite::stream::MaybeTlsStream<std::net::TcpStream>,
    >,
    frame_packet: &[u8],
    chunk_counter: &mut u64,
) -> Result<(), ()> {
    let total_len = u32::try_from(frame_packet.len()).map_err(|_| ())?;
    let mut offset = 0_usize;

    while offset < frame_packet.len() {
        let end = frame_packet
            .len()
            .min(offset.saturating_add(VP8_FRAME_CHUNK_DATA_LEN));
        let chunk_len = end.saturating_sub(offset);
        let mut payload = Vec::with_capacity(VP8_FRAME_CHUNK_HEADER_LEN + chunk_len);
        payload.extend_from_slice(VP8_FRAME_CHUNK_MAGIC);
        payload.extend_from_slice(&total_len.to_le_bytes());
        payload.extend_from_slice(&(offset as u32).to_le_bytes());
        payload.extend_from_slice(&(chunk_len as u32).to_le_bytes());
        payload.extend_from_slice(&frame_packet[offset..end]);

        let frame_packet = encode_media_packet(StreamCodec::Vp8, MediaPacketKind::Frame, &payload);
        socket
            .send(Message::Binary(frame_packet.into()))
            .map_err(|_| ())?;
        *chunk_counter = chunk_counter.saturating_add(1);
        offset = end;
    }

    Ok(())
}
