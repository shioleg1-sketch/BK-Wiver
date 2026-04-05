use dxgi_capture_rs::{
    CaptureError, DXGIManager, describe_dxgi_adapters_and_outputs,
    describe_dxgi_initialization_attempts,
};
use image::{ImageBuffer, RgbaImage};
use screenshots::Screen;
use std::time::Duration;
use windows::{
    Graphics::{
        Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession},
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
    },
    Win32::{
        Foundation::POINT,
        Graphics::{
            Direct3D::{
                D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_11_0,
                D3D_FEATURE_LEVEL_9_1,
            },
            Direct3D11::{
                D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
                D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
                D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
                ID3D11Texture2D,
            },
            Dxgi::{Common::DXGI_SAMPLE_DESC, IDXGIDevice},
            Gdi::{HMONITOR, MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint},
        },
        System::WinRT::{
            Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
            Graphics::Capture::IGraphicsCaptureItemInterop,
        },
    },
    core::{Interface, factory},
};
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CaptureAvailabilityState {
    Ready,
    VirtualDisplayPending,
    CaptureUnavailable,
}

pub struct WindowsCaptureEngine {
    strategy: WindowsCaptureStrategy,
    dxgi_backend: Option<DxgiCaptureBackend>,
    wgc_backend: Option<WgcCaptureBackend>,
    availability_state: CaptureAvailabilityState,
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
                log_dxgi_environment();
                None
            }
        };
        let wgc_backend = if dxgi_backend.is_none() {
            match WgcCaptureBackend::new() {
                Ok(backend) => {
                    logging::append_log(
                        "INFO",
                        "capture.wgc",
                        format!("initialized backend={}", wgc_backend_name_for(strategy)),
                    );
                    Some(backend)
                }
                Err(error) => {
                    logging::append_log(
                        "WARN",
                        "capture.wgc",
                        format!("initialization failed: {}", error),
                    );
                    None
                }
            }
        } else {
            None
        };
        let availability_state = if dxgi_backend.is_some() {
            CaptureAvailabilityState::Ready
        } else if wgc_backend.is_some() {
            CaptureAvailabilityState::Ready
        } else {
            unavailable_state_for(strategy)
        };

        logging::append_log(
            "INFO",
            "capture.strategy",
            format!(
                "strategy={} preferred_backend={} dxgi_available={} availability={}",
                strategy_name(strategy),
                backend_name_for(strategy),
                dxgi_backend.is_some(),
                availability_name_for(strategy, availability_state),
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
            wgc_backend,
            availability_state,
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
                    self.availability_state = CaptureAvailabilityState::Ready;
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
                        self.availability_state = unavailable_state_for(self.strategy);
                        self.bump_retry_backoff(frame_index);
                    }

                    return self.unavailable_frame(frame_index);
                }
            }
        }

        if let Some(backend) = self.wgc_backend.as_mut() {
            match backend.capture(max_dimensions, frame_index) {
                Ok(image) => {
                    self.availability_state = CaptureAvailabilityState::Ready;
                    return self.capture_frame(image, wgc_backend_name_for(self.strategy), false, frame_index);
                }
                Err(error) => {
                    logging::append_log(
                        "WARN",
                        "capture.wgc",
                        format!("frame capture failed: {}", error),
                    );
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
            self.wgc_backend = None;
            logging::append_log(
                "INFO",
                "capture",
                format!("retrying backend {}", backend_name_for(self.strategy)),
            );
            self.availability_state = CaptureAvailabilityState::Ready;
            self.reset_retry_backoff(frame_index);
            true
        } else {
            if self.wgc_backend.is_none() {
                self.wgc_backend = match WgcCaptureBackend::new() {
                    Ok(backend) => {
                        logging::append_log(
                            "INFO",
                            "capture.wgc",
                            format!("reinitialized backend={}", wgc_backend_name_for(self.strategy)),
                        );
                        Some(backend)
                    }
                    Err(error) => {
                        logging::append_log(
                            "WARN",
                            "capture.wgc",
                            format!("reinitialization failed: {}", error),
                        );
                        None
                    }
                };
            }
            if self.wgc_backend.is_some() {
                self.availability_state = CaptureAvailabilityState::Ready;
            } else {
                self.availability_state = unavailable_state_for(self.strategy);
            }
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

fn log_dxgi_environment() {
    match describe_dxgi_adapters_and_outputs() {
        Ok(lines) => {
            if lines.is_empty() {
                logging::append_log("WARN", "capture.dxgi", "dxgi environment probe returned no adapters");
            } else {
                for line in lines {
                    logging::append_log("INFO", "capture.dxgi_probe", line);
                }
            }
        }
        Err(error) => logging::append_log(
            "WARN",
            "capture.dxgi_probe",
            format!("failed to enumerate dxgi adapters/outputs: {}", error),
        ),
    }

    match describe_dxgi_initialization_attempts(0) {
        Ok(lines) => {
            if lines.is_empty() {
                logging::append_log(
                    "WARN",
                    "capture.dxgi_probe",
                    "dxgi initialization probe returned no attempts",
                );
            } else {
                for line in lines {
                    logging::append_log("INFO", "capture.dxgi_probe", line);
                }
            }
        }
        Err(error) => logging::append_log(
            "WARN",
            "capture.dxgi_probe",
            format!("failed to probe dxgi initialization attempts: {}", error),
        ),
    }
}

struct DxgiCaptureBackend {
    manager: DXGIManager,
    source_index: usize,
    last_frame: Option<RgbaImage>,
    last_frame_index: u32,
}

struct WgcCaptureBackend {
    _d3d_device: ID3D11Device,
    d3d_context: ID3D11DeviceContext,
    frame_pool: Direct3D11CaptureFramePool,
    _session: GraphicsCaptureSession,
    _item: GraphicsCaptureItem,
    staging_texture: Option<ID3D11Texture2D>,
    staging_dimensions: (u32, u32),
    last_frame: Option<RgbaImage>,
    last_frame_index: u32,
}

impl WgcCaptureBackend {
    fn new() -> Result<Self, String> {
        let _ = windows::core::initialize_mta().map_err(|error| error.to_string())?;
        if !GraphicsCaptureSession::IsSupported().map_err(|error| error.to_string())? {
            return Err("Windows Graphics Capture is not supported".to_owned());
        }

        let monitor = primary_monitor_handle()?;
        let item_interop: IGraphicsCaptureItemInterop =
            factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
                .map_err(|error| error.to_string())?;
        let item = unsafe {
            item_interop
                .CreateForMonitor::<GraphicsCaptureItem>(monitor)
                .map_err(|error| error.to_string())?
        };

        let mut d3d_device = None;
        let mut d3d_context = None;
        let feature_levels = [
            D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_10_0,
            D3D_FEATURE_LEVEL_9_1,
        ];
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut d3d_device),
                None,
                Some(&mut d3d_context),
            )
        }
        .map_err(|error| error.to_string())?;

        let d3d_device = d3d_device.ok_or_else(|| "missing D3D11 device".to_owned())?;
        let d3d_context = d3d_context.ok_or_else(|| "missing D3D11 device context".to_owned())?;
        let dxgi_device: IDXGIDevice = d3d_device.cast().map_err(|error| error.to_string())?;
        let inspectable = unsafe {
            CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device).map_err(|error| error.to_string())?
        };
        let winrt_device: IDirect3DDevice =
            inspectable.cast().map_err(|error| error.to_string())?;

        let size = item.Size().map_err(|error| error.to_string())?;
        let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )
        .map_err(|error| error.to_string())?;
        let session = frame_pool
            .CreateCaptureSession(&item)
            .map_err(|error| error.to_string())?;
        let _ = session.SetIsCursorCaptureEnabled(true);
        let _ = session.SetIsBorderRequired(false);
        session.StartCapture().map_err(|error| error.to_string())?;

        Ok(Self {
            _d3d_device: d3d_device,
            d3d_context,
            frame_pool,
            _session: session,
            _item: item,
            staging_texture: None,
            staging_dimensions: (0, 0),
            last_frame: None,
            last_frame_index: 0,
        })
    }

    fn capture(
        &mut self,
        max_dimensions: (u32, u32),
        frame_index: u32,
    ) -> Result<RgbaImage, String> {
        let frame = match self.frame_pool.TryGetNextFrame() {
            Ok(frame) => frame,
            Err(error) => {
                return self
                    .last_frame
                    .clone()
                    .ok_or_else(|| format!("wgc frame unavailable before first frame: {}", error));
            }
        };

        let content_size = frame.ContentSize().map_err(|error| error.to_string())?;
        let surface = frame.Surface().map_err(|error| error.to_string())?;
        let access: IDirect3DDxgiInterfaceAccess =
            surface.cast().map_err(|error| error.to_string())?;
        let source_texture: ID3D11Texture2D =
            unsafe { access.GetInterface() }.map_err(|error| error.to_string())?;

        let staging_texture =
            self.ensure_staging_texture(&source_texture, content_size.Width as u32, content_size.Height as u32)?;

        unsafe {
            self.d3d_context.CopyResource(staging_texture, &source_texture);
        }

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.d3d_context
                .Map(staging_texture, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|error| error.to_string())?;
        }

        let width = content_size.Width.max(1) as usize;
        let height = content_size.Height.max(1) as usize;
        let row_pitch = mapped.RowPitch as usize;
        let data_ptr = mapped.pData as *const u8;
        let mut bgra = vec![0_u8; width * height * 4];
        for row in 0..height {
            let src_row = unsafe { std::slice::from_raw_parts(data_ptr.add(row * row_pitch), width * 4) };
            let dst_offset = row * width * 4;
            bgra[dst_offset..dst_offset + (width * 4)].copy_from_slice(src_row);
        }

        unsafe {
            self.d3d_context.Unmap(staging_texture, 0);
        }

        let image = if width as u32 == max_dimensions.0 && height as u32 == max_dimensions.1 {
            ImageBuffer::from_raw(width as u32, height as u32, bgra)
                .ok_or_else(|| "failed to build WGC frame".to_owned())?
        } else {
            fit_bgra_frame(width as u32, height as u32, bgra, max_dimensions)?
        };

        if self.last_frame.is_none()
            || frame_index.wrapping_sub(self.last_frame_index) >= 30
        {
            self.last_frame = Some(image.clone());
            self.last_frame_index = frame_index;
        }

        let _ = frame.Close();
        Ok(image)
    }

    fn ensure_staging_texture(
        &mut self,
        source_texture: &ID3D11Texture2D,
        width: u32,
        height: u32,
    ) -> Result<&ID3D11Texture2D, String> {
        if self.staging_texture.is_none() || self.staging_dimensions != (width, height) {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            unsafe {
                source_texture.GetDesc(&mut desc);
            }
            desc.Width = width;
            desc.Height = height;
            desc.MipLevels = 1;
            desc.ArraySize = 1;
            desc.SampleDesc = DXGI_SAMPLE_DESC { Count: 1, Quality: 0 };
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;

            let mut texture = None;
            unsafe {
                self._d3d_device
                    .CreateTexture2D(&desc, None, Some(&mut texture))
                    .map_err(|error| error.to_string())?;
            }
            self.staging_texture = texture;
            self.staging_dimensions = (width, height);
        }

        self.staging_texture
            .as_ref()
            .ok_or_else(|| "missing WGC staging texture".to_owned())
    }
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
    availability_name_for(strategy, unavailable_state_for(strategy))
}

