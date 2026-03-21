use std::{mem::ManuallyDrop, ptr::copy_nonoverlapping};

use image::RgbaImage;
use windows::{
    core::{Error, Interface, VARIANT},
    Win32::{
        Media::MediaFoundation::{
            CLSID_MSH264EncoderMFT, CODECAPI_AVEncCommonLowLatency,
            CODECAPI_AVEncCommonMaxBitRate, CODECAPI_AVEncCommonMeanBitRate,
            CODECAPI_AVEncCommonRateControlMode, CODECAPI_AVEncVideoMaxKeyframeDistance,
            CODECAPI_AVLowLatencyMode, CODECAPI_AVRealtimeControl, IMFMediaBuffer,
            IMFMediaType, IMFSample, IMFTransform, ICodecAPI, MF_E_NOTACCEPTING,
            MF_E_TRANSFORM_NEED_MORE_INPUT, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE,
            MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG_SEQUENCE_HEADER,
            MF_MT_MPEG2_PROFILE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_VERSION,
            MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFShutdown, MFStartup,
            MFVideoFormat_H264, MFVideoFormat_NV12, MFVideoInterlace_Progressive,
            MFT_MESSAGE_COMMAND_FLUSH, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
            MFT_MESSAGE_NOTIFY_END_OF_STREAM, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
            MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_INFO,
            eAVEncCommonRateControlMode_CBR, eAVEncH264VProfile_ConstrainedBase,
        },
        System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize},
    },
};

use super::StreamProfile;

pub struct H264EncoderSession {
    _com_guard: ComGuard,
    _mf_guard: MfGuard,
    transform: IMFTransform,
    output_stream_info: MFT_OUTPUT_STREAM_INFO,
    width: u32,
    height: u32,
    frame_duration_100ns: i64,
    next_sample_time_100ns: i64,
    sequence_header_annex_b: Vec<u8>,
    nal_length_size: usize,
    pending_packets: Vec<Vec<u8>>,
    prepend_sequence_header: bool,
}

type WinResult<T> = windows::core::Result<T>;

impl H264EncoderSession {
    pub fn new(width: u32, height: u32, profile: StreamProfile) -> std::result::Result<Self, String> {
        let com_guard = ComGuard::new().map_err(|error| error.to_string())?;
        let mf_guard = MfGuard::new().map_err(|error| error.to_string())?;
        let transform: IMFTransform = unsafe {
            CoCreateInstance(&CLSID_MSH264EncoderMFT, None, CLSCTX_INPROC_SERVER)
        }
        .map_err(|error| error.to_string())?;

        apply_software_compat_settings(&transform, profile).map_err(|error| error.to_string())?;
        configure_output_type(&transform, width, height, profile).map_err(|error| error.to_string())?;
        configure_input_type(&transform, width, height, profile).map_err(|error| error.to_string())?;
        unsafe {
            transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)
                .map_err(|error| error.to_string())?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|error| error.to_string())?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|error| error.to_string())?;
        }

        let output_stream_info = unsafe { transform.GetOutputStreamInfo(0) }
            .map_err(|error| error.to_string())?;

        let (nal_length_size, sequence_header_annex_b) =
            read_sequence_header(&transform).map_err(|error| error.to_string())?;

        Ok(Self {
            _com_guard: com_guard,
            _mf_guard: mf_guard,
            transform,
            output_stream_info,
            width,
            height,
            frame_duration_100ns: 10_000_000_i64 / i64::from(profile.target_fps().max(1)),
            next_sample_time_100ns: 0,
            sequence_header_annex_b,
            nal_length_size,
            pending_packets: Vec::new(),
            prepend_sequence_header: true,
        })
    }

    pub fn push_frame(&mut self, image: &RgbaImage) -> std::result::Result<(), String> {
        let nv12 = rgba_to_nv12(image);
        let sample = create_input_sample(
            &nv12,
            self.next_sample_time_100ns,
            self.frame_duration_100ns,
        )
        .map_err(|error| error.to_string())?;

        loop {
            let result = unsafe { self.transform.ProcessInput(0, &sample, 0) };
            match result {
                Ok(()) => {
                    self.next_sample_time_100ns += self.frame_duration_100ns;
                    self.drain_output().map_err(|error| error.to_string())?;
                    return Ok(());
                }
                Err(error) if error.code() == MF_E_NOTACCEPTING => {
                    self.drain_output().map_err(|error| error.to_string())?;
                }
                Err(error) => return Err(error.to_string()),
            }
        }
    }

    pub fn drain_packets(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.pending_packets)
    }

    pub fn matches(&self, width: u32, height: u32) -> bool {
        self.width == width && self.height == height
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn drain_output(&mut self) -> WinResult<()> {
        loop {
            let sample = create_output_sample(self.output_stream_info.cbSize.max(4096))?;
            let output = MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(Some(sample.clone())),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            };
            let mut status = 0;
            let result = unsafe { self.transform.ProcessOutput(0, &mut [output], &mut status) };
            match result {
                Ok(()) => {
                    let bytes = contiguous_sample_bytes(&sample)?;
                    if bytes.is_empty() {
                        continue;
                    }
                    let mut annex_b = avcc_to_annex_b(&bytes, self.nal_length_size);
                    if annex_b.is_empty() {
                        annex_b = bytes;
                    }
                    if self.prepend_sequence_header && !self.sequence_header_annex_b.is_empty() {
                        let mut with_header =
                            Vec::with_capacity(self.sequence_header_annex_b.len() + annex_b.len());
                        with_header.extend_from_slice(&self.sequence_header_annex_b);
                        with_header.extend_from_slice(&annex_b);
                        annex_b = with_header;
                        self.prepend_sequence_header = false;
                    } else if contains_idr(&annex_b) && !self.sequence_header_annex_b.is_empty() {
                        let mut with_header =
                            Vec::with_capacity(self.sequence_header_annex_b.len() + annex_b.len());
                        with_header.extend_from_slice(&self.sequence_header_annex_b);
                        with_header.extend_from_slice(&annex_b);
                        annex_b = with_header;
                    }
                    self.pending_packets.push(annex_b);
                }
                Err(error) if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                Err(error) => return Err(error),
            }
        }
    }
}

