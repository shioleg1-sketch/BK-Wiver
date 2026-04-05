use dxgi_capture_rs::{CaptureError, DXGIManager};
use image::{ImageBuffer, RgbaImage};
use screenshots::Screen;
use std::time::Duration;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN, SM_REMOTESESSION,
};

use crate::logging;

use super::{
    CaptureFrame,
    common::{build_test_frame, fit_frame},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowsCaptureStrategy {
    LocalDisplay,
    RemoteDesktopWithDisplay,
    HeadlessNoDisplay,
}

pub struct WindowsCaptureEngine {
    strategy: WindowsCaptureStrategy,
    dxgi_backend: Option<DxgiCaptureBackend>,
    consecutive_slow_dxgi_frames: u32,
    next_retry_frame: u32,
    retry_interval_frames: u32,
    last_successful_frame: Option<RgbaImage>,
    last_successful_frame_index: u32,
}

impl WindowsCaptureEngine {
    pub fn new() -> Self {
        let strategy = detect_capture_strategy();
        let dxgi_backend = match DxgiCaptureBackend::new() {
            Ok(backend) => Some(backend),
            Err(error) => {
                logging::append_log(
                    "WARN",
                    "capture.dxgi",
                    format!("initialization failed: {}", error),
                );
                None
            }
        };

        logging::append_log(
            "INFO",
            "capture.strategy",
            format!(
                "strategy={} preferred_backend={} dxgi_available={}",
                strategy_name(strategy),
                backend_name_for(strategy),
                dxgi_backend.is_some(),
            ),
        );

        if strategy == WindowsCaptureStrategy::HeadlessNoDisplay {
            logging::append_log(
                "WARN",
                "capture.strategy",
                "headless session detected without an active display output; DXGI capture requires a real or virtual display",
            );
        }

        Self {
            strategy,
            dxgi_backend,
            consecutive_slow_dxgi_frames: 0,
            next_retry_frame: 120,
            retry_interval_frames: 120,
            last_successful_frame: None,
            last_successful_frame_index: 0,
        }
    }

    pub fn capture(&mut self, max_dimensions: (u32, u32), frame_index: u32) -> CaptureFrame {
        if self.dxgi_backend.is_none() && frame_index >= self.next_retry_frame {
            self.try_restore_dxgi(frame_index);
        }

        if self.dxgi_backend.is_some() {
            let (result, capture_elapsed) = {
                let backend = self.dxgi_backend.as_mut().expect("dxgi backend checked");
                let started = std::time::Instant::now();
                let result = backend.capture(max_dimensions, frame_index);
                (result, started.elapsed())
            };

            match result {
                Ok(image) => {
                    self.handle_dxgi_capture_timing(capture_elapsed);
                    self.reset_retry_backoff(frame_index);
                    return self.capture_frame(image, backend_name_for(self.strategy), false, frame_index);
                }
                Err(error) => {
                    self.consecutive_slow_dxgi_frames = 0;
                    logging::append_log(
                        "WARN",
                        "capture.dxgi",
                        format!("frame capture failed: {}", error),
                    );

                    if should_retry_dxgi(&error) {
                        self.dxgi_backend = None;
                        self.bump_retry_backoff(frame_index);
                    }

                    return self.unavailable_frame(frame_index);
                }
            }
        }

        self.unavailable_frame(frame_index)
    }

    fn unavailable_frame(&mut self, frame_index: u32) -> CaptureFrame {
        if let Some(image) = self.last_successful_frame.clone() {
            return CaptureFrame {
                image,
                backend: unavailable_backend_name_for(self.strategy),
                used_fallback: true,
            };
        }

        CaptureFrame {
            image: build_test_frame(frame_index),
            backend: unavailable_backend_name_for(self.strategy),
            used_fallback: true,
        }
    }

    fn capture_frame(
        &mut self,
        image: RgbaImage,
        backend: &'static str,
        used_fallback: bool,
        frame_index: u32,
    ) -> CaptureFrame {
        if self.last_successful_frame.is_none()
            || frame_index.wrapping_sub(self.last_successful_frame_index) >= 30
        {
            self.last_successful_frame = Some(image.clone());
            self.last_successful_frame_index = frame_index;
        }

        CaptureFrame {
            image,
            backend,
            used_fallback,
        }
    }

    fn try_restore_dxgi(&mut self, frame_index: u32) -> bool {
        let preferred_source_index = self
            .dxgi_backend
            .as_ref()
            .map(DxgiCaptureBackend::source_index)
            .unwrap_or(0);

        self.dxgi_backend = match DxgiCaptureBackend::with_preferred_source_index(preferred_source_index) {
            Ok(backend) => Some(backend),
            Err(error) => {
                logging::append_log(
                    "WARN",
                    "capture.dxgi",
                    format!("reinitialization failed: {}", error),
                );
                None
            }
        };

        if self.dxgi_backend.is_some() {
            logging::append_log(
                "INFO",
                "capture",
                format!("retrying backend {}", backend_name_for(self.strategy)),
            );
            self.reset_retry_backoff(frame_index);
            true
        } else {
            self.bump_retry_backoff(frame_index);
            false
        }
    }

    fn reset_retry_backoff(&mut self, frame_index: u32) {
        self.retry_interval_frames = 120;
        self.next_retry_frame = frame_index.saturating_add(self.retry_interval_frames);
    }

    fn bump_retry_backoff(&mut self, frame_index: u32) {
        self.retry_interval_frames = (self.retry_interval_frames.saturating_mul(2)).clamp(120, 960);
        self.next_retry_frame = frame_index.saturating_add(self.retry_interval_frames);
    }

    fn handle_dxgi_capture_timing(&mut self, elapsed: Duration) {
        const SLOW_DXGI_FRAME_MS: u128 = 75;
        const SEVERE_DXGI_FRAME_MS: u128 = 120;
        const MAX_CONSECUTIVE_SLOW_DXGI_FRAMES: u32 = 3;

        let elapsed_ms = elapsed.as_millis();
        if elapsed_ms > SLOW_DXGI_FRAME_MS {
            self.consecutive_slow_dxgi_frames =
                self.consecutive_slow_dxgi_frames.saturating_add(1);
            if self.consecutive_slow_dxgi_frames == 1
                || elapsed_ms > SEVERE_DXGI_FRAME_MS
                || self.consecutive_slow_dxgi_frames >= MAX_CONSECUTIVE_SLOW_DXGI_FRAMES
            {
                logging::append_log(
                    "WARN",
                    "capture.dxgi",
                    format!(
                        "slow frame detected elapsed_ms={} consecutive_slow_frames={}",
                        elapsed_ms, self.consecutive_slow_dxgi_frames
                    ),
                );
            }

            if self.consecutive_slow_dxgi_frames >= MAX_CONSECUTIVE_SLOW_DXGI_FRAMES {
                self.reinitialize_dxgi_backend(format!(
                    "slow frame recovery elapsed_ms={} consecutive_slow_frames={}",
                    elapsed_ms, self.consecutive_slow_dxgi_frames
                ));
            }
        } else {
            self.consecutive_slow_dxgi_frames = 0;
        }
    }

    fn reinitialize_dxgi_backend(&mut self, reason: String) {
        logging::append_log(
            "WARN",
            "capture.dxgi",
            format!("reinitializing backend reason={}", reason),
        );

        let preferred_source_index = self
            .dxgi_backend
            .as_ref()
            .map(DxgiCaptureBackend::source_index)
            .unwrap_or(0);

        let replacement = match DxgiCaptureBackend::with_preferred_source_index(preferred_source_index) {
            Ok(backend) => Some(backend),
            Err(error) => {
                logging::append_log(
                    "WARN",
                    "capture.dxgi",
                    format!("reinitialization failed after slow frame: {}", error),
                );
                None
            }
        };

        if let Some(backend) = replacement {
            self.dxgi_backend = Some(backend);
        } else {
            logging::append_log(
                "WARN",
                "capture.dxgi",
                "keeping existing dxgi backend after failed slow-frame reinitialization",
            );
        }

        self.consecutive_slow_dxgi_frames = 0;
    }
}

struct DxgiCaptureBackend {
    manager: DXGIManager,
    source_index: usize,
    last_frame: Option<RgbaImage>,
    last_frame_index: u32,
}

impl DxgiCaptureBackend {
    fn new() -> Result<Self, String> {
        Self::with_preferred_source_index(0)
    }

    fn with_preferred_source_index(preferred_source_index: usize) -> Result<Self, String> {
        let mut manager = DXGIManager::new(16).map_err(|error| error.to_string())?;
        if preferred_source_index > 0 {
            manager.set_capture_source_index(preferred_source_index);
        }
        let source_index = manager.get_capture_source_index();
        logging::append_log(
            "INFO",
            "capture.dxgi",
            format!(
                "initialized source_index={} preferred_source_index={}",
                source_index, preferred_source_index
            ),
        );
        Ok(Self {
            manager,
            source_index,
            last_frame: None,
            last_frame_index: 0,
        })
    }

    fn source_index(&self) -> usize {
        self.source_index
    }

    fn capture(
        &mut self,
        max_dimensions: (u32, u32),
        frame_index: u32,
    ) -> Result<RgbaImage, String> {
        match self.manager.capture_frame_components() {
            Ok((bgra, (width, height))) => {
                let image = if width as u32 == max_dimensions.0 && height as u32 == max_dimensions.1
                {
                    ImageBuffer::from_raw(width as u32, height as u32, bgra)
                        .ok_or_else(|| "failed to build BGRA frame".to_owned())?
                } else {
                    fit_bgra_frame(width as u32, height as u32, bgra, max_dimensions)?
                };
                if self.last_frame.is_none()
                    || frame_index.wrapping_sub(self.last_frame_index) >= 30
                {
                    self.last_frame = Some(image.clone());
                    self.last_frame_index = frame_index;
                }
                Ok(image)
            }
            Err(CaptureError::Timeout) => self
                .last_frame
                .clone()
                .ok_or_else(|| "dxgi frame timeout before first frame".to_owned()),
            Err(CaptureError::AccessLost) => {
                self.manager = DXGIManager::new(16).map_err(|error| error.to_string())?;
                if self.source_index > 0 {
                    self.manager.set_capture_source_index(self.source_index);
                }
                self.source_index = self.manager.get_capture_source_index();
                self.last_frame
                    .clone()
                    .ok_or_else(|| "dxgi access lost before first frame".to_owned())
            }
            Err(error) => Err(error.to_string()),
        }
    }
}

fn fit_bgra_frame(
    width: u32,
    height: u32,
    mut bgra: Vec<u8>,
    max_dimensions: (u32, u32),
) -> Result<RgbaImage, String> {
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    let rgba = ImageBuffer::from_raw(width, height, bgra)
        .ok_or_else(|| "failed to build RGBA frame".to_owned())?;
    let mut fitted = fit_frame(rgba, max_dimensions);
    for pixel in fitted.as_mut().chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Ok(fitted)
}

fn should_retry_dxgi(error: &str) -> bool {
    error.contains("timeout before first frame") || error.contains("access lost before first frame")
}

fn detect_capture_strategy() -> WindowsCaptureStrategy {
    let remote = unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0;
    let has_display_output = has_active_display_output();

    let strategy = if remote && has_display_output {
        WindowsCaptureStrategy::RemoteDesktopWithDisplay
    } else if remote {
        WindowsCaptureStrategy::HeadlessNoDisplay
    } else {
        WindowsCaptureStrategy::LocalDisplay
    };

    match strategy {
        WindowsCaptureStrategy::LocalDisplay => {
            logging::append_log("INFO", "capture", "local display session detected");
        }
        WindowsCaptureStrategy::RemoteDesktopWithDisplay => {
            logging::append_log(
                "INFO",
                "capture",
                "remote desktop session detected with active display output; DXGI-only capture enabled",
            );
        }
        WindowsCaptureStrategy::HeadlessNoDisplay => {
            logging::append_log(
                "WARN",
                "capture",
                "remote desktop/headless session detected without active display output; DXGI capture may remain unavailable until a real or virtual display appears",
            );
        }
    }

    strategy
}

fn strategy_name(strategy: WindowsCaptureStrategy) -> &'static str {
    match strategy {
        WindowsCaptureStrategy::LocalDisplay => "local-display",
        WindowsCaptureStrategy::RemoteDesktopWithDisplay => "remote-desktop-with-display",
        WindowsCaptureStrategy::HeadlessNoDisplay => "headless-no-display",
    }
}

fn has_active_display_output() -> bool {
    let has_screen = Screen::all().map(|screens| !screens.is_empty()).unwrap_or(false);
    let has_metrics =
        unsafe { GetSystemMetrics(SM_CXSCREEN) } > 0 && unsafe { GetSystemMetrics(SM_CYSCREEN) } > 0;
    has_screen || has_metrics
}

fn backend_name_for(strategy: WindowsCaptureStrategy) -> &'static str {
    match strategy {
        WindowsCaptureStrategy::LocalDisplay => "windows-dxgi",
        WindowsCaptureStrategy::RemoteDesktopWithDisplay => "windows-rdp-dxgi",
        WindowsCaptureStrategy::HeadlessNoDisplay => "windows-headless-dxgi",
    }
}

fn unavailable_backend_name_for(strategy: WindowsCaptureStrategy) -> &'static str {
    match strategy {
        WindowsCaptureStrategy::LocalDisplay => "windows-dxgi-unavailable",
        WindowsCaptureStrategy::RemoteDesktopWithDisplay => "windows-rdp-dxgi-unavailable",
        WindowsCaptureStrategy::HeadlessNoDisplay => "windows-headless-dxgi-unavailable",
    }
}
