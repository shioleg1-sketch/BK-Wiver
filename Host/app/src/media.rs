use std::{
    env,
    io::{Cursor, Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Duration,
};

use image::{ColorType, ImageEncoder, RgbaImage, codecs::jpeg::JpegEncoder};
use serde_json::json;
use tungstenite::{Message, connect};
use url::Url;

use crate::{capture::CaptureEngine, logging};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const MEDIA_PACKET_MAGIC: &[u8; 4] = b"BKWM";
const MEDIA_PACKET_VERSION: u8 = 1;
const IDLE_REFRESH_INTERVAL_TICKS: u64 = 30;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamCodec {
    Auto,
    Jpeg,
    H264,
}

impl StreamCodec {
    pub fn from_wire(value: &str) -> Self {
        match value {
            "jpeg" => Self::Jpeg,
            "h264" => Self::H264,
            _ => Self::Auto,
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::Auto | Self::Jpeg => 1,
            Self::H264 => 2,
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

    fn jpeg_quality(self) -> u8 {
        match self {
            Self::Fast => 65,
            Self::Balanced => 80,
            Self::Sharp => 90,
        }
    }

    fn active_frame_delay(self) -> Duration {
        match self {
            Self::Fast => Duration::from_millis(28),
            Self::Balanced => Duration::from_millis(34),
            Self::Sharp => Duration::from_millis(48),
        }
    }

    fn idle_frame_delay(self) -> Duration {
        match self {
            Self::Fast => Duration::from_millis(90),
            Self::Balanced => Duration::from_millis(120),
            Self::Sharp => Duration::from_millis(160),
        }
    }

    fn target_fps(self) -> u32 {
        match self {
            Self::Fast => 36,
            Self::Balanced => 30,
            Self::Sharp => 22,
        }
    }

    fn target_crf(self) -> &'static str {
        match self {
            Self::Fast => "29",
            Self::Balanced => "26",
            Self::Sharp => "23",
        }
    }

    fn target_maxrate(self) -> &'static str {
        match self {
            Self::Fast => "1800k",
            Self::Balanced => "3000k",
            Self::Sharp => "4500k",
        }
    }

    fn target_bufsize(self) -> &'static str {
        match self {
            Self::Fast => "900k",
            Self::Balanced => "1500k",
            Self::Sharp => "2200k",
        }
    }
}

struct H264EncoderSession {
    child: Child,
    stdin: ChildStdin,
    packet_rx: Receiver<Vec<u8>>,
    width: u32,
    height: u32,
    flavor: H264EncoderFlavor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum H264EncoderFlavor {
    #[cfg(windows)]
    Nvenc,
    #[cfg(windows)]
    Qsv,
    #[cfg(windows)]
    Amf,
    #[cfg(target_os = "macos")]
    VideoToolbox,
    Libx264,
}

impl H264EncoderFlavor {
    fn label(self) -> &'static str {
        match self {
            #[cfg(windows)]
            Self::Nvenc => "h264_nvenc",
            #[cfg(windows)]
            Self::Qsv => "h264_qsv",
            #[cfg(windows)]
            Self::Amf => "h264_amf",
            #[cfg(target_os = "macos")]
            Self::VideoToolbox => "h264_videotoolbox",
            Self::Libx264 => "libx264",
        }
    }

    fn append_ffmpeg_args(self, command: &mut Command, profile: StreamProfile) {
        match self {
            #[cfg(windows)]
            Self::Nvenc => {
                command
                    .arg("-c:v")
                    .arg("h264_nvenc")
                    .arg("-preset")
                    .arg("p1")
                    .arg("-tune")
                    .arg("ll")
                    .arg("-rc:v")
                    .arg("vbr")
                    .arg("-cq:v")
                    .arg(profile.target_crf())
                    .arg("-b:v")
                    .arg("0")
                    .arg("-maxrate")
                    .arg(profile.target_maxrate())
                    .arg("-bufsize")
                    .arg(profile.target_bufsize())
                    .arg("-profile:v")
                    .arg("baseline")
                    .arg("-pix_fmt")
                    .arg("yuv420p")
                    .arg("-g")
                    .arg(profile.target_fps().to_string())
                    .arg("-bf")
                    .arg("0");
            }
            #[cfg(windows)]
            Self::Qsv => {
                command
                    .arg("-c:v")
                    .arg("h264_qsv")
                    .arg("-preset")
                    .arg("veryfast")
                    .arg("-look_ahead")
                    .arg("0")
                    .arg("-maxrate")
                    .arg(profile.target_maxrate())
                    .arg("-bufsize")
                    .arg(profile.target_bufsize())
                    .arg("-profile:v")
                    .arg("baseline")
                    .arg("-pix_fmt")
                    .arg("nv12")
                    .arg("-g")
                    .arg(profile.target_fps().to_string())
                    .arg("-bf")
                    .arg("0");
            }
            #[cfg(windows)]
            Self::Amf => {
                command
                    .arg("-c:v")
                    .arg("h264_amf")
                    .arg("-usage")
                    .arg("ultralowlatency")
                    .arg("-quality")
                    .arg("speed")
                    .arg("-maxrate")
                    .arg(profile.target_maxrate())
                    .arg("-bufsize")
                    .arg(profile.target_bufsize())
                    .arg("-profile:v")
                    .arg("baseline")
                    .arg("-pix_fmt")
                    .arg("nv12")
                    .arg("-g")
                    .arg(profile.target_fps().to_string())
                    .arg("-bf")
                    .arg("0");
            }
            #[cfg(target_os = "macos")]
            Self::VideoToolbox => {
                command
                    .arg("-c:v")
                    .arg("h264_videotoolbox")
                    .arg("-realtime")
                    .arg("1")
                    .arg("-allow_sw")
                    .arg("1")
                    .arg("-maxrate")
                    .arg(profile.target_maxrate())
                    .arg("-bufsize")
                    .arg(profile.target_bufsize())
                    .arg("-profile:v")
                    .arg("baseline")
                    .arg("-pix_fmt")
                    .arg("yuv420p")
                    .arg("-g")
                    .arg(profile.target_fps().to_string())
                    .arg("-bf")
                    .arg("0");
            }
            Self::Libx264 => {
                command
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
                    .arg(profile.target_maxrate())
                    .arg("-bufsize")
                    .arg(profile.target_bufsize())
                    .arg("-g")
                    .arg(profile.target_fps().to_string())
                    .arg("-keyint_min")
                    .arg(profile.target_fps().to_string())
                    .arg("-x264-params")
                    .arg("scenecut=0:repeat-headers=1:bframes=0:sync-lookahead=0:rc-lookahead=0:sliced-threads=1");
            }
        }
    }
}