impl Drop for H264EncoderSession {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
        }
    }
}

struct ComGuard;

impl ComGuard {
    fn new() -> WinResult<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
        Ok(Self)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct MfGuard;

impl MfGuard {
    fn new() -> WinResult<Self> {
        unsafe { MFStartup(MF_VERSION, 0) }?;
        Ok(Self)
    }
}

impl Drop for MfGuard {
    fn drop(&mut self) {
        let _ = unsafe { MFShutdown() };
    }
}

fn configure_output_type(
    transform: &IMFTransform,
    width: u32,
    height: u32,
    profile: StreamProfile,
) -> WinResult<()> {
    // Configure a broadly compatible baseline software H.264 output.
    let media_type: IMFMediaType = unsafe { MFCreateMediaType()? };
    unsafe {
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &windows::Win32::Media::MediaFoundation::MFMediaType_Video)?;
        media_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        media_type.SetUINT32(&MF_MT_AVG_BITRATE, parse_bitrate(profile.target_maxrate())?)?;
        media_type.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_ConstrainedBase.0 as u32)?;
        media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        media_type.SetUINT64(&MF_MT_FRAME_SIZE, pack_u32_pair(width, height))?;
        media_type.SetUINT64(&MF_MT_FRAME_RATE, pack_u32_pair(profile.target_fps(), 1))?;
        media_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_u32_pair(1, 1))?;
        transform.SetOutputType(0, &media_type, 0)?;
    }
    Ok(())
}

fn apply_software_compat_settings(transform: &IMFTransform, profile: StreamProfile) -> WinResult<()> {
    let codec_api: ICodecAPI = transform.cast()?;
    unsafe {
        let _ = codec_api.SetAllDefaults();
        set_codec_value(&codec_api, &CODECAPI_AVLowLatencyMode, true.into());
        set_codec_value(&codec_api, &CODECAPI_AVEncCommonLowLatency, true.into());
        set_codec_value(&codec_api, &CODECAPI_AVRealtimeControl, true.into());
        set_codec_value(
            &codec_api,
            &CODECAPI_AVEncCommonRateControlMode,
            (eAVEncCommonRateControlMode_CBR.0).into(),
        );
        set_codec_value(
            &codec_api,
            &CODECAPI_AVEncCommonMeanBitRate,
            parse_bitrate(profile.target_maxrate())?.into(),
        );
        set_codec_value(
            &codec_api,
            &CODECAPI_AVEncCommonMaxBitRate,
            parse_bitrate(profile.target_maxrate())?.into(),
        );
        set_codec_value(
            &codec_api,
            &CODECAPI_AVEncVideoMaxKeyframeDistance,
            profile.target_fps().into(),
        );
    }
    Ok(())
}

