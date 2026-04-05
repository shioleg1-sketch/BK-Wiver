use std::{
    collections::VecDeque,
    env,
    fs,
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

#[cfg(feature = "in-process-encoding")]
use ffmpeg_next as ffmpeg;
#[cfg(feature = "in-process-encoding")]
use ffmpeg_next::codec::{encoder, Id};
#[cfg(feature = "in-process-encoding")]
use ffmpeg_next::codec::Context;
#[cfg(feature = "in-process-encoding")]
use ffmpeg_next::format::Pixel;
#[cfg(feature = "in-process-encoding")]
use ffmpeg_next::Dictionary;

use serde_json::json;
use tungstenite::{Message, connect};
use url::Url;

use crossbeam_channel::{Receiver as CrossbeamReceiver, bounded, TrySendError};

use crate::{capture::CaptureEngine, logging};

// Improvement 6: precise frame timing
use spin_sleep::sleep;
// Improvement 3: screen change detection
use twox_hash::XxHash64;
use std::hash::Hasher;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const MEDIA_PACKET_MAGIC: &[u8; 4] = b"BKWM";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const IVF_HEADER_LEN: usize = 32;
const IVF_FRAME_HEADER_LEN: usize = 12;
const VP8_FRAME_CHUNK_MAGIC: &[u8; 4] = b"BKWC";
const VP8_FRAME_CHUNK_HEADER_LEN: usize = 16;
const VP8_FRAME_CHUNK_DATA_LEN: usize = 4096;

// Оптимизированные настройки кодеков
const H264_BITRATE: u64 = 5_000_000;
const VP8_BITRATE: u64 = 3_000_000;
const VP8_FPS: u32 = 30;
const VP8_KEYFRAME_INTERVAL: u32 = 50;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamCodec {
    Auto,
    H264,
    Vp8,
}

impl StreamCodec {
    pub fn from_wire(value: &str) -> Self {
        match value {
            "vp8" => Self::Vp8,
            "h264" => Self::H264,
            "auto" => Self::Auto,
            _ => Self::Auto,
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::Auto => 2,
            Self::H264 => 2,
            Self::Vp8 => 1,
        }
    }

    fn wire_name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::H264 => "h264",
            Self::Vp8 => "vp8",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HardwareEncoder {
    Nvenc,
    Qsv,
    Amf,
    VideoToolbox,
    Libx264,
}

impl HardwareEncoder {
    fn codec_name(self) -> &'static str {
        match self {
            Self::Nvenc => "h264_nvenc",
            Self::Qsv => "h264_qsv",
            Self::Amf => "h264_amf",
            Self::VideoToolbox => "h264_videotoolbox",
            Self::Libx264 => "libx264",
        }
    }
}

fn resolve_auto_codec(best_h264_encoder: HardwareEncoder) -> StreamCodec {
    let _ = best_h264_encoder;
    StreamCodec::H264
}

pub fn detect_best_h264_encoder() -> HardwareEncoder {
    #[cfg(target_os = "windows")]
    {
        let candidates = [HardwareEncoder::Nvenc, HardwareEncoder::Qsv, HardwareEncoder::Amf];
        for encoder in candidates {
            let output = Command::new(ffmpeg_executable_path())
                .args(["-f", "lavfi", "-i", "color=c=black:s=2x2:r=1:d=1",
                       "-t", "0.04", "-frames:v", "1",
                       "-c:v", encoder.codec_name(), "-f", "null", "-"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .stdin(Stdio::null())
                .output();
            if let Ok(output) = output
                && output.status.success()
            {
                logging::append_log(
                    "INFO",
                    "media.encoder_detect",
                    format!("found hardware encoder: {}", encoder.codec_name()),
                );
                return encoder;
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let output = Command::new(ffmpeg_executable_path())
            .args(["-f", "lavfi", "-i", "color=c=black:s=2x2:r=1:d=1",
                   "-t", "0.04", "-frames:v", "1",
                   "-c:v", "h264_videotoolbox", "-f", "null", "-"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .output();
        if let Ok(output) = output
            && output.status.success()
        {
            logging::append_log(
                "INFO",
                "media.encoder_detect",
                "found hardware encoder: h264_videotoolbox",
            );
            return HardwareEncoder::VideoToolbox;
        }
    }

    logging::append_log("INFO", "media.encoder_detect", "no HW encoder, using libx264");
    HardwareEncoder::Libx264
}

struct RawFrame {
    data: Vec<u8>,
    width: u32,
    height: u32,
    backend: &'static str,
    capture_ms: u128,
    captured_at: Instant,
    is_i_frame: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CaptureRuntimeStatus {
    state: String,
    backend: String,
    profile: String,
    updated_at_ms: u64,
    width: u32,
    height: u32,
    message: String,
}

#[derive(Default)]
struct CaptureTelemetry {
    samples_ms: VecDeque<u128>,
}

impl CaptureTelemetry {
    fn push(&mut self, capture_ms: u128) -> u128 {
        const MAX_SAMPLES: usize = 12;

        self.samples_ms.push_back(capture_ms);
        while self.samples_ms.len() > MAX_SAMPLES {
            let _ = self.samples_ms.pop_front();
        }

        self.average_ms()
    }

    fn average_ms(&self) -> u128 {
        if self.samples_ms.is_empty() {
            return 0;
        }

        self.samples_ms.iter().copied().sum::<u128>() / self.samples_ms.len() as u128
    }
}

/// Improvement 3: Compute a fast perceptual hash of a frame for change detection.
fn compute_frame_hash(data: &[u8]) -> u64 {
    let step = data.len().max(1) / 4096;
    let mut hasher = XxHash64::with_seed(0);
    for i in (0..data.len()).step_by(step.max(1)) {
        hasher.write_u8(data[i]);
    }
    hasher.finish()
}

fn app_state_dir() -> PathBuf {
    let local_app_data = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    local_app_data.join("BK-Wiver").join("state")
}

fn save_capture_status(status: &CaptureRuntimeStatus) {
    let state_dir = app_state_dir();
    let _ = fs::create_dir_all(&state_dir);
    let path = state_dir.join("capture-status.json");
    if let Ok(body) = serde_json::to_string_pretty(status) {
        let _ = fs::write(path, body);
    }
}

fn capture_state_for_backend(backend: &str) -> (&'static str, String) {
    if backend.contains("virtual-display-pending") {
        (
            "virtual_display_pending",
            "Для быстрого захвата требуется виртуальный дисплей.".to_owned(),
        )
    } else if backend.contains("screenshots") {
        (
            "degraded",
            "Используется программный захват экрана, качество и FPS могут быть ниже.".to_owned(),
        )
    } else if backend.contains("dxgi-unavailable") {
        (
            "capture_unavailable",
            "DXGI недоступен для текущей сессии или адаптера.".to_owned(),
        )
    } else {
        ("ready", "Захват работает.".to_owned())
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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
            Self::Sharp => (1920, 1080),
        }
    }

    fn capture_dimensions_for_backend(self, backend_hint: Option<&str>) -> (u32, u32) {
        let is_remote_backend = backend_hint
            .map(|backend| {
                backend.starts_with("windows-rdp-") || backend.starts_with("windows-headless-")
            })
            .unwrap_or(false);
        let is_wgc_backend = backend_hint
            .map(|backend| backend.contains("-wgc"))
            .unwrap_or(false);

        match self {
            Self::Fast if is_remote_backend && is_wgc_backend => (854, 480),
            Self::Balanced if is_remote_backend && is_wgc_backend => (960, 540),
            Self::Sharp if is_remote_backend && is_wgc_backend => (1280, 720),
            Self::Sharp if is_remote_backend => (1600, 900),
            _ => self.max_dimensions(),
        }
    }

    fn adaptive_capture_dimensions(
        self,
        backend_hint: Option<&str>,
        recent_capture_ms: Option<u128>,
    ) -> (u32, u32) {
        let base = self.capture_dimensions_for_backend(backend_hint);
        let Some(backend) = backend_hint else {
            return base;
        };

        let is_slow_backend =
            backend.contains("-gdi") || backend.contains("screenshots") || backend.contains("-wgc");
        if !is_slow_backend {
            return base;
        }

        let Some(capture_ms) = recent_capture_ms else {
            return base;
        };

        match (self, capture_ms) {
            (Self::Sharp, 241..) => (960, 540),
            (Self::Sharp, 181..) => (1152, 648),
            (Self::Sharp, 121..) => (1280, 720),
            (Self::Sharp, 81..) => (1366, 768),
            (Self::Balanced, 241..) => (768, 432),
            (Self::Balanced, 181..) => (854, 480),
            (Self::Balanced, 121..) => (960, 540),
            (Self::Balanced, 81..) => (1024, 576),
            (Self::Fast, 241..) => (512, 288),
            (Self::Fast, 181..) => (640, 360),
            (Self::Fast, 121..) => (768, 432),
            _ => base,
        }
    }

    fn target_frame_interval(self) -> Duration {
        Duration::from_secs_f64(1.0 / self.target_fps() as f64)
    }

    fn target_fps(self) -> u32 {
        match self {
            Self::Fast => 30,
            Self::Balanced => 30,
            Self::Sharp => 30,
        }
    }

    fn target_crf(self) -> &'static str {
        match self {
            Self::Fast => "35",
            Self::Balanced => "31",
            Self::Sharp => "23",
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

fn bitrate_string(bits_per_second: u64) -> String {
    format!("{}k", (bits_per_second / 1000).max(1))
}

fn scaled_bitrate_for_dimensions(
    baseline_bits_per_second: u64,
    width: u32,
    height: u32,
    baseline_dimensions: (u32, u32),
) -> String {
    let baseline_pixels =
        u64::from(baseline_dimensions.0).saturating_mul(u64::from(baseline_dimensions.1)).max(1);
    let actual_pixels = u64::from(width).saturating_mul(u64::from(height)).max(1);
    let scaled = baseline_bits_per_second
        .saturating_mul(actual_pixels)
        .saturating_div(baseline_pixels)
        .max(900_000);
    bitrate_string(scaled)
}

fn h264_bitrate_for_profile(profile: StreamProfile, width: u32, height: u32) -> String {
    let baseline = match profile {
        StreamProfile::Fast => H264_BITRATE.saturating_mul(55).saturating_div(100),
        StreamProfile::Balanced => H264_BITRATE,
        StreamProfile::Sharp => H264_BITRATE.saturating_mul(12).saturating_div(10),
    };
    scaled_bitrate_for_dimensions(baseline, width, height, profile.max_dimensions())
}

fn vp8_bitrate_for_profile(profile: StreamProfile, width: u32, height: u32) -> String {
    let baseline = match profile {
        StreamProfile::Fast => VP8_BITRATE.saturating_mul(80).saturating_div(100),
        StreamProfile::Balanced => VP8_BITRATE,
        StreamProfile::Sharp => VP8_BITRATE.saturating_mul(150).saturating_div(100),
    };
    scaled_bitrate_for_dimensions(baseline, width, height, profile.max_dimensions())
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
        Self::new_hw(width, height, profile, HardwareEncoder::Libx264, "rgba")
    }

    fn new_hw(
        width: u32,
        height: u32,
        profile: StreamProfile,
        encoder: HardwareEncoder,
        input_pix_fmt: &str,
    ) -> Result<Self, String> {
        let ffmpeg = ffmpeg_executable_path();
        logging::append_log(
            "INFO",
            "media.h264_encoder",
            format!(
                "starting ffmpeg={} encoder={} input={} width={} height={} fps={}",
                ffmpeg.display(),
                encoder.codec_name(),
                input_pix_fmt,
                width,
                height,
                profile.target_fps()
            ),
        );

        let mut command = Command::new(ffmpeg);
        let target_bitrate = h264_bitrate_for_profile(profile, width, height);
        command
            .arg("-loglevel").arg("error")
            .arg("-f").arg("rawvideo")
            .arg("-pix_fmt").arg(input_pix_fmt)
            .arg("-s").arg(format!("{width}x{height}"))
            .arg("-r").arg(profile.target_fps().to_string())
            .arg("-i").arg("pipe:0")
            .arg("-an")
            .arg("-c:v").arg(encoder.codec_name());

        match encoder {
            HardwareEncoder::Nvenc => {
                command
                    .arg("-preset").arg("p1")
                    .arg("-tune").arg("ll")
                    .arg("-rc").arg("cbr")
                    .arg("-b:v").arg(&target_bitrate)
                    .arg("-maxrate").arg(&target_bitrate)
                    .arg("-bufsize").arg(&target_bitrate)
                    .arg("-bf").arg("0")
                    .arg("-g").arg(profile.target_fps().to_string());
            }
            HardwareEncoder::Qsv => {
                command
                    .arg("-preset").arg("veryfast")
                    .arg("-look_ahead").arg("0")
                    .arg("-bf").arg("0")
                    .arg("-g").arg(profile.target_fps().to_string());
            }
            HardwareEncoder::Amf => {
                command
                    .arg("-usage").arg("ultralowlatency")
                    .arg("-rc").arg("cbr")
                    .arg("-b:v").arg(&target_bitrate)
                    .arg("-maxrate").arg(&target_bitrate)
                    .arg("-bf").arg("0")
                    .arg("-g").arg(profile.target_fps().to_string());
            }
            HardwareEncoder::VideoToolbox => {
                command
                    .arg("-b:v").arg(&target_bitrate)
                    .arg("-realtime").arg("true")
                    .arg("-bf").arg("0")
                    .arg("-g").arg(profile.target_fps().to_string());
            }
            HardwareEncoder::Libx264 => {
                command
                    .arg("-preset").arg("ultrafast")
                    .arg("-tune").arg("zerolatency")
                    .arg("-profile:v").arg("baseline")
                    .arg("-pix_fmt").arg("yuv420p")
                    .arg("-bf").arg("0")
                    .arg("-refs").arg("1")
                    .arg("-crf").arg(profile.target_crf())
                    .arg("-maxrate").arg(&target_bitrate)
                    .arg("-bufsize").arg(&target_bitrate)
                    .arg("-g").arg(profile.target_fps().to_string())
                    .arg("-keyint_min").arg(profile.target_fps().to_string())
                    .arg("-x264-params")
                    .arg("scenecut=0:repeat-headers=1:bframes=0:sync-lookahead=0:rc-lookahead=0:sliced-threads=1");
            }
        }

        command
            .arg("-f").arg("h264")
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
            let mut buffer = [0_u8; 65536];
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

    fn push_raw(&mut self, data: &[u8]) -> Result<(), String> {
        self.stdin
            .write_all(data)
            .map_err(|error| error.to_string())?;
        self.stdin
            .flush()
            .map_err(|error| error.to_string())
    }

    fn push_frame(&mut self, image: &image::RgbaImage) -> Result<(), String> {
        self.push_raw(image.as_raw())
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
    fn new(
        width: u32,
        height: u32,
        profile: StreamProfile,
        input_pix_fmt: &str,
    ) -> Result<Self, String> {
        let ffmpeg = ffmpeg_executable_path();
        logging::append_log(
            "INFO",
            "media.vp8_encoder",
            format!(
                "starting ffmpeg={} encoder=libvpx input={} width={} height={} fps={}",
                ffmpeg.display(),
                input_pix_fmt,
                width,
                height,
                profile.target_fps()
            ),
        );

        let mut command = Command::new(ffmpeg);
        let target_bitrate = vp8_bitrate_for_profile(profile, width, height);
        command
            .arg("-loglevel")
            .arg("error")
            .arg("-f")
            .arg("rawvideo")
            .arg("-pix_fmt")
            .arg(input_pix_fmt)
            .arg("-s")
            .arg(format!("{width}x{height}"))
            .arg("-r")
            .arg(VP8_FPS.to_string())
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
            .arg("-row-mt")
            .arg("1")
            .arg("-tile-columns")
            .arg("1")
            .arg("-g")
            .arg(VP8_KEYFRAME_INTERVAL.to_string())
            .arg("-keyint_min")
            .arg(VP8_KEYFRAME_INTERVAL.to_string())
            .arg("-crf")
            .arg(profile.target_crf())
            .arg("-b:v")
            .arg(&target_bitrate)
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

    fn push_frame_from_raw(&mut self, data: &[u8], _width: u32, _height: u32) -> Result<(), String> {
        self.stdin
            .write_all(data)
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
        #[cfg(windows)]
        let input_pix_fmt = "bgra";
        #[cfg(not(windows))]
        let input_pix_fmt = "rgba";

        let best_encoder = detect_best_h264_encoder();
        let Ok(url) = media_url(&server_url, &token, &session_id) else {
            logging::append_log(
                "WARN",
                "media",
                format!("invalid media url session_id={}", session_id),
            );
            return;
        };

        let (frame_tx, frame_rx): (
            crossbeam_channel::Sender<RawFrame>,
            CrossbeamReceiver<RawFrame>,
        ) = bounded(4);
        let frame_drop_rx = frame_rx.clone();
        let stop_capture = stop_flag.clone();
        let capture_profile = profile.clone();
        let capture_session_id = session_id.clone();
        let encode_session_id = session_id.clone();

        let capture_handle = thread::spawn(move || {
            let mut frame_index = 0u32;
            let mut capture_engine = CaptureEngine::new();
            let mut dropped_stale_frames = 0u64;
            let mut last_backend_hint: Option<&'static str> = None;
            let mut capture_telemetry = CaptureTelemetry::default();
            let mut smoothed_capture_ms: Option<u128> = None;
            let mut last_logged_dimensions: Option<(StreamProfile, (u32, u32), Option<&'static str>)> = None;
            // Improvement 3: screen change detection
            let mut prev_frame_hash: Option<u64> = None;
            // Fix 3: Minimum frames between dimension changes to prevent oscillation
            let mut frames_since_dimension_change = 0u32;
            let mut last_capture_dimensions: Option<(u32, u32)> = None;
            const MIN_FRAMES_BETWEEN_DIMENSION_CHANGES: u32 = 60; // At least 2 seconds at 30fps

            while !stop_capture.load(Ordering::Relaxed) {
                let stream_profile = capture_profile
                    .lock()
                    .map(|guard| *guard)
                    .unwrap_or(StreamProfile::Balanced);

                // Fix 3: Enforce minimum frames between dimension changes
                frames_since_dimension_change += 1;
                let capture_dimensions = if frames_since_dimension_change < MIN_FRAMES_BETWEEN_DIMENSION_CHANGES {
                    // Use previous dimensions to avoid oscillation
                    last_capture_dimensions.unwrap_or(stream_profile.max_dimensions())
                } else {
                    let dims = stream_profile.adaptive_capture_dimensions(last_backend_hint, smoothed_capture_ms);
                    last_capture_dimensions = Some(dims);
                    frames_since_dimension_change = 0;
                    dims
                };
                if last_logged_dimensions
                    != Some((stream_profile, capture_dimensions, last_backend_hint))
                {
                    if capture_dimensions != stream_profile.max_dimensions() {
                        logging::append_log(
                            "INFO",
                            "media.profile",
                            format!(
                                "session_id={} profile={} backend_hint={} recent_capture_ms={} tuned_capture={}x{}",
                                capture_session_id,
                                stream_profile.wire_name(),
                                last_backend_hint.unwrap_or("unknown"),
                                smoothed_capture_ms
                                    .map(|value| value.to_string())
                                    .unwrap_or_else(|| "unknown".to_owned()),
                                capture_dimensions.0,
                                capture_dimensions.1
                            ),
                        );
                    }
                    last_logged_dimensions =
                        Some((stream_profile, capture_dimensions, last_backend_hint));
                }
                let capture_started = Instant::now();
                let captured = capture_engine.capture(capture_dimensions, frame_index);
                let capture_ms = capture_started.elapsed().as_millis();
                last_backend_hint = Some(captured.backend);
                smoothed_capture_ms = Some(capture_telemetry.push(capture_ms));

                if captured.used_fallback && frame_index % 60 == 1 {
                    logging::append_log(
                        "WARN",
                        "capture",
                        format!("fallback frame active backend={}", captured.backend),
                    );
                }

                let frame_image = captured.image;
                let (capture_state, capture_message) = capture_state_for_backend(captured.backend);
                save_capture_status(&CaptureRuntimeStatus {
                    state: capture_state.to_owned(),
                    backend: captured.backend.to_owned(),
                    profile: stream_profile.wire_name().to_owned(),
                    updated_at_ms: now_ms(),
                    width: frame_image.width(),
                    height: frame_image.height(),
                    message: capture_message,
                });

                // Improvement 3: skip unchanged frames (but force I-frame every 30)
                let current_hash = compute_frame_hash(frame_image.as_raw());
                let is_key = frame_index % 30 == 0;
                if prev_frame_hash == Some(current_hash) && !is_key {
                    prev_frame_hash = Some(current_hash);
                    frame_index = frame_index.wrapping_add(1);
                    let elapsed = capture_started.elapsed();
                    if let Some(remaining) = stream_profile.target_frame_interval().checked_sub(elapsed) {
                        sleep(remaining);
                    }
                    continue;
                }
                prev_frame_hash = Some(current_hash);

                let frame = RawFrame {
                    width: frame_image.width(),
                    height: frame_image.height(),
                    data: frame_image.into_raw(),
                    backend: captured.backend,
                    capture_ms,
                    captured_at: Instant::now(),
                    is_i_frame: is_key,
                };

                match frame_tx.try_send(frame) {
                    Ok(()) => {}
                    Err(TrySendError::Full(frame)) => {
                        // Improvement 5: prioritize I-frames, drop stale frames
                        if frame.is_i_frame {
                            while frame_drop_rx.try_recv().is_ok() {}
                            if frame_tx.try_send(frame).is_err() {}
                        }
                        dropped_stale_frames = dropped_stale_frames.saturating_add(1);
                        if dropped_stale_frames == 1 || dropped_stale_frames % 120 == 0 {
                            logging::append_log(
                                "INFO",
                                "media.capture",
                                format!(
                                    "dropped_stale_frames={} strategy=i_frame_priority",
                                    dropped_stale_frames
                                ),
                            );
                        }
                    }
                    Err(TrySendError::Disconnected(_)) => break,
                }

                if frame_index < 5 || frame_index % 60 == 0 {
                    logging::append_log(
                        "INFO",
                        "media.capture_perf",
                        format!(
                            "session_id={} backend={} profile={} frame={} capture_ms={}",
                            capture_session_id,
                            captured.backend,
                            stream_profile.wire_name(),
                            frame_index.saturating_add(1),
                            smoothed_capture_ms.unwrap_or(capture_ms)
                        ),
                    );
                }

                frame_index = frame_index.wrapping_add(1);

                let elapsed = capture_started.elapsed();
                if let Some(remaining) = stream_profile.target_frame_interval().checked_sub(elapsed)
                {
                    sleep(remaining);
                }
            }
        });

        let encode_handle = thread::spawn(move || {
            while !stop_flag.load(Ordering::Relaxed) {
                match connect(url.as_str()) {
                    Ok((mut socket, _)) => {
                        let mut h264_encoder: Option<H264EncoderSession> = None;
                        let mut h264_config_sent = false;
                        let mut vp8_encoder: Option<Vp8EncoderSession> = None;
                        let mut vp8_config_sent = false;
                        let mut vp8_header_buffer = Vec::new();
                        let mut vp8_chunks_sent = 0u64;
                        let mut h264_packets_sent = 0u64;
                        let mut sent_frames = 0u64;
                        let mut active_codec: Option<StreamCodec> = None;
                        let mut auto_codec_override: Option<StreamCodec> = None;
                        let mut auto_retry_h264_after: Option<Instant> = None;
                        let stream_start = Instant::now();

                        // Fix 1: Debounce dimension changes — only restart encoder if
                        // dimensions have been stable for DIMENSION_DEBOUNCE_FRAMES frames.
                        let mut pending_dimensions: Option<(u32, u32)> = None;
                        let mut pending_dimensions_frame_count = 0u32;
                        const DIMENSION_DEBOUNCE_FRAMES: u32 = 30; // ~1s at 30fps

                        // Fix 2: Encoder warmup — drop first N frames after restart.
                        let mut encoder_warmup_frames = 0u32;
                        const ENCODER_WARMUP_FRAMES: u32 = 5;

                        logging::append_log(
                            "INFO",
                            "media",
                            format!(
                                "encoder thread connected session_id={} encoder={}",
                                encode_session_id,
                                best_encoder.codec_name()
                            ),
                        );

                        while !stop_flag.load(Ordering::Relaxed) {
                            let frame = match frame_rx.recv_timeout(Duration::from_millis(500)) {
                                Ok(f) => f,
                                Err(_) => continue,
                            };

                            // Compute PTS in microseconds since stream start
                            let pts_us = frame.captured_at
                                .duration_since(stream_start)
                                .as_micros() as i64;
                            let is_i_frame = frame.is_i_frame;

                            let stream_profile = profile
                                .lock()
                                .map(|guard| *guard)
                                .unwrap_or(StreamProfile::Balanced);
                            let preferred_codec = codec_preference
                                .lock()
                                .map(|guard| *guard)
                                .unwrap_or(StreamCodec::Auto);
                            if preferred_codec != StreamCodec::Auto {
                                auto_codec_override = None;
                                auto_retry_h264_after = None;
                            }

                            let selected_codec = match preferred_codec {
                                StreamCodec::Auto => {
                                    if auto_codec_override == Some(StreamCodec::Vp8)
                                        && auto_retry_h264_after
                                            .map(|deadline| Instant::now() < deadline)
                                            .unwrap_or(false)
                                    {
                                        StreamCodec::Vp8
                                    } else {
                                        if auto_codec_override.is_some() {
                                            logging::append_log(
                                                "INFO",
                                                "media.codec_auto",
                                                format!(
                                                    "session_id={} retrying codec=h264 after cooldown",
                                                    encode_session_id
                                                ),
                                            );
                                        }
                                        auto_codec_override = None;
                                        resolve_auto_codec(best_encoder)
                                    }
                                }
                                other => other,
                            };

                            if active_codec != Some(selected_codec) {
                                logging::append_log(
                                    "INFO",
                                    "media.codec_switch",
                                    format!(
                                        "session_id={} requested={} active={}",
                                        session_id,
                                        preferred_codec.wire_name(),
                                        selected_codec.wire_name()
                                    ),
                                );
                                match selected_codec {
                                    StreamCodec::H264 => {
                                        vp8_encoder = None;
                                        vp8_config_sent = false;
                                        vp8_header_buffer.clear();
                                    }
                                    StreamCodec::Vp8 => {
                                        h264_encoder = None;
                                        h264_config_sent = false;
                                    }
                                    StreamCodec::Auto => {}
                                }
                                active_codec = Some(selected_codec);
                            }

                            let encode_started = Instant::now();
                            let queue_ms = frame.captured_at.elapsed().as_millis();

                            match selected_codec {
                                StreamCodec::H264 => {
                                    let mut sent_packets_this_frame = 0u64;
                                    let mut sent_bytes_this_frame = 0usize;

                                    // Fix 1: Debounce dimension changes — but don't skip frames entirely
                                    // Only restart encoder if dimensions changed, otherwise reuse
                                    let frame_dims = (frame.width, frame.height);
                                    let encoder_needs_restart = h264_encoder
                                        .as_ref()
                                        .map(|e| !e.matches(frame.width, frame.height))
                                        .unwrap_or(true);

                                    if encoder_needs_restart && pending_dimensions != Some(frame_dims) {
                                        pending_dimensions = Some(frame_dims);
                                        pending_dimensions_frame_count = 0;
                                    }
                                    pending_dimensions_frame_count += 1;

                                    // Only skip if we're about to restart encoder AND haven't stabilized
                                    if encoder_needs_restart && pending_dimensions_frame_count < 3 {
                                        // Skip max 2 frames during encoder restart
                                        continue;
                                    }

                                    if let Err(error) = ensure_h264_encoder_hw(
                                        &mut h264_encoder,
                                        frame.width,
                                        frame.height,
                                        stream_profile,
                                        best_encoder,
                                        input_pix_fmt,
                                    ) {
                                        logging::append_log(
                                            "WARN",
                                            "media.h264_encoder",
                                            format!("failed to start encoder: {}", error),
                                        );
                                        h264_encoder = None;
                                        h264_config_sent = false;
                                        if preferred_codec == StreamCodec::Auto {
                                            let fallback = resolve_auto_codec(best_encoder);
                                            if fallback != StreamCodec::H264 {
                                                auto_codec_override = Some(fallback);
                                                auto_retry_h264_after =
                                                    Some(Instant::now() + Duration::from_secs(10));
                                                active_codec = None;
                                                logging::append_log(
                                                    "WARN",
                                                    "media.codec_auto",
                                                    format!(
                                                        "session_id={} fallback={} reason=h264_start_failed",
                                                        encode_session_id,
                                                        fallback.wire_name()
                                                    ),
                                                );
                                                continue;
                                            }
                                        }
                                    } else {
                                        // Fix 2: Mark warmup period after encoder (re)start
                                        encoder_warmup_frames = ENCODER_WARMUP_FRAMES;

                                        if let Some(encoder) = &mut h264_encoder {
                                            // Fix 2: Drop warmup frames — encoder may produce 0 packets initially
                                            if encoder_warmup_frames > 0 {
                                                encoder_warmup_frames -= 1;
                                                continue;
                                            }

                                            if encoder.push_raw(&frame.data).is_ok() {
                                                let packets = encoder.drain_packets();
                                                if !packets.is_empty() && !h264_config_sent {
                                                    let (width, height) = encoder.dimensions();
                                                    let config = json!({
                                                        "width": width,
                                                        "height": height,
                                                    });
                                                    let payload =
                                                        serde_json::to_vec(&config).unwrap_or_default();
                                                    if payload.is_empty() {
                                                        break;
                                                    }
                                                    let packet = encode_media_packet(
                                                        StreamCodec::H264,
                                                        MediaPacketKind::Config,
                                                        &payload,
                                                        0,
                                                        false,
                                                    );
                                                    if socket
                                                        .send(Message::Binary(packet.into()))
                                                        .is_err()
                                                    {
                                                        break;
                                                    }
                                                    h264_config_sent = true;
                                                    logging::append_log(
                                                        "INFO",
                                                        "media.h264_encoder",
                                                        format!(
                                                            "config sent encoder={} width={} height={}",
                                                            best_encoder.codec_name(),
                                                            width,
                                                            height
                                                        ),
                                                    );
                                                }

                                                let payload_len =
                                                    packets.iter().map(Vec::len).sum::<usize>();
                                                let mut payload = Vec::with_capacity(payload_len);
                                                for packet in packets {
                                                    payload.extend_from_slice(&packet);
                                                }

                                                if !payload.is_empty() {
                                                    let packet = encode_media_packet(
                                                        StreamCodec::H264,
                                                        MediaPacketKind::Frame,
                                                        &payload,
                                                        pts_us,
                                                        is_i_frame,
                                                    );
                                                    if socket
                                                        .send(Message::Binary(packet.into()))
                                                        .is_err()
                                                    {
                                                        break;
                                                    }
                                                    sent_packets_this_frame =
                                                        sent_packets_this_frame.saturating_add(1);
                                                    sent_bytes_this_frame =
                                                        sent_bytes_this_frame.saturating_add(
                                                            payload_len,
                                                        );
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
                                                if preferred_codec == StreamCodec::Auto {
                                                    let fallback = resolve_auto_codec(best_encoder);
                                                    if fallback != StreamCodec::H264 {
                                                        auto_codec_override = Some(fallback);
                                                        auto_retry_h264_after = Some(
                                                            Instant::now() + Duration::from_secs(10),
                                                        );
                                                        active_codec = None;
                                                        logging::append_log(
                                                            "WARN",
                                                            "media.codec_auto",
                                                            format!(
                                                                "session_id={} fallback={} reason=h264_stdin_failed",
                                                                encode_session_id,
                                                                fallback.wire_name()
                                                            ),
                                                        );
                                                        continue;
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    let encode_ms = encode_started.elapsed().as_millis();
                                    sent_frames = sent_frames.saturating_add(1);
                                    if sent_frames <= 5 || sent_frames % 60 == 0 {
                                        logging::append_log(
                                            "INFO",
                                            "media.perf",
                                            format!(
                                                "session_id={} codec=h264 encoder={} profile={} backend={} frame={} capture_ms={} queue_ms={} encode_ms={} packets={} bytes={}",
                                                encode_session_id,
                                                best_encoder.codec_name(),
                                                stream_profile.wire_name(),
                                                frame.backend,
                                                sent_frames,
                                                frame.capture_ms,
                                                queue_ms,
                                                encode_ms,
                                                sent_packets_this_frame,
                                                sent_bytes_this_frame
                                            ),
                                        );
                                    }
                                }
                                StreamCodec::Vp8 => {
                                    let mut sent_packets_this_frame = 0u64;
                                    let mut sent_bytes_this_frame = 0usize;
                                    match ensure_vp8_encoder(
                                        &mut vp8_encoder,
                                        frame.width,
                                        frame.height,
                                        stream_profile,
                                        input_pix_fmt,
                                    ) {
                                        Ok(()) => {
                                            if let Some(encoder) = &mut vp8_encoder {
                                                if encoder
                                                    .push_frame_from_raw(
                                                        &frame.data,
                                                        frame.width,
                                                        frame.height,
                                                    )
                                                    .is_ok()
                                                {
                                                    let chunks = encoder.drain_packets();
                                                    for chunk in chunks {
                                                        if !vp8_config_sent {
                                                            vp8_header_buffer
                                                                .extend_from_slice(&chunk);
                                                            if vp8_header_buffer.len()
                                                                < IVF_HEADER_LEN
                                                            {
                                                                continue;
                                                            }

                                                            let header =
                                                                vp8_header_buffer[..IVF_HEADER_LEN]
                                                                    .to_vec();
                                                            let config_packet = encode_media_packet(
                                                                StreamCodec::Vp8,
                                                                MediaPacketKind::Config,
                                                                &header,
                                                                0,
                                                                false,
                                                            );
                                                            if socket
                                                                .send(Message::Binary(
                                                                    config_packet.into(),
                                                                ))
                                                                .is_err()
                                                            {
                                                                break;
                                                            }
                                                            let (width, height) =
                                                                encoder.dimensions();
                                                            logging::append_log(
                                                                "INFO",
                                                                "media.vp8_encoder",
                                                                format!(
                                                                    "config sent encoder=libvpx width={} height={}",
                                                                    width, height
                                                                ),
                                                            );
                                                            vp8_config_sent = true;

                                                            if vp8_header_buffer.len()
                                                                > IVF_HEADER_LEN
                                                            {
                                                                let remainder = vp8_header_buffer
                                                                    [IVF_HEADER_LEN..]
                                                                    .to_vec();
                                                                if send_vp8_frame_chunks(
                                                                    &mut socket,
                                                                    &remainder,
                                                                    &mut vp8_chunks_sent,
                                                                    0,
                                                                    false,
                                                                )
                                                                .is_err()
                                                                {
                                                                    break;
                                                                }
                                                                sent_packets_this_frame =
                                                                    sent_packets_this_frame
                                                                        .saturating_add(1);
                                                                sent_bytes_this_frame =
                                                                    sent_bytes_this_frame
                                                                        .saturating_add(
                                                                            remainder.len(),
                                                                        );
                                                            }
                                                            vp8_header_buffer.clear();
                                                        } else if send_vp8_frame_chunks(
                                                            &mut socket,
                                                            &chunk,
                                                            &mut vp8_chunks_sent,
                                                            pts_us,
                                                            is_i_frame,
                                                        )
                                                        .is_err()
                                                        {
                                                            break;
                                                        } else {
                                                            sent_packets_this_frame =
                                                                sent_packets_this_frame
                                                                    .saturating_add(1);
                                                            sent_bytes_this_frame =
                                                                sent_bytes_this_frame
                                                                    .saturating_add(chunk.len());
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
                                            if preferred_codec == StreamCodec::Auto {
                                                auto_codec_override = Some(StreamCodec::H264);
                                                auto_retry_h264_after = None;
                                                active_codec = None;
                                                logging::append_log(
                                                    "WARN",
                                                    "media.codec_auto",
                                                    format!(
                                                        "session_id={} fallback=h264 reason=vp8_start_failed",
                                                        encode_session_id
                                                    ),
                                                );
                                                continue;
                                            }
                                        }
                                    }

                                    let encode_ms = encode_started.elapsed().as_millis();
                                    sent_frames = sent_frames.saturating_add(1);
                                    if sent_frames <= 5 || sent_frames % 60 == 0 {
                                        logging::append_log(
                                            "INFO",
                                            "media.perf",
                                            format!(
                                                "session_id={} codec=vp8 profile={} backend={} frame={} capture_ms={} queue_ms={} encode_ms={} packets={} bytes={}",
                                                encode_session_id,
                                                stream_profile.wire_name(),
                                                frame.backend,
                                                sent_frames,
                                                frame.capture_ms,
                                                queue_ms,
                                                encode_ms,
                                                sent_packets_this_frame,
                                                sent_bytes_this_frame
                                            ),
                                        );
                                    }
                                }
                                StreamCodec::Auto => {}
                            }
                        }

                        let _ = socket.close(None);
                    }
                    Err(_) => {
                        logging::append_log(
                            "WARN",
                            "media",
                            format!("connect failed session_id={}", session_id),
                        );
                        thread::sleep(Duration::from_secs(2));
                    }
                }
            }
        });

        let _ = capture_handle.join();
        let _ = encode_handle.join();
    });
}

fn ensure_h264_encoder_hw(
    encoder: &mut Option<H264EncoderSession>,
    width: u32,
    height: u32,
    profile: StreamProfile,
    hw_encoder: HardwareEncoder,
    input_pix_fmt: &str,
) -> Result<(), String> {
    let needs_restart = encoder
        .as_ref()
        .map(|active| !active.matches(width, height))
        .unwrap_or(true);

    if !needs_restart {
        return Ok(());
    }

    match H264EncoderSession::new_hw(width, height, profile, hw_encoder, input_pix_fmt) {
        Ok(session) => {
            *encoder = Some(session);
            Ok(())
        }
        Err(error) => Err(error),
    }
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
    input_pix_fmt: &str,
) -> Result<(), String> {
    let needs_restart = encoder
        .as_ref()
        .map(|active| !active.matches(width, height, profile))
        .unwrap_or(true);

    if !needs_restart {
        return Ok(());
    }

    match Vp8EncoderSession::new(width, height, profile, input_pix_fmt) {
        Ok(session) => {
            *encoder = Some(session);
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn encode_media_packet(
    codec: StreamCodec,
    kind: MediaPacketKind,
    payload: &[u8],
    pts_us: i64,
    is_i_frame: bool,
) -> Vec<u8> {
    const MEDIA_PACKET_VERSION: u8 = 2;
    let flags: u8 = if is_i_frame { 1 } else { 0 };
    let mut bytes = Vec::with_capacity(16 + payload.len());
    bytes.extend_from_slice(MEDIA_PACKET_MAGIC);
    bytes.push(MEDIA_PACKET_VERSION);
    bytes.push(codec.code());
    bytes.push(kind.code());
    bytes.push(flags);
    bytes.extend_from_slice(&pts_us.to_le_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

fn send_vp8_frame_chunks(
    socket: &mut tungstenite::WebSocket<
        tungstenite::stream::MaybeTlsStream<std::net::TcpStream>,
    >,
    frame_packet: &[u8],
    chunk_counter: &mut u64,
    pts_us: i64,
    is_i_frame: bool,
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

        let frame_packet = encode_media_packet(
            StreamCodec::Vp8,
            MediaPacketKind::Frame,
            &payload,
            pts_us,
            is_i_frame,
        );
        socket
            .send(Message::Binary(frame_packet.into()))
            .map_err(|_| ())?;
        *chunk_counter = chunk_counter.saturating_add(1);
        offset = end;
    }

    Ok(())
}

// ============================================================================
// Improvement 1: In-process H.264 encoding via ffmpeg-next
// ============================================================================
// This module provides in-process H.264 encoding using the ffmpeg-next crate,
// eliminating the overhead of spawning ffmpeg subprocesses. It is designed to
// eventually replace H264EncoderSession for lower latency and better resource
// utilization.
//
// Integration point: In the encode thread of spawn_stream(), the
// InProcessH264Encoder can be used instead of H264EncoderSession by calling
// push_frame() with BGRA/RGBA data and reading encoded packets via
// drain_packets()/take_packets().
// ============================================================================

#[cfg(feature = "in-process-encoding")]
/// In-process H.264 encoder using ffmpeg-next (no subprocess overhead).
///
/// This encoder accepts raw BGRA frames via `push_frame()`, converts them to
/// YUV420P internally via ffmpeg's swscale, and produces H.264 Annex-B packets
/// that can be sent directly over the wire.
#[allow(dead_code)]
pub struct InProcessH264Encoder {
    encoder: ffmpeg::encoder::Video,
    scaler: ffmpeg::software::scaling::Context,
    input_format: Pixel,
    packet_tx: std::sync::mpsc::Sender<Vec<u8>>,
    packet_rx: std::sync::mpsc::Receiver<Vec<u8>>,
    width: u32,
    height: u32,
    frame_count: i64,
}

#[cfg(feature = "in-process-encoding")]
#[allow(dead_code)]
impl InProcessH264Encoder {
    /// Create a new in-process H.264 encoder.
    ///
    /// - `width`/`height`: output dimensions
    /// - `fps`: target framerate
    /// - `bitrate`: target bitrate in bits per second
    /// - `input_format`: the pixel format of input frames (e.g. `Pixel::BGRA`)
    pub fn new(
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u64,
        input_format: Pixel,
    ) -> Result<Self, String> {
        ffmpeg::init().map_err(|e| format!("ffmpeg init failed: {e}"))?;

        let output_format = Pixel::YUV420P;

        // Find the H.264 encoder (try HW encoders first, fall back to libx264)
        let codec = encoder::find(Id::H264)
            .or_else(|| encoder::find_by_name("h264_nvenc"))
            .or_else(|| encoder::find_by_name("h264_videotoolbox"))
            .or_else(|| encoder::find_by_name("h264_qsv"))
            .ok_or_else(|| "no H.264 encoder found".to_owned())?;

        // Create encoder context
        let ctx = Context::new_with_codec(codec);
        let mut enc = ctx.encoder().video().map_err(|e| format!("encoder video: {e}"))?;

        enc.set_width(width);
        enc.set_height(height);
        enc.set_format(output_format);
        enc.set_bit_rate(bitrate as usize);
        enc.set_max_bit_rate(bitrate as usize);
        enc.set_frame_rate(Some((fps as i32, 1)));
        enc.set_time_base((1, fps as i32));
        enc.set_gop(fps);
        enc.set_max_b_frames(0);

        let mut opts = Dictionary::new();
        opts.set("preset", "ultrafast");
        opts.set("tune", "zerolatency");
        opts.set("profile", "baseline");

        let encoder = enc.open_with(opts).map_err(|e| format!("encoder open: {e}"))?;

        // Create scaler for input format -> YUV420P conversion
        let scaler = ffmpeg::software::converter(
            (width, height),
            input_format,
            output_format,
        )
        .map_err(|e| format!("scaler creation failed: {e}"))?;

        let (tx, rx) = std::sync::mpsc::channel();

        Ok(Self {
            encoder,
            scaler,
            input_format,
            packet_tx: tx,
            packet_rx: rx,
            width,
            height,
            frame_count: 0,
        })
    }

    /// Push a raw frame (in the input pixel format specified at construction) to the encoder.
    ///
    /// The `data` slice should contain `width * height * 4` bytes
    /// in the input pixel format (e.g. BGRA = 4 bytes per pixel).
    pub fn push_frame(&mut self, data: &[u8]) -> Result<(), String> {
        let expected_len = (self.width * self.height * 4) as usize;
        if data.len() != expected_len {
            return Err(format!(
                "frame size mismatch: expected {expected_len} bytes, got {} bytes",
                data.len()
            ));
        }

        // Create input frame and copy data
        let mut input_frame = ffmpeg::frame::Video::empty();
        input_frame.set_format(self.input_format);
        input_frame.set_width(self.width);
        input_frame.set_height(self.height);
        input_frame.set_pts(Some(self.frame_count));

        // Copy data into the frame, handling stride
        let dst_stride = input_frame.stride(0);
        let src_stride = (self.width * 4) as usize;
        let h = self.height as usize;

        for row in 0..h {
            let src_start = row * src_stride;
            let dst_start = row * dst_stride;
            let copy_len = src_stride.min(dst_stride);
            if src_start + copy_len <= data.len() && dst_start + copy_len <= input_frame.data_mut(0).len()
            {
                input_frame.data_mut(0)[dst_start..dst_start + copy_len]
                    .copy_from_slice(&data[src_start..src_start + copy_len]);
            }
        }

        // Convert to YUV420P
        let mut output_frame = ffmpeg::frame::Video::empty();
        self.scaler
            .run(&input_frame, &mut output_frame)
            .map_err(|e| format!("scaler failed: {e}"))?;
        output_frame.set_pts(Some(self.frame_count));

        // Send frame to encoder
        self.encoder
            .send_frame(&output_frame)
            .map_err(|e| format!("encoder send failed: {e}"))?;

        // Collect encoded packets
        self.collect_packets();

        self.frame_count += 1;
        Ok(())
    }

    /// Drain encoded packets from the encoder.
    pub fn drain_packets(&self) -> Vec<Vec<u8>> {
        let mut result = Vec::new();
        while let Ok(pkt) = self.packet_rx.try_recv() {
            result.push(pkt);
        }
        result
    }

    /// Take all currently buffered packets.
    pub fn take_packets(&mut self) -> Vec<Vec<u8>> {
        self.drain_packets()
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn matches(&self, width: u32, height: u32) -> bool {
        self.width == width && self.height == height
    }

    /// Flush the encoder (e.g. before shutdown or resolution change).
    /// Returns any remaining packets.
    pub fn flush(&mut self) -> Vec<Vec<u8>> {
        let _ = self.encoder.send_eof();
        self.collect_packets();
        self.drain_packets()
    }

    fn collect_packets(&mut self) {
        let mut packet = ffmpeg::Packet::empty();
        while self.encoder.receive_packet(&mut packet).is_ok() {
            let data = packet.data().map(|s| s.to_vec()).unwrap_or_default();
            let _ = self.packet_tx.send(data);
            packet = ffmpeg::Packet::empty();
        }
    }
}

#[cfg(feature = "in-process-encoding")]
impl Drop for InProcessH264Encoder {
    fn drop(&mut self) {
        let _ = self.encoder.send_eof();
        self.collect_packets();
    }
}

// ============================================================================
// Improvement 4: NV12/I420 optimized capture path
// ============================================================================
// DXGI on Windows can capture frames directly in NV12 format, avoiding the
// expensive BGRA->YUV conversion in the encoder pipeline. This helper provides
// an efficient BGRA->NV12 conversion for the current code path, and serves as
// the target format for future DXGI NV12 native capture support.
//
// NV12 layout: full-resolution Y plane followed by interleaved UV plane at
// half resolution (4:2:0 chroma subsampling).
// ============================================================================

/// Convert BGRA pixel data to NV12 format.
///
/// NV12 consists of:
/// - Y plane: width * height bytes (full resolution luma)
/// - UV plane: (width/2) * (height/2) * 2 bytes (interleaved chroma)
///
/// Total size: width * height * 3 / 2 bytes
///
/// This is a straightforward implementation suitable as a baseline. For
/// production use, consider SIMD-optimized versions (SSE2/AVX2 on x86,
/// NEON on ARM) or GPU-accelerated conversion.
#[allow(dead_code)]
pub fn convert_bgra_to_nv12(bgra: &[u8], width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let y_size = w * h;
    let uv_width = (w + 1) / 2;
    let uv_height = (h + 1) / 2;
    let uv_size = uv_width * uv_height * 2;
    let total_size = y_size + uv_size;

    let mut nv12 = vec![0u8; total_size];
    let (y_plane, uv_plane) = nv12.split_at_mut(y_size);

    // RGB to YUV conversion coefficients (BT.601 full range)
    // Y  =  0.257 * R + 0.504 * G + 0.098 * B + 16
    // U  = -0.148 * R - 0.291 * G + 0.439 * B + 128
    // V  =  0.439 * R - 0.368 * G - 0.071 * B + 128
    //
    // Using fixed-point arithmetic for performance (multiply by 256):
    // Y  = (66*R + 129*G +  25*B + 4096) >> 8
    // U  = (-38*R -  74*G + 112*B + 32768) >> 8
    // V  = (112*R -  94*G -  18*B + 32768) >> 8

    for y in 0..h {
        for x in 0..w {
            let src_idx = (y * w + x) * 4;
            let b = bgra[src_idx] as i32;
            let g = bgra[src_idx + 1] as i32;
            let r = bgra[src_idx + 2] as i32;
            // bgra[src_idx + 3] is alpha, ignored

            let y_val = ((66 * r + 129 * g + 25 * b + 4096) >> 8).clamp(0, 255) as u8;
            y_plane[y * w + x] = y_val;

            // UV are subsampled 2x2
            if x % 2 == 0 && y % 2 == 0 {
                let uv_x = x / 2;
                let uv_y = y / 2;
                let uv_idx = uv_y * uv_width * 2 + uv_x * 2;

                if uv_idx + 1 < uv_plane.len() {
                    // Average 2x2 block for chroma
                    let mut sum_u = 0i32;
                    let mut sum_v = 0i32;
                    let mut count = 0i32;

                    for dy in 0..2 {
                        for dx in 0..2 {
                            let px = x + dx;
                            let py = y + dy;
                            if px < w && py < h {
                                let pi = (py * w + px) * 4;
                                let pb = bgra[pi] as i32;
                                let pg = bgra[pi + 1] as i32;
                                let pr = bgra[pi + 2] as i32;
                                sum_u += -38 * pr - 74 * pg + 112 * pb + 32768;
                                sum_v += 112 * pr - 94 * pg - 18 * pb + 32768;
                                count += 1;
                            }
                        }
                    }

                    let u_val = ((sum_u / count) >> 8).clamp(0, 255) as u8;
                    let v_val = ((sum_v / count) >> 8).clamp(0, 255) as u8;
                    uv_plane[uv_idx] = u_val;
                    uv_plane[uv_idx + 1] = v_val;
                }
            }
        }
    }

    nv12
}

/// Convert BGRA to I420 (planar YUV420) format.
///
/// I420 consists of three separate planes:
/// - Y plane: width * height
/// - U plane: (width/2) * (height/2)
/// - V plane: (width/2) * (height/2)
///
/// Total size: width * height * 3 / 2 bytes
///
/// I420 is the standard input format for most software H.264 encoders
/// (libx264, x264, etc.).
#[allow(dead_code)]
pub fn convert_bgra_to_i420(bgra: &[u8], width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let y_size = w * h;
    let uv_width = (w + 1) / 2;
    let uv_height = (h + 1) / 2;
    let uv_size = uv_width * uv_height;
    let total_size = y_size + uv_size * 2;

    let mut i420 = vec![0u8; total_size];
    let (y_plane, rest) = i420.split_at_mut(y_size);
    let (u_plane, v_plane) = rest.split_at_mut(uv_size);

    for y in 0..h {
        for x in 0..w {
            let src_idx = (y * w + x) * 4;
            let b = bgra[src_idx] as i32;
            let g = bgra[src_idx + 1] as i32;
            let r = bgra[src_idx + 2] as i32;

            let y_val = ((66 * r + 129 * g + 25 * b + 4096) >> 8).clamp(0, 255) as u8;
            y_plane[y * w + x] = y_val;

            if x % 2 == 0 && y % 2 == 0 {
                let uv_x = x / 2;
                let uv_y = y / 2;
                let uv_idx = uv_y * uv_width + uv_x;

                if uv_idx < u_plane.len() && uv_idx < v_plane.len() {
                    let mut sum_u = 0i32;
                    let mut sum_v = 0i32;
                    let mut count = 0i32;

                    for dy in 0..2 {
                        for dx in 0..2 {
                            let px = x + dx;
                            let py = y + dy;
                            if px < w && py < h {
                                let pi = (py * w + px) * 4;
                                let pb = bgra[pi] as i32;
                                let pg = bgra[pi + 1] as i32;
                                let pr = bgra[pi + 2] as i32;
                                sum_u += -38 * pr - 74 * pg + 112 * pb + 32768;
                                sum_v += 112 * pr - 94 * pg - 18 * pb + 32768;
                                count += 1;
                            }
                        }
                    }

                    u_plane[uv_idx] = ((sum_u / count) >> 8).clamp(0, 255) as u8;
                    v_plane[uv_idx] = ((sum_v / count) >> 8).clamp(0, 255) as u8;
                }
            }
        }
    }

    i420
}