fn h264_encoder_candidates() -> Vec<H264EncoderFlavor> {
    let mut candidates = vec![H264EncoderFlavor::Libx264];
    #[cfg(windows)]
    {
        candidates.push(H264EncoderFlavor::Nvenc);
        candidates.push(H264EncoderFlavor::Qsv);
        candidates.push(H264EncoderFlavor::Amf);
    }
    #[cfg(target_os = "macos")]
    {
        candidates.push(H264EncoderFlavor::VideoToolbox);
    }
    candidates
}

impl H264EncoderSession {
    fn new(
        width: u32,
        height: u32,
        profile: StreamProfile,
        flavor: H264EncoderFlavor,
    ) -> Result<Self, String> {
        let ffmpeg = ffmpeg_executable_path();
        logging::append_log(
            "INFO",
            "media.h264_encoder",
            format!(
                "starting ffmpeg={} encoder={} width={} height={} fps={}",
                ffmpeg.display(),
                flavor.label(),
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
            .arg("-an");
        flavor.append_ffmpeg_args(&mut command, profile);
        command
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
            flavor,
        })
    }

    fn push_frame(&mut self, image: &RgbaImage) -> Result<(), String> {
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

    fn flavor(&self) -> H264EncoderFlavor {
        self.flavor
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
        let mut stream_tick = 0_u64;
        let mut capture_engine = CaptureEngine::new();
        let mut previous_signature: Option<Vec<u8>> = None;
        let mut h264_no_output_streak: u32;
        let mut h264_packets_sent = 0_u64;
        let mut h264_restart_allowed_at_tick = 0_u64;
        let mut last_sent_tick = 0_u64;
        while !stop_flag.load(Ordering::Relaxed) {
            match connect(url.as_str()) {
                Ok((mut socket, _)) => {
                    let mut h264_encoder: Option<H264EncoderSession> = None;
                    let mut h264_disabled_flavors: Vec<H264EncoderFlavor> = Vec::new();
                    let mut h264_config_sent = false;
                    h264_no_output_streak = 0;
                    while !stop_flag.load(Ordering::Relaxed) {
                        stream_tick = stream_tick.saturating_add(1);
                        let stream_profile =
                            profile.lock().map(|guard| *guard).unwrap_or(StreamProfile::Balanced);
                        let preferred_codec = codec_preference
                            .lock()
                            .map(|guard| *guard)
                            .unwrap_or(StreamCodec::Auto);

                        let captured = capture_engine.capture(stream_profile.max_dimensions(), frame_index);
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
                            .map(|previous| signature_distance(previous, &signature) > 4)
                            .unwrap_or(true);
                        previous_signature = Some(signature);
                        let should_refresh_idle =
                            stream_tick.saturating_sub(last_sent_tick) >= IDLE_REFRESH_INTERVAL_TICKS;

                        if !is_active && !should_refresh_idle {
                            thread::sleep(stream_profile.idle_frame_delay());
                            continue;
                        }

                        let mut sent_frame = false;
                        let should_try_h264 =
                            matches!(preferred_codec, StreamCodec::Auto | StreamCodec::H264)
                                && stream_tick >= h264_restart_allowed_at_tick;
                        if should_try_h264 {
                            match ensure_h264_encoder(
                                &mut h264_encoder,
                                &h264_disabled_flavors,
                                frame_image.width(),
                                frame_image.height(),
                                stream_profile,
                            ) {
                                Ok(()) => {
                                    if let Some(encoder) = &mut h264_encoder {
                                        if encoder.push_frame(&frame_image).is_ok() {
                                            let packets = encoder.drain_packets();
                                            if packets.is_empty() {
                                                h264_no_output_streak =
                                                    h264_no_output_streak.saturating_add(1);
                                                if h264_no_output_streak >= 3 {
                                                    logging::append_log(
                                                        "WARN",
                                                        "media.h264_encoder",
                                                        format!(
                                                            "no h264 output after {} frames on {}; switching to jpeg fallback",
                                                            h264_no_output_streak,
                                                            encoder.flavor().label()
                                                        ),
                                                    );
                                                    h264_disabled_flavors
                                                        .push(encoder.flavor());
                                                    h264_encoder = None;
                                                    h264_config_sent = false;
                                                    h264_restart_allowed_at_tick =
                                                        stream_tick.saturating_add(24);
                                                    h264_no_output_streak = 0;
                                                }
                                            } else {
                                                h264_no_output_streak = 0;
                                                if !h264_config_sent {
                                                    let (width, height) = encoder.dimensions();
                                                    logging::append_log(
                                                        "INFO",
                                                        "media.h264_encoder",
                                                        format!(
                                                            "config sent encoder={} width={} height={}",
                                                            encoder.flavor().label(),
                                                            width,
                                                            height
                                                        ),
                                                    );
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
                                                    if socket
                                                        .send(Message::Binary(packet.into()))
                                                        .is_err()
                                                    {
                                                        break;
                                                    }
                                                    h264_config_sent = true;
                                                }
                                            }
                                            for packet in packets {
                                                let packet = encode_media_packet(
                                                    StreamCodec::H264,
                                                    MediaPacketKind::Frame,
                                                    &packet,
                                                );
                                                if socket.send(Message::Binary(packet.into())).is_err()
                                                {
                                                    sent_frame = false;
                                                    break;
                                                }
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
                                                sent_frame = true;
                                            }
                                        } else {
                                            logging::append_log(
                                                "WARN",
                                                "media.h264_encoder",
                                                format!(
                                                    "ffmpeg stdin write failed on {}, falling back to jpeg",
                                                    encoder.flavor().label()
                                                ),
                                            );
                                            h264_disabled_flavors.push(encoder.flavor());
                                            h264_encoder = None;
                                            h264_config_sent = false;
                                            h264_restart_allowed_at_tick =
                                                stream_tick.saturating_add(24);
                                            h264_no_output_streak = 0;
                                        }
                                    }
                                }
                                Err(error) => {
                                    logging::append_log(
                                        "WARN",
                                        "media.h264_encoder",
                                        format!(
                                            "failed to start encoder, falling back to jpeg: {}",
                                            error
                                        ),
                                    );
                                    h264_encoder = None;
                                    h264_config_sent = false;
                                    h264_restart_allowed_at_tick =
                                        stream_tick.saturating_add(24);
                                    h264_no_output_streak = 0;
                                }
                            }
                        }

                        if !sent_frame {
                            let Ok(frame) = encode_jpeg(&frame_image, stream_profile.jpeg_quality())
                            else {
                                break;
                            };
                            let packet =
                                encode_media_packet(StreamCodec::Jpeg, MediaPacketKind::Frame, &frame);
                            if socket.send(Message::Binary(packet.into())).is_err() {
                                break;
                            }
                            sent_frame = true;
                        }

                        if sent_frame {
                            last_sent_tick = stream_tick;
                        }

                        thread::sleep(if is_active {
                            stream_profile.active_frame_delay()
                        } else {
                            stream_profile.idle_frame_delay()
                        });
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

fn encode_jpeg(image: &RgbaImage, quality: u8) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut cursor = Cursor::new(&mut bytes);
    let encoder = JpegEncoder::new_with_quality(&mut cursor, quality);
    encoder
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ColorType::Rgba8.into(),
        )
        .map_err(|error| error.to_string())?;
    Ok(bytes)
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
    if let Ok(current_exe) = env::current_exe()
        && let Some(parent) = current_exe.parent()
    {
        let bundled = parent.join("ffmpeg.exe");
        if bundled.exists() {
            return bundled;
        }
    }

    PathBuf::from("ffmpeg")
}

fn ensure_h264_encoder(
    encoder: &mut Option<H264EncoderSession>,
    disabled_flavors: &[H264EncoderFlavor],
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

    let mut last_error = None;
    for flavor in h264_encoder_candidates()
        .into_iter()
        .filter(|candidate| !disabled_flavors.contains(candidate))
    {
        match H264EncoderSession::new(width, height, profile, flavor) {
            Ok(session) => {
                *encoder = Some(session);
                return Ok(());
            }
            Err(error) => {
                logging::append_log(
                    "WARN",
                    "media.h264_encoder",
                    format!("failed to start encoder {}: {}", flavor.label(), error),
                );
                last_error = Some(format!("{}: {}", flavor.label(), error));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| "no available h264 encoders after fallback attempts".to_owned()))
}

fn configure_hidden_process(_command: &mut Command) {
    #[cfg(windows)]
    {
        _command.creation_flags(CREATE_NO_WINDOW);
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
