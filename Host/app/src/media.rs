use std::{
    env,
    io::{Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use tungstenite::{Message, connect};
use url::Url;

use crate::{capture::CaptureEngine, logging};

const MEDIA_PACKET_MAGIC: &[u8; 4] = b"BKWM";
const MEDIA_PACKET_VERSION: u8 = 1;
const IDLE_REFRESH_INTERVAL_TICKS: u64 = 30;
const IVF_HEADER_LEN: usize = 32;
const IVF_FRAME_HEADER_LEN: usize = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamCodec {
    Vp8,
}

impl StreamCodec {
    pub fn from_wire(_value: &str) -> Self {
        Self::Vp8
    }

    fn code(self) -> u8 {
        match self {
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

    fn active_frame_delay(self) -> Duration {
        match self {
            Self::Fast => Duration::from_millis(28),
            Self::Balanced => Duration::from_millis(34),
            Self::Sharp => Duration::from_millis(33),
        }
    }

    fn idle_frame_delay(self) -> Duration {
        match self {
            Self::Fast => Duration::from_millis(90),
            Self::Balanced => Duration::from_millis(120),
            Self::Sharp => Duration::from_millis(90),
        }
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
            Self::Fast => "1800k",
            Self::Balanced => "3200k",
            Self::Sharp => "7000k",
        }
    }

    fn target_deadline(self) -> &'static str {
        match self {
            Self::Fast => "realtime",
            Self::Balanced => "realtime",
            Self::Sharp => "good",
        }
    }

    fn target_cpu_used(self) -> &'static str {
        match self {
            Self::Fast => "8",
            Self::Balanced => "6",
            Self::Sharp => "5",
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
    _codec_preference: Arc<Mutex<StreamCodec>>,
) {
    thread::spawn(move || {
        let Ok(url) = media_url(&server_url, &token, &session_id) else {
            return;
        };

        let mut frame_index = 0_u32;
        let mut stream_tick = 0_u64;
        let mut capture_engine = CaptureEngine::new();
        let mut previous_signature: Option<Vec<u8>> = None;
        let mut last_sent_tick = 0_u64;

        while !stop_flag.load(Ordering::Relaxed) {
            match connect(url.as_str()) {
                Ok((mut socket, _)) => {
                    let mut vp8_encoder: Option<Vp8EncoderSession> = None;
                    let mut vp8_config_sent = false;
                    let mut vp8_header_buffer = Vec::new();
                    let mut vp8_chunks_sent = 0_u64;
                    let mut perf_log_frame_index = 0_u64;

                    while !stop_flag.load(Ordering::Relaxed) {
                        let loop_started_at = Instant::now();
                        stream_tick = stream_tick.saturating_add(1);
                        let stream_profile =
                            profile.lock().map(|guard| *guard).unwrap_or(StreamProfile::Balanced);

                        let capture_started_at = Instant::now();
                        let captured =
                            capture_engine.capture(stream_profile.max_dimensions(), frame_index);
                        let capture_elapsed = capture_started_at.elapsed();
                        let frame_image = captured.image;
                        frame_index = frame_index.wrapping_add(1);

                        if captured.used_fallback && frame_index % 60 == 1 {
                            logging::append_log(
                                "WARN",
                                "capture",
                                format!("fallback frame active backend={}", captured.backend),
                            );
                        }

                        let signature = frame_signature(frame_image.as_raw());
                        let is_active = previous_signature
                            .as_ref()
                            .map(|previous| signature_distance(previous, &signature) > 2)
                            .unwrap_or(true);
                        previous_signature = Some(signature);
                        let should_refresh_idle =
                            stream_tick.saturating_sub(last_sent_tick) >= IDLE_REFRESH_INTERVAL_TICKS;

                        if !is_active && !should_refresh_idle {
                            thread::sleep(stream_profile.idle_frame_delay());
                            continue;
                        }

                        let mut sent_frame = false;
                        let mut path_label = "vp8";
                        let mut encode_elapsed = Duration::ZERO;
                        let mut send_elapsed = Duration::ZERO;
                        let mut packets_produced = 0_usize;

                        match ensure_vp8_encoder(
                            &mut vp8_encoder,
                            frame_image.width(),
                            frame_image.height(),
                            stream_profile,
                        ) {
                            Ok(()) => {
                                if let Some(encoder) = &mut vp8_encoder {
                                    let encode_started_at = Instant::now();
                                    if encoder.push_frame(&frame_image).is_ok() {
                                        encode_elapsed = encode_started_at.elapsed();
                                        let chunks = encoder.drain_packets();
                                        packets_produced = chunks.len();
                                        let send_started_at = Instant::now();

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
                                                    sent_frame = false;
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
                                                        vp8_header_buffer[IVF_HEADER_LEN..].to_vec();
                                                    let frame_packet = encode_media_packet(
                                                        StreamCodec::Vp8,
                                                        MediaPacketKind::Frame,
                                                        &remainder,
                                                    );
                                                    if socket
                                                        .send(Message::Binary(frame_packet.into()))
                                                        .is_err()
                                                    {
                                                        sent_frame = false;
                                                        break;
                                                    }
                                                    vp8_chunks_sent =
                                                        vp8_chunks_sent.saturating_add(1);
                                                    sent_frame = true;
                                                }
                                                vp8_header_buffer.clear();
                                            } else {
                                                let frame_packet = encode_media_packet(
                                                    StreamCodec::Vp8,
                                                    MediaPacketKind::Frame,
                                                    &chunk,
                                                );
                                                if socket.send(Message::Binary(frame_packet.into())).is_err()
                                                {
                                                    sent_frame = false;
                                                    break;
                                                }
                                                vp8_chunks_sent =
                                                    vp8_chunks_sent.saturating_add(1);
                                                sent_frame = true;
                                            }
                                        }

                                        if sent_frame
                                            && (vp8_chunks_sent == 1 || vp8_chunks_sent % 120 == 0)
                                        {
                                            logging::append_log(
                                                "INFO",
                                                "media.vp8_encoder",
                                                format!("sent_vp8_chunks={}", vp8_chunks_sent),
                                            );
                                        }
                                        send_elapsed = send_started_at.elapsed();
                                    } else {
                                        logging::append_log(
                                            "WARN",
                                            "media.vp8_encoder",
                                            "ffmpeg stdin write failed, restarting encoder",
                                        );
                                        vp8_encoder = None;
                                        vp8_config_sent = false;
                                        vp8_header_buffer.clear();
                                    }
                                }
                            }
                            Err(error) => {
                                logging::append_log(
                                    "WARN",
                                    "media.vp8_encoder",
                                    format!("failed to start encoder: {}", error),
                                );
                                vp8_encoder = None;
                                vp8_config_sent = false;
                                vp8_header_buffer.clear();
                            }
                        }

                        if sent_frame {
                            last_sent_tick = stream_tick;
                        } else {
                            path_label = "vp8-wait";
                        }

                        let sleep_duration = if is_active {
                            stream_profile.active_frame_delay()
                        } else {
                            stream_profile.idle_frame_delay()
                        };
                        perf_log_frame_index = perf_log_frame_index.saturating_add(1);
                        if perf_log_frame_index <= 5 || perf_log_frame_index % 60 == 0 {
                            logging::append_log(
                                "INFO",
                                "media.perf",
                                format!(
                                    "session_id={} path={} profile={} active={} frame={} capture_ms={} encode_ms={} send_ms={} packets={} total_ms={} sleep_ms={}",
                                    session_id,
                                    path_label,
                                    stream_profile.wire_name(),
                                    is_active,
                                    perf_log_frame_index,
                                    capture_elapsed.as_millis(),
                                    encode_elapsed.as_millis(),
                                    send_elapsed.as_millis(),
                                    packets_produced,
                                    loop_started_at.elapsed().as_millis(),
                                    sleep_duration.as_millis(),
                                ),
                            );
                        }

                        thread::sleep(sleep_duration);
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

fn frame_signature(bytes: &[u8]) -> Vec<u8> {
    let desired = 96usize;
    let step = (bytes.len() / desired.max(1)).max(1);
    bytes.iter().step_by(step).take(desired).copied().collect()
}

fn signature_distance(previous: &[u8], next: &[u8]) -> u32 {
    previous
        .iter()
        .zip(next.iter())
        .map(|(left, right)| left.abs_diff(*right) as u32)
        .sum::<u32>()
        / previous.len().max(1) as u32
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
