use dxgi_capture_rs::{CaptureError, DXGIManager};
use image::{ImageBuffer, RgbaImage};
use windows_sys::Win32::{
    Foundation::HWND,
    Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, SetStretchBltMode, StretchBlt, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, COLORONCOLOR, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
        SRCCOPY,
    },
    UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN, SM_REMOTESESSION},
};

use crate::logging;

use super::{
    common::{fit_frame, ScreenshotsCaptureBackend},
    CaptureFrame,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowsCaptureBackendKind {
    DxgiDuplication,
    Win32Gdi,
    ScreenshotsFallback,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowsCaptureStrategy {
    LocalDisplay,
    RemoteDesktop,
}

pub struct WindowsCaptureEngine {
    strategy: WindowsCaptureStrategy,
    preferred_backend: WindowsCaptureBackendKind,
    dxgi_backend: Option<DxgiCaptureBackend>,
    gdi_backend: Option<GdiCaptureBackend>,
    screenshots_fallback: ScreenshotsCaptureBackend,
    last_successful_frame: Option<RgbaImage>,
    last_successful_frame_index: u32,
}

impl WindowsCaptureEngine {
    pub fn new() -> Self {
        let strategy = detect_capture_strategy();
        let dxgi_backend = if strategy == WindowsCaptureStrategy::LocalDisplay {
            match DxgiCaptureBackend::new() {
                Ok(backend) => Some(backend),
                Err(error) => {
                    logging::append_log(
                        "WARN",
                        "capture.dxgi",
                        format!("initialization failed: {}", error),
                    );
                    None
                }
            }
        } else {
            logging::append_log(
                "INFO",
                "capture",
                "remote desktop session detected, preferring scaled GDI capture",
            );
            None
        };

        let gdi_backend = match GdiCaptureBackend::new() {
            Ok(backend) => Some(backend),
            Err(error) => {
                logging::append_log(
                    "WARN",
                    "capture.gdi",
                    format!("initialization failed: {}", error),
                );
                None
            }
        };

        Self {
            strategy,
            preferred_backend: if strategy == WindowsCaptureStrategy::RemoteDesktop {
                WindowsCaptureBackendKind::Win32Gdi
            } else {
                WindowsCaptureBackendKind::DxgiDuplication
            },
            dxgi_backend,
            gdi_backend,
            screenshots_fallback: ScreenshotsCaptureBackend::with_backend_name(
                "windows-screenshots-fallback",
            ),
            last_successful_frame: None,
            last_successful_frame_index: 0,
        }
    }

    pub fn capture(&mut self, max_dimensions: (u32, u32), frame_index: u32) -> CaptureFrame {
        if self.preferred_backend == WindowsCaptureBackendKind::ScreenshotsFallback
            && frame_index % 120 == 0
        {
            self.try_restore_primary_backends();
        }

        match self.preferred_backend {
            WindowsCaptureBackendKind::DxgiDuplication => {
                if let Some(backend) = &mut self.dxgi_backend {
                    match backend.capture(max_dimensions, frame_index) {
                        Ok(image) => self.capture_frame(image, "windows-dxgi", false, frame_index),
                        Err(error) => {
                            logging::append_log(
                                "WARN",
                                "capture.dxgi",
                                format!("frame capture failed: {}", error),
                            );

                            if should_retry_dxgi(&error) {
                                match self.capture_with_gdi(max_dimensions) {
                                    Ok(image) => self.capture_frame(
                                        image,
                                        "windows-gdi-temporary",
                                        false,
                                        frame_index,
                                    ),
                                    Err(gdi_error) => {
                                        logging::append_log(
                                            "WARN",
                                            "capture.gdi",
                                            format!(
                                                "temporary fallback after dxgi failure also failed: {}",
                                                gdi_error
                                            ),
                                        );
                                        self.preferred_backend =
                                            WindowsCaptureBackendKind::ScreenshotsFallback;
                                        self.capture_with_screenshots(max_dimensions, frame_index)
                                    }
                                }
                            } else {
                                logging::append_log(
                                    "WARN",
                                    "capture.dxgi",
                                    "switching preferred backend to windows-gdi",
                                );
                                self.preferred_backend = WindowsCaptureBackendKind::Win32Gdi;
                                self.capture(max_dimensions, frame_index)
                            }
                        }
                    }
                } else {
                    logging::append_log(
                        "WARN",
                        "capture.dxgi",
                        "backend unavailable, switching preferred backend to windows-gdi",
                    );
                    self.preferred_backend = WindowsCaptureBackendKind::Win32Gdi;
                    self.capture(max_dimensions, frame_index)
                }
            }
            WindowsCaptureBackendKind::Win32Gdi => match self.capture_with_gdi(max_dimensions) {
                Ok(image) => {
                    let backend_name = if self.strategy == WindowsCaptureStrategy::RemoteDesktop {
                        "windows-rdp-gdi"
                    } else {
                        "windows-gdi"
                    };
                    self.capture_frame(image, backend_name, false, frame_index)
                }
                Err(error) => {
                    logging::append_log(
                            "WARN",
                            "capture.gdi",
                            format!(
                                "frame capture failed, switching preferred backend to screenshots fallback: {}",
                                error
                            ),
                        );
                    self.preferred_backend = WindowsCaptureBackendKind::ScreenshotsFallback;
                    self.capture_with_screenshots(max_dimensions, frame_index)
                }
            },
            WindowsCaptureBackendKind::ScreenshotsFallback => {
                self.capture_with_screenshots(max_dimensions, frame_index)
            }
        }
    }

    fn capture_with_gdi(&mut self, max_dimensions: (u32, u32)) -> Result<RgbaImage, String> {
        if let Some(backend) = &mut self.gdi_backend {
            return backend.capture(max_dimensions, self.strategy);
        }

        let mut backend = GdiCaptureBackend::new()?;
        let result = backend.capture(max_dimensions, self.strategy);
        self.gdi_backend = Some(backend);
        result
    }

    fn capture_with_screenshots(
        &mut self,
        max_dimensions: (u32, u32),
        frame_index: u32,
    ) -> CaptureFrame {
        match self.screenshots_fallback.try_capture(max_dimensions) {
            Ok(image) => self.capture_frame(
                image,
                self.screenshots_fallback.backend_name(),
                false,
                frame_index,
            ),
            Err(error) => {
                logging::append_log(
                    "WARN",
                    "capture.screenshots",
                    format!("frame capture failed: {}", error),
                );
                if let Some(image) = self.last_successful_frame.clone() {
                    return CaptureFrame {
                        image,
                        backend: "windows-last-frame",
                        used_fallback: false,
                    };
                }
                CaptureFrame {
                    image: super::common::build_test_frame(frame_index),
                    backend: "test-fallback",
                    used_fallback: true,
                }
            }
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

    fn try_restore_primary_backends(&mut self) {
        if self.strategy == WindowsCaptureStrategy::LocalDisplay && self.dxgi_backend.is_none() {
            self.dxgi_backend = match DxgiCaptureBackend::new() {
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
        }

        if self.gdi_backend.is_none() {
            self.gdi_backend = match GdiCaptureBackend::new() {
                Ok(backend) => Some(backend),
                Err(error) => {
                    logging::append_log(
                        "WARN",
                        "capture.gdi",
                        format!("reinitialization failed: {}", error),
                    );
                    None
                }
            };
        }

        if self.dxgi_backend.is_some() {
            logging::append_log("INFO", "capture", "retrying primary backend windows-dxgi");
            self.preferred_backend = WindowsCaptureBackendKind::DxgiDuplication;
        } else if self.gdi_backend.is_some() {
            let backend_name = if self.strategy == WindowsCaptureStrategy::RemoteDesktop {
                "windows-rdp-gdi"
            } else {
                "windows-gdi"
            };
            logging::append_log(
                "INFO",
                "capture",
                format!("retrying backend {backend_name}"),
            );
            self.preferred_backend = WindowsCaptureBackendKind::Win32Gdi;
        }
    }
}

struct DxgiCaptureBackend {
    manager: DXGIManager,
    last_frame: Option<RgbaImage>,
    last_frame_index: u32,
}

impl DxgiCaptureBackend {
    fn new() -> Result<Self, String> {
        let mut manager = DXGIManager::new(16).map_err(|error| error.to_string())?;
        manager.set_capture_source_index(0);
        Ok(Self {
            manager,
            last_frame: None,
            last_frame_index: 0,
        })
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
                self.manager.set_capture_source_index(0);
                self.last_frame
                    .clone()
                    .ok_or_else(|| "dxgi access lost before first frame".to_owned())
            }
            Err(error) => Err(error.to_string()),
        }
    }
}

struct GdiCaptureBackend {
    screen_dc: HDC,
    memory_dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    source_width: i32,
    source_height: i32,
    capture_width: i32,
    capture_height: i32,
    bgra: Vec<u8>,
}

impl GdiCaptureBackend {
    fn new() -> Result<Self, String> {
        let (source_width, source_height) = current_screen_size()?;
        Self::create(source_width, source_height, source_width, source_height)
    }

    fn capture(
        &mut self,
        max_dimensions: (u32, u32),
        strategy: WindowsCaptureStrategy,
    ) -> Result<RgbaImage, String> {
        let (source_width, source_height) = current_screen_size()?;
        let (capture_width, capture_height) =
            desired_capture_size(source_width, source_height, max_dimensions, strategy);

        if source_width != self.source_width
            || source_height != self.source_height
            || capture_width != self.capture_width
            || capture_height != self.capture_height
        {
            let replacement =
                Self::create(source_width, source_height, capture_width, capture_height)?;
            *self = replacement;
        }

        let blt_ok = if strategy == WindowsCaptureStrategy::RemoteDesktop {
            unsafe {
                SetStretchBltMode(self.memory_dc, COLORONCOLOR);
                StretchBlt(
                    self.memory_dc,
                    0,
                    0,
                    self.capture_width,
                    self.capture_height,
                    self.screen_dc,
                    0,
                    0,
                    self.source_width,
                    self.source_height,
                    SRCCOPY | CAPTUREBLT,
                )
            }
        } else {
            unsafe {
                BitBlt(
                    self.memory_dc,
                    0,
                    0,
                    self.capture_width,
                    self.capture_height,
                    self.screen_dc,
                    0,
                    0,
                    SRCCOPY | CAPTUREBLT,
                )
            }
        };
        if blt_ok == 0 {
            return Err(if strategy == WindowsCaptureStrategy::RemoteDesktop {
                "StretchBlt failed".to_owned()
            } else {
                "BitBlt failed".to_owned()
            });
        }

        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: self.capture_width,
                biHeight: -self.capture_height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: (self.capture_width * self.capture_height * 4) as u32,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [unsafe { std::mem::zeroed() }; 1],
        };

        let scanlines = unsafe {
            GetDIBits(
                self.memory_dc,
                self.bitmap,
                0,
                self.capture_height as u32,
                self.bgra.as_mut_ptr() as *mut _,
                &mut info,
                DIB_RGB_COLORS,
            )
        };

        if scanlines == 0 {
            return Err("GetDIBits failed".to_owned());
        }

        let image = ImageBuffer::from_raw(
            self.capture_width as u32,
            self.capture_height as u32,
            self.bgra.clone(),
        );
        Ok(
            if self.capture_width as u32 == max_dimensions.0
                && self.capture_height as u32 == max_dimensions.1
            {
                image.ok_or_else(|| "failed to build BGRA frame".to_owned())?
            } else {
                fit_bgra_frame(
                    self.capture_width as u32,
                    self.capture_height as u32,
                    self.bgra.clone(),
                    max_dimensions,
                )?
            },
        )
    }

    fn create(
        source_width: i32,
        source_height: i32,
        capture_width: i32,
        capture_height: i32,
    ) -> Result<Self, String> {
        let screen_dc = unsafe { GetDC(0 as HWND) };
        if screen_dc.is_null() {
            return Err("GetDC failed".to_owned());
        }

        let memory_dc = unsafe { CreateCompatibleDC(screen_dc) };
        if memory_dc.is_null() {
            unsafe {
                ReleaseDC(0 as HWND, screen_dc);
            }
            return Err("CreateCompatibleDC failed".to_owned());
        }

        let bitmap = unsafe { CreateCompatibleBitmap(screen_dc, capture_width, capture_height) };
        if bitmap.is_null() {
            unsafe {
                DeleteDC(memory_dc);
                ReleaseDC(0 as HWND, screen_dc);
            }
            return Err("CreateCompatibleBitmap failed".to_owned());
        }

        let previous = unsafe { SelectObject(memory_dc, bitmap as HGDIOBJ) };
        if previous.is_null() {
            unsafe {
                DeleteObject(bitmap as HGDIOBJ);
                DeleteDC(memory_dc);
                ReleaseDC(0 as HWND, screen_dc);
            }
            return Err("SelectObject failed".to_owned());
        }

        Ok(Self {
            screen_dc,
            memory_dc,
            bitmap,
            previous,
            source_width,
            source_height,
            capture_width,
            capture_height,
            bgra: vec![0_u8; (capture_width as usize) * (capture_height as usize) * 4],
        })
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

impl Drop for GdiCaptureBackend {
    fn drop(&mut self) {
        unsafe {
            if !self.memory_dc.is_null() && !self.previous.is_null() {
                SelectObject(self.memory_dc, self.previous);
            }
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap as HGDIOBJ);
            }
            if !self.memory_dc.is_null() {
                DeleteDC(self.memory_dc);
            }
            if !self.screen_dc.is_null() {
                ReleaseDC(0 as HWND, self.screen_dc);
            }
        }
    }
}

fn current_screen_size() -> Result<(i32, i32), String> {
    let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    if width <= 0 || height <= 0 {
        return Err("invalid screen size".to_owned());
    }
    Ok((width, height))
}

fn should_retry_dxgi(error: &str) -> bool {
    error.contains("timeout before first frame") || error.contains("access lost before first frame")
}

fn detect_capture_strategy() -> WindowsCaptureStrategy {
    if unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0 {
        WindowsCaptureStrategy::RemoteDesktop
    } else {
        WindowsCaptureStrategy::LocalDisplay
    }
}

fn desired_capture_size(
    source_width: i32,
    source_height: i32,
    max_dimensions: (u32, u32),
    strategy: WindowsCaptureStrategy,
) -> (i32, i32) {
    if strategy != WindowsCaptureStrategy::RemoteDesktop {
        return (source_width, source_height);
    }

    if source_width <= 0 || source_height <= 0 {
        return (1, 1);
    }

    let target =
        capture_target_dimensions(source_width as u32, source_height as u32, max_dimensions);
    (target.0.max(1) as i32, target.1.max(1) as i32)
}

fn capture_target_dimensions(
    source_width: u32,
    source_height: u32,
    max_dimensions: (u32, u32),
) -> (u32, u32) {
    if max_dimensions.0 == 0 || max_dimensions.1 == 0 {
        return (source_width.max(1), source_height.max(1));
    }

    let scale = (max_dimensions.0 as f32 / source_width as f32)
        .min(max_dimensions.1 as f32 / source_height as f32)
        .min(1.0);

    let mut width = ((source_width as f32 * scale).round() as u32).max(1);
    let mut height = ((source_height as f32 * scale).round() as u32).max(1);

    if width > 1 && width % 2 != 0 {
        width = width.saturating_sub(1);
    }
    if height > 1 && height % 2 != 0 {
        height = height.saturating_sub(1);
    }

    (width.max(1), height.max(1))
}
