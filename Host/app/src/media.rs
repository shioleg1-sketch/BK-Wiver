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

use image::{
    ColorType, ImageBuffer, ImageEncoder, Rgba, RgbaImage, codecs::jpeg::JpegEncoder,
    imageops::FilterType,
};
use screenshots::Screen;
use serde_json::json;
use tungstenite::{Message, connect};
use url::Url;

use crate::logging;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const TEST_FRAME_WIDTH: u32 = 960;
const TEST_FRAME_HEIGHT: u32 = 540;
const MEDIA_PACKET_MAGIC: &[u8; 4] = b"BKWM";
const MEDIA_PACKET_VERSION: u8 = 1;
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
            Self::Fast => Duration::from_millis(55),
            Self::Balanced => Duration::from_millis(75),
            Self::Sharp => Duration::from_millis(95),
        }
    }

    fn idle_frame_delay(self) -> Duration {
        match self {
            Self::Fast => Duration::from_millis(140),
            Self::Balanced => Duration::from_millis(180),
            Self::Sharp => Duration::from_millis(230),
        }
    }

    fn target_fps(self) -> u32 {
        match self {
            Self::Fast => 24,
            Self::Balanced => 18,
            Self::Sharp => 14,
        }
    }
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
                "starting ffmpeg={} width={} height={} fps={}",
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
            .arg("veryfast")
            .arg("-tune")
            .arg("zerolatency")
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg("-g")
            .arg(profile.target_fps().to_string())
            .arg("-keyint_min")
            .arg(profile.target_fps().to_string())
            .arg("-x264-params")
            .arg("scenecut=0:repeat-headers=1")
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
        let mut active_screen = select_capture_screen();
        let mut previous_signature: Option<Vec<u8>> = None;
        while !stop_flag.load(Ordering::Relaxed) {
            match connect(url.as_str()) {
                Ok((mut socket, _)) => {
                    let mut h264_encoder: Option<H264EncoderSession> = None;
                    while !stop_flag.load(Ordering::Relaxed) {
                        let stream_profile =
                            profile.lock().map(|guard| *guard).unwrap_or(StreamProfile::Balanced);
                        let preferred_codec = codec_preference
                            .lock()
                            .map(|guard| *guard)
                            .unwrap_or(StreamCodec::Auto);

                        let frame_image = match active_screen.as_ref() {
                            Some(screen) => match capture_screen_image(screen, stream_profile) {
                                Ok(frame) => frame,
                                Err(_) => {
                                    active_screen = select_capture_screen();
                                    build_test_frame(frame_index)
                                }
                            },
                            None => {
                                active_screen = select_capture_screen();
                                build_test_frame(frame_index)
                            }
                        };
                        frame_index = frame_index.wrapping_add(1);

                        let signature = frame_signature(frame_image.as_raw());
                        let is_active = previous_signature
                            .as_ref()
                            .map(|previous| signature_distance(previous, &signature) > 18)
                            .unwrap_or(true);
                        previous_signature = Some(signature);

                        let mut sent_frame = false;
                        let should_try_h264 =
                            matches!(preferred_codec, StreamCodec::Auto | StreamCodec::H264);
                        if should_try_h264 {
                            match ensure_h264_encoder(
                                &mut h264_encoder,
                                &mut socket,
                                frame_image.width(),
                                frame_image.height(),
                                stream_profile,
                            ) {
                                Ok(()) => {
                                    if let Some(encoder) = &mut h264_encoder {
                                        if encoder.push_frame(&frame_image).is_ok() {
                                            for packet in encoder.drain_packets() {
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
                                                sent_frame = true;
                                            }
                                        } else {
                                            logging::append_log(
                                                "WARN",
                                                "media.h264_encoder",
                                                "ffmpeg stdin write failed, falling back to jpeg",
                                            );
                                            h264_encoder = None;
                                        }
                                    }
                                }
                                Err(_) => {
                                    logging::append_log(
                                        "WARN",
                                        "media.h264_encoder",
                                        "failed to start encoder, falling back to jpeg",
                                    );
                                    h264_encoder = None;
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

fn select_capture_screen() -> Option<Screen> {
    let screens = Screen::all().ok()?;
    screens
        .iter()
        .find(|screen| screen.display_info.is_primary)
        .copied()
        .or_else(|| screens.into_iter().next())
}

fn capture_screen_image(screen: &Screen, profile: StreamProfile) -> Result<RgbaImage, String> {
    let captured = screen.capture().map_err(|error| error.to_string())?;
    let width = captured.width();
    let height = captured.height();
    let raw = captured.into_raw();

    let image = ImageBuffer::from_raw(width, height, raw)
        .ok_or_else(|| "failed to build RGBA frame".to_owned())?;

    Ok(fit_frame(image, profile.max_dimensions()))
}

fn fit_frame(image: RgbaImage, max_dimensions: (u32, u32)) -> RgbaImage {
    let width = image.width();
    let height = image.height();
    if width <= max_dimensions.0 && height <= max_dimensions.1 {
        return image;
    }

    let scale = (max_dimensions.0 as f32 / width as f32)
        .min(max_dimensions.1 as f32 / height as f32);
    let resized_width = ((width as f32 * scale).round() as u32).max(1);
    let resized_height = ((height as f32 * scale).round() as u32).max(1);
    image::imageops::resize(&image, resized_width, resized_height, FilterType::Triangle)
}

fn build_test_frame(frame_index: u32) -> RgbaImage {
    let mut image: RgbaImage = ImageBuffer::new(TEST_FRAME_WIDTH, TEST_FRAME_HEIGHT);
    let shift = (frame_index * 13) % TEST_FRAME_WIDTH;
    let pulse = ((frame_index * 17) % 255) as u8;

    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let r = (((x + shift) % TEST_FRAME_WIDTH) * 255 / TEST_FRAME_WIDTH) as u8;
        let g = ((y * 255) / TEST_FRAME_HEIGHT) as u8;
        let mut b = pulse;

        if x > shift.saturating_sub(28) && x < (shift + 28).min(TEST_FRAME_WIDTH) {
            b = 240;
        }

        let border = x < 6 || y < 6 || x > TEST_FRAME_WIDTH - 7 || y > TEST_FRAME_HEIGHT - 7;
        *pixel = if border {
            Rgba([220, 226, 233, 255])
        } else {
            Rgba([r, g, b, 255])
        };
    }
    image
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
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
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

    *encoder = Some(H264EncoderSession::new(width, height, profile)?);
    logging::append_log(
        "INFO",
        "media.h264_encoder",
        format!("config sent width={} height={}", width, height),
    );
    let config = json!({
        "width": width,
        "height": height,
    });
    let payload = serde_json::to_vec(&config).map_err(|error| error.to_string())?;
    let packet = encode_media_packet(StreamCodec::H264, MediaPacketKind::Config, &payload);
    socket
        .send(Message::Binary(packet.into()))
        .map_err(|error| error.to_string())?;
    Ok(())
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