unsafe fn set_codec_value(codec_api: &ICodecAPI, key: &windows::core::GUID, value: VARIANT) {
    let _ = unsafe { codec_api.SetValue(key, &value as *const _) };
}

fn configure_input_type(
    transform: &IMFTransform,
    width: u32,
    height: u32,
    profile: StreamProfile,
) -> WinResult<()> {
    let media_type: IMFMediaType = unsafe { MFCreateMediaType()? };
    unsafe {
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &windows::Win32::Media::MediaFoundation::MFMediaType_Video)?;
        media_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        media_type.SetUINT64(&MF_MT_FRAME_SIZE, pack_u32_pair(width, height))?;
        media_type.SetUINT64(&MF_MT_FRAME_RATE, pack_u32_pair(profile.target_fps(), 1))?;
        media_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_u32_pair(1, 1))?;
        transform.SetInputType(0, &media_type, 0)?;
    }
    Ok(())
}

fn create_input_sample(bytes: &[u8], sample_time: i64, sample_duration: i64) -> WinResult<IMFSample> {
    let sample = unsafe { MFCreateSample()? };
    let buffer = unsafe { MFCreateMemoryBuffer(bytes.len() as u32)? };
    write_buffer(&buffer, bytes)?;
    unsafe {
        sample.AddBuffer(&buffer)?;
        sample.SetSampleTime(sample_time)?;
        sample.SetSampleDuration(sample_duration)?;
    }
    Ok(sample)
}

fn create_output_sample(capacity: u32) -> WinResult<IMFSample> {
    let sample = unsafe { MFCreateSample()? };
    let buffer = unsafe { MFCreateMemoryBuffer(capacity)? };
    unsafe {
        sample.AddBuffer(&buffer)?;
    }
    Ok(sample)
}

fn contiguous_sample_bytes(sample: &IMFSample) -> WinResult<Vec<u8>> {
    let buffer: IMFMediaBuffer = unsafe { sample.ConvertToContiguousBuffer()? };
    read_buffer(&buffer)
}

fn write_buffer(buffer: &IMFMediaBuffer, bytes: &[u8]) -> WinResult<()> {
    unsafe {
        let mut ptr = std::ptr::null_mut();
        let mut max_len = 0;
        let mut cur_len = 0;
        buffer.Lock(&mut ptr, Some(&mut max_len), Some(&mut cur_len))?;
        if max_len < bytes.len() as u32 {
            buffer.Unlock()?;
            return Err(Error::from_win32());
        }
        copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        buffer.SetCurrentLength(bytes.len() as u32)?;
        buffer.Unlock()?;
    }
    Ok(())
}

fn read_buffer(buffer: &IMFMediaBuffer) -> WinResult<Vec<u8>> {
    unsafe {
        let mut ptr = std::ptr::null_mut();
        let mut max_len = 0;
        let mut cur_len = 0;
        buffer.Lock(&mut ptr, Some(&mut max_len), Some(&mut cur_len))?;
        let mut bytes = vec![0_u8; cur_len as usize];
        copy_nonoverlapping(ptr, bytes.as_mut_ptr(), cur_len as usize);
        buffer.Unlock()?;
        Ok(bytes)
    }
}

fn read_sequence_header(transform: &IMFTransform) -> WinResult<(usize, Vec<u8>)> {
    let output_type = unsafe { transform.GetOutputCurrentType(0)? };
    let blob_size = unsafe { output_type.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER)? };
    if blob_size == 0 {
        return Ok((4, Vec::new()));
    }
    let mut blob = vec![0_u8; blob_size as usize];
    let mut written = 0;
    unsafe {
        output_type.GetBlob(
            &MF_MT_MPEG_SEQUENCE_HEADER,
            blob.as_mut_slice(),
            Some(&mut written),
        )?;
    }
    blob.truncate(written as usize);
    Ok(parse_avcc_sequence_header(&blob))
}