fn wgc_backend_name_for(strategy: WindowsCaptureStrategy) -> &'static str {
    match strategy {
        WindowsCaptureStrategy::LocalDisplay => "windows-wgc",
        WindowsCaptureStrategy::RemoteDesktopWithDisplay => "windows-rdp-wgc",
        WindowsCaptureStrategy::HeadlessNoDisplay => "windows-headless-wgc",
    }
}

fn unavailable_state_for(strategy: WindowsCaptureStrategy) -> CaptureAvailabilityState {
    match strategy {
        WindowsCaptureStrategy::HeadlessNoDisplay => CaptureAvailabilityState::VirtualDisplayPending,
        WindowsCaptureStrategy::RemoteDesktopWithDisplay => CaptureAvailabilityState::CaptureUnavailable,
        WindowsCaptureStrategy::LocalDisplay => CaptureAvailabilityState::CaptureUnavailable,
    }
}

fn availability_name_for(
    strategy: WindowsCaptureStrategy,
    state: CaptureAvailabilityState,
) -> &'static str {
    match (strategy, state) {
        (_, CaptureAvailabilityState::Ready) => backend_name_for(strategy),
        (WindowsCaptureStrategy::LocalDisplay, CaptureAvailabilityState::VirtualDisplayPending) => {
            "windows-virtual-display-pending"
        }
        (
            WindowsCaptureStrategy::RemoteDesktopWithDisplay,
            CaptureAvailabilityState::VirtualDisplayPending,
        ) => "windows-rdp-virtual-display-pending",
        (
            WindowsCaptureStrategy::HeadlessNoDisplay,
            CaptureAvailabilityState::VirtualDisplayPending,
        ) => "windows-headless-virtual-display-pending",
        (WindowsCaptureStrategy::LocalDisplay, CaptureAvailabilityState::CaptureUnavailable) => {
            "windows-dxgi-unavailable"
        }
        (
            WindowsCaptureStrategy::RemoteDesktopWithDisplay,
            CaptureAvailabilityState::CaptureUnavailable,
        ) => "windows-rdp-dxgi-unavailable",
        (
            WindowsCaptureStrategy::HeadlessNoDisplay,
            CaptureAvailabilityState::CaptureUnavailable,
        ) => "windows-headless-dxgi-unavailable",
    }
}

fn primary_monitor_handle() -> Result<HMONITOR, String> {
    let monitor = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
    if monitor.0.is_null() {
        Err("primary monitor handle is null".to_owned())
    } else {
        Ok(monitor)
    }
}
