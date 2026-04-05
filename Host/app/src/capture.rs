mod common;
#[cfg(windows)]
mod windows;

use image::RgbaImage;

#[cfg(windows)]
use windows::WindowsCaptureEngine;

#[cfg(not(windows))]
use common::ScreenshotsCaptureBackend;

pub struct CaptureFrame {
    pub image: RgbaImage,
    pub backend: &'static str,
    pub used_fallback: bool,
}

pub struct CaptureEngine {
    #[cfg(windows)]
    inner: WindowsCaptureEngine,
    #[cfg(not(windows))]
    inner: ScreenshotsCaptureBackend,
}

impl CaptureEngine {
    pub fn new() -> Self {
        Self {
            #[cfg(windows)]
            inner: WindowsCaptureEngine::new(),
            #[cfg(not(windows))]
            inner: ScreenshotsCaptureBackend::with_backend_name("screenshots"),
        }
    }

    pub fn capture(&mut self, max_dimensions: (u32, u32), frame_index: u32) -> CaptureFrame {
        self.inner.capture(max_dimensions, frame_index)
    }
}