fn parse_avcc_sequence_header(blob: &[u8]) -> (usize, Vec<u8>) {
    if blob.len() < 7 || blob[0] != 1 {
        return (4, blob.to_vec());
    }

    let nal_length_size = usize::from((blob[4] & 0x03) + 1);
    let mut offset = 5usize;
    let num_sps = usize::from(blob[offset] & 0x1f);
    offset += 1;
    let mut annex_b = Vec::new();
    for _ in 0..num_sps {
        if offset + 2 > blob.len() {
            return (nal_length_size, annex_b);
        }
        let len = u16::from_be_bytes([blob[offset], blob[offset + 1]]) as usize;
        offset += 2;
        if offset + len > blob.len() {
            return (nal_length_size, annex_b);
        }
        annex_b.extend_from_slice(&[0, 0, 0, 1]);
        annex_b.extend_from_slice(&blob[offset..offset + len]);
        offset += len;
    }
    if offset >= blob.len() {
        return (nal_length_size, annex_b);
    }
    let num_pps = usize::from(blob[offset]);
    offset += 1;
    for _ in 0..num_pps {
        if offset + 2 > blob.len() {
            return (nal_length_size, annex_b);
        }
        let len = u16::from_be_bytes([blob[offset], blob[offset + 1]]) as usize;
        offset += 2;
        if offset + len > blob.len() {
            return (nal_length_size, annex_b);
        }
        annex_b.extend_from_slice(&[0, 0, 0, 1]);
        annex_b.extend_from_slice(&blob[offset..offset + len]);
        offset += len;
    }
    (nal_length_size, annex_b)
}

fn avcc_to_annex_b(bytes: &[u8], nal_length_size: usize) -> Vec<u8> {
    if bytes.starts_with(&[0, 0, 0, 1]) || bytes.starts_with(&[0, 0, 1]) {
        return bytes.to_vec();
    }
    if !(1..=4).contains(&nal_length_size) {
        return Vec::new();
    }

    let mut offset = 0usize;
    let mut annex_b = Vec::with_capacity(bytes.len() + 64);
    while offset + nal_length_size <= bytes.len() {
        let mut nal_len = 0usize;
        for &byte in &bytes[offset..offset + nal_length_size] {
            nal_len = (nal_len << 8) | usize::from(byte);
        }
        offset += nal_length_size;
        if nal_len == 0 || offset + nal_len > bytes.len() {
            return Vec::new();
        }
        annex_b.extend_from_slice(&[0, 0, 0, 1]);
        annex_b.extend_from_slice(&bytes[offset..offset + nal_len]);
        offset += nal_len;
    }
    annex_b
}

fn contains_idr(bytes: &[u8]) -> bool {
    let mut i = 0usize;
    while i + 4 < bytes.len() {
        if bytes[i..].starts_with(&[0, 0, 0, 1]) {
            let nal_type = bytes[i + 4] & 0x1f;
            if nal_type == 5 {
                return true;
            }
            i += 4;
        } else if bytes[i..].starts_with(&[0, 0, 1]) {
            let nal_type = bytes[i + 3] & 0x1f;
            if nal_type == 5 {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}

fn parse_bitrate(value: &str) -> WinResult<u32> {
    let trimmed = value.trim_end_matches('k');
    let kbps = trimmed.parse::<u32>().map_err(|_| Error::from_win32())?;
    Ok(kbps.saturating_mul(1000))
}

fn pack_u32_pair(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

fn rgba_to_nv12(image: &RgbaImage) -> Vec<u8> {
    let width = image.width() as usize;
    let height = image.height() as usize;
    let rgba = image.as_raw();
    let mut nv12 = vec![0_u8; width * height + (width * height) / 2];
    let (y_plane, uv_plane) = nv12.split_at_mut(width * height);

    for y in (0..height).step_by(2) {
        for x in (0..width).step_by(2) {
            let mut u_acc = 0.0f32;
            let mut v_acc = 0.0f32;
            let mut samples = 0.0f32;
            for dy in 0..2 {
                for dx in 0..2 {
                    let px = x + dx;
                    let py = y + dy;
                    if px >= width || py >= height {
                        continue;
                    }
                    let idx = (py * width + px) * 4;
                    let r = rgba[idx] as f32;
                    let g = rgba[idx + 1] as f32;
                    let b = rgba[idx + 2] as f32;
                    let y_val = (0.257 * r + 0.504 * g + 0.098 * b + 16.0).round();
                    let u_val = (-0.148 * r - 0.291 * g + 0.439 * b + 128.0).round();
                    let v_val = (0.439 * r - 0.368 * g - 0.071 * b + 128.0).round();
                    y_plane[py * width + px] = y_val.clamp(0.0, 255.0) as u8;
                    u_acc += u_val;
                    v_acc += v_val;
                    samples += 1.0;
                }
            }
            let uv_index = (y / 2) * width + x;
            uv_plane[uv_index] = (u_acc / samples).clamp(0.0, 255.0) as u8;
            uv_plane[uv_index + 1] = (v_acc / samples).clamp(0.0, 255.0) as u8;
        }
    }

    nv12
}
