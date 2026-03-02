use super::{CaptureConfig, CaptureError, CapturedFrame, FramePixelFormat, ScreenCapture};
use crate::sync::clock::SyncClock;
use log::{error, info, warn};
use std::time::{Duration, Instant};
use win_desktop_duplication::devices::AdapterFactory;
use win_desktop_duplication::tex_reader::TextureReader;
use win_desktop_duplication::DesktopDuplicationApi;

use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::Interface;

/// Number of staging textures in the round-robin pool.
/// While slot N is being read by CPU, GPU copies into slot N+1.
const STAGING_POOL_SIZE: usize = 3;

/// D3D11 VideoProcessor state for GPU-side BGRA→NV12 conversion.
struct VideoProcessorState {
    video_context: ID3D11VideoContext,
    video_processor: ID3D11VideoProcessor,
    enumerator: ID3D11VideoProcessorEnumerator,
    nv12_output: ID3D11Texture2D,
    /// Round-robin staging texture pool for pipelined GPU→CPU readback.
    staging_pool: Vec<ID3D11Texture2D>,
    /// Current index into the staging pool.
    staging_index: usize,
    output_width: u32,
    output_height: u32,
}

/// Windows screen capture using DXGI Desktop Duplication API.
///
/// Captures the primary monitor's framebuffer. When a D3D11 VideoProcessor
/// is available, converts BGRA→NV12 on the GPU before CPU readback (62%
/// bandwidth reduction). Falls back to raw BGRA if VideoProcessor init fails.
pub struct WindowsCapture {
    clock: SyncClock,
    config: Option<CaptureConfig>,
    running: bool,
    duplication: Option<DesktopDuplicationApi>,
    reader: Option<TextureReader>,
    last_frame_time: Option<Instant>,
    /// Reusable buffer for frame data. Avoids per-frame allocation.
    frame_buffer: Vec<u8>,
    /// D3D11 VideoProcessor for GPU-side BGRA→NV12 conversion.
    /// None when fallback to BGRA readback (old GPU/driver).
    video_processor: Option<VideoProcessorState>,
    /// Whether NV12 GPU conversion is active. Determines pixel format tag.
    nv12_active: bool,
    /// Adapter index for multi-monitor support.
    adapter_index: u32,
    /// Display index for multi-monitor support.
    display_index: u32,
}

impl WindowsCapture {
    pub fn new(clock: SyncClock) -> Self {
        Self {
            clock,
            config: None,
            running: false,
            duplication: None,
            reader: None,
            last_frame_time: None,
            frame_buffer: Vec::new(),
            video_processor: None,
            nv12_active: false,
            adapter_index: 0,
            display_index: 0,
        }
    }

    /// Create a new capture targeting specific adapter and display.
    #[allow(dead_code)]
    pub fn with_display(clock: SyncClock, adapter_index: u32, display_index: u32) -> Self {
        Self {
            adapter_index,
            display_index,
            ..Self::new(clock)
        }
    }

    /// Initialize or reinitialize the DXGI duplication session.
    fn init_duplication(&mut self) -> Result<(), CaptureError> {
        win_desktop_duplication::set_process_dpi_awareness();
        win_desktop_duplication::co_init();

        let adapter = AdapterFactory::new()
            .get_adapter_by_idx(self.adapter_index)
            .ok_or_else(|| {
                CaptureError::Platform(format!(
                    "Failed to get adapter at index {}",
                    self.adapter_index,
                ))
            })?;

        let output = adapter
            .get_display_by_idx(self.display_index)
            .ok_or_else(|| {
                CaptureError::Platform(format!(
                    "Failed to get display at index {}",
                    self.display_index,
                ))
            })?;

        let duplication = DesktopDuplicationApi::new(adapter, output)
            .map_err(|e| CaptureError::Platform(format!("Failed to create duplication: {e:?}")))?;

        let (device, ctx) = duplication.get_device_and_ctx();
        let reader = TextureReader::new(device, ctx);

        self.duplication = Some(duplication);
        self.reader = Some(reader);
        Ok(())
    }

    /// Try to initialize the D3D11 VideoProcessor for GPU-side BGRA→NV12.
    ///
    /// Falls back gracefully: if VideoProcessor creation fails (old GPU/driver),
    /// capture continues with the BGRA readback path.
    fn try_init_video_processor(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<(), CaptureError> {
        let duplication = self
            .duplication
            .as_ref()
            .ok_or(CaptureError::NotStarted)?;
        let (device, device_ctx) = duplication.get_device_and_ctx();

        match create_video_processor(&device, &device_ctx, width, height) {
            Ok(state) => {
                info!(
                    "D3D11 VideoProcessor initialized: BGRA→NV12 GPU conversion active ({}x{})",
                    width, height,
                );
                self.video_processor = Some(state);
                self.nv12_active = true;
                Ok(())
            }
            Err(e) => {
                warn!(
                    "D3D11 VideoProcessor unavailable, falling back to BGRA readback: {e}"
                );
                self.nv12_active = false;
                Ok(())
            }
        }
    }

    /// Poll a frame using the NV12 GPU conversion path.
    ///
    /// BGRA texture → VideoProcessor → NV12 output → staging copy → CPU readback.
    /// Returns 3MB NV12 instead of 8MB BGRA at 1080p.
    fn poll_frame_nv12(
        &mut self,
        config: &CaptureConfig,
    ) -> Result<Option<CapturedFrame>, CaptureError> {
        let duplication = self
            .duplication
            .as_mut()
            .ok_or(CaptureError::NotStarted)?;

        let texture = match duplication.acquire_next_frame_now() {
            Ok(tex) => tex,
            Err(e) => {
                let err_msg = format!("{e:?}");
                if err_msg.contains("AccessLost") || err_msg.contains("DXGI_ERROR_ACCESS_LOST") {
                    warn!("DXGI access lost, reinitializing...");
                    if let Err(reinit_err) = self.init_duplication() {
                        return Err(CaptureError::Platform(format!(
                            "Reinit failed: {reinit_err}"
                        )));
                    }
                    // Re-init video processor after duplication re-init
                    let _ = self.try_init_video_processor(config.width, config.height);
                    return Ok(None);
                }
                return Ok(None);
            }
        };

        let vp = self
            .video_processor
            .as_mut()
            .ok_or(CaptureError::NotStarted)?;

        let duplication = self
            .duplication
            .as_ref()
            .ok_or(CaptureError::NotStarted)?;
        let (_device, device_ctx) = duplication.get_device_and_ctx();

        // Get the DXGI frame texture as ID3D11Texture2D.
        // win_desktop_duplication's Texture wraps an ID3D11Texture2D internally.
        let src_texture: ID3D11Texture2D = unsafe {
            let raw_texture = texture.as_raw_ref();
            raw_texture.cast().map_err(|e| {
                CaptureError::Platform(format!("Failed to cast to ID3D11Texture2D: {e}"))
            })?
        };

        // Run through VideoProcessor: BGRA input → NV12 output
        if let Err(e) = run_video_processor(
            &vp.video_context,
            &vp.video_processor,
            &vp.enumerator,
            &src_texture,
            &vp.nv12_output,
            vp.output_width,
            vp.output_height,
        ) {
            warn!("VideoProcessor blit failed: {e}");
            return Ok(None);
        }

        // Copy NV12 output to current staging slot
        let staging_idx = vp.staging_index;
        let staging = &vp.staging_pool[staging_idx];
        unsafe {
            device_ctx.CopyResource(staging, &vp.nv12_output);
        }
        vp.staging_index = (staging_idx + 1) % STAGING_POOL_SIZE;

        // Map and read NV12 data from staging texture
        let nv12_size = (vp.output_width * vp.output_height * 3 / 2) as usize;
        self.frame_buffer.clear();

        let mapped = unsafe {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            device_ctx
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| {
                    CaptureError::Platform(format!("Failed to map staging texture: {e}"))
                })?;
            mapped
        };

        // Copy NV12 data respecting row pitch. NV12 has two planes:
        // Y plane: height rows of width bytes
        // UV plane: height/2 rows of width bytes
        let row_pitch = mapped.RowPitch as usize;
        let width = vp.output_width as usize;
        let height = vp.output_height as usize;

        self.frame_buffer.reserve(nv12_size);
        unsafe {
            let src = mapped.pData as *const u8;
            // Y plane
            for row in 0..height {
                let row_start = src.add(row * row_pitch);
                let slice = std::slice::from_raw_parts(row_start, width);
                self.frame_buffer.extend_from_slice(slice);
            }
            // UV plane (starts after Y plane in the texture, at row offset `height`)
            for row in 0..(height / 2) {
                let row_start = src.add((height + row) * row_pitch);
                let slice = std::slice::from_raw_parts(row_start, width);
                self.frame_buffer.extend_from_slice(slice);
            }
        }

        unsafe {
            device_ctx.Unmap(staging, 0);
        }

        if self.frame_buffer.len() != nv12_size {
            warn!(
                "NV12 frame size mismatch: expected {nv12_size}, got {}",
                self.frame_buffer.len(),
            );
            return Ok(None);
        }

        self.last_frame_time = Some(Instant::now());

        Ok(Some(CapturedFrame {
            timestamp_us: self.clock.now_us(),
            width: vp.output_width,
            height: vp.output_height,
            data: self.frame_buffer.clone(),
            pixel_format: FramePixelFormat::Nv12,
        }))
    }

    /// Poll a frame using the legacy BGRA readback path.
    fn poll_frame_bgra(
        &mut self,
        config: &CaptureConfig,
    ) -> Result<Option<CapturedFrame>, CaptureError> {
        let duplication = self
            .duplication
            .as_mut()
            .ok_or(CaptureError::NotStarted)?;

        let texture = match duplication.acquire_next_frame_now() {
            Ok(tex) => tex,
            Err(e) => {
                let err_msg = format!("{e:?}");
                if err_msg.contains("AccessLost") || err_msg.contains("DXGI_ERROR_ACCESS_LOST") {
                    warn!("DXGI access lost, reinitializing...");
                    if let Err(reinit_err) = self.init_duplication() {
                        return Err(CaptureError::Platform(format!(
                            "Reinit failed: {reinit_err}"
                        )));
                    }
                    return Ok(None);
                }
                return Ok(None);
            }
        };

        let reader = self.reader.as_mut().ok_or(CaptureError::NotStarted)?;

        self.frame_buffer.clear();
        if let Err(e) = reader.get_data(&mut self.frame_buffer, &texture) {
            error!("TextureReader error: {e:?}");
            return Ok(None);
        }

        if self.frame_buffer.is_empty() {
            return Ok(None);
        }

        let total_pixels = self.frame_buffer.len() / 4;
        let (width, height) = if config.width > 0 && config.height > 0 {
            (config.width, config.height)
        } else if total_pixels > 0 {
            let w_est = (total_pixels as f64).sqrt();
            let candidates: &[(u32, u32)] = &[
                (16, 9), (16, 10), (4, 3), (21, 9),
            ];
            let mut best = (w_est.round() as u32, (total_pixels as f64 / w_est).round() as u32);
            for &(aw, ah) in candidates {
                let w = ((total_pixels as f64 * aw as f64 / ah as f64).sqrt()).round() as u32;
                let h = total_pixels as u32 / w.max(1);
                if (w as usize * h as usize) == total_pixels {
                    best = (w, h);
                    break;
                }
            }
            best
        } else {
            return Ok(None);
        };

        self.last_frame_time = Some(Instant::now());

        Ok(Some(CapturedFrame {
            timestamp_us: self.clock.now_us(),
            width,
            height,
            data: self.frame_buffer.clone(),
            pixel_format: FramePixelFormat::Bgra,
        }))
    }
}

impl ScreenCapture for WindowsCapture {
    fn start(&mut self, config: CaptureConfig) -> Result<(), CaptureError> {
        if self.running {
            return Err(CaptureError::AlreadyRunning);
        }

        self.init_duplication()?;

        // Try to initialize NV12 GPU conversion. Falls back to BGRA if it fails.
        let _ = self.try_init_video_processor(config.width, config.height);

        // Pre-allocate frame buffer based on configured dimensions and active format.
        let frame_bytes = if self.nv12_active {
            (config.width as usize * config.height as usize * 3) / 2
        } else {
            config.width as usize * config.height as usize * 4
        };
        if self.frame_buffer.capacity() < frame_bytes {
            self.frame_buffer = Vec::with_capacity(frame_bytes);
        }
        let format_label = if self.nv12_active { "NV12" } else { "BGRA" };
        info!(
            "Windows capture pre-allocated {}MB frame buffer ({format_label})",
            frame_bytes / (1024 * 1024),
        );

        self.config = Some(config);
        self.running = true;
        self.last_frame_time = None;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), CaptureError> {
        if !self.running {
            return Err(CaptureError::NotStarted);
        }
        self.running = false;
        self.duplication = None;
        self.reader = None;
        self.video_processor = None;
        self.nv12_active = false;
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn poll_frame(&mut self) -> Result<Option<CapturedFrame>, CaptureError> {
        if !self.running {
            return Err(CaptureError::NotStarted);
        }

        let config = self.config.as_ref().ok_or(CaptureError::NotStarted)?.clone();

        // Rate limiting: skip if not enough time has elapsed since last frame
        let frame_interval = Duration::from_secs_f64(1.0 / config.target_fps as f64);
        if let Some(last) = self.last_frame_time {
            if last.elapsed() < frame_interval {
                return Ok(None);
            }
        }

        if self.nv12_active {
            self.poll_frame_nv12(&config)
        } else {
            self.poll_frame_bgra(&config)
        }
    }

    /// Returns whether NV12 GPU conversion is active.
    fn nv12_active(&self) -> bool {
        self.nv12_active
    }
}

// ---------------------------------------------------------------------------
// D3D11 VideoProcessor helpers
// ---------------------------------------------------------------------------

/// Create the D3D11 VideoProcessor, NV12 output texture, and staging pool.
fn create_video_processor(
    device: &ID3D11Device,
    device_ctx: &ID3D11DeviceContext,
    width: u32,
    height: u32,
) -> Result<VideoProcessorState, String> {
    unsafe {
        // Get ID3D11VideoDevice from the D3D device
        let video_device: ID3D11VideoDevice = device
            .cast()
            .map_err(|e| format!("ID3D11VideoDevice cast failed: {e}"))?;

        // Get ID3D11VideoContext from the device context
        let video_context: ID3D11VideoContext = device_ctx
            .cast()
            .map_err(|e| format!("ID3D11VideoContext cast failed: {e}"))?;

        // Create VideoProcessorEnumerator describing the conversion
        let content_desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputWidth: width,
            InputHeight: height,
            OutputWidth: width,
            OutputHeight: height,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
            ..Default::default()
        };

        let enumerator = video_device
            .CreateVideoProcessorEnumerator(&content_desc)
            .map_err(|e| format!("CreateVideoProcessorEnumerator failed: {e}"))?;

        // Create VideoProcessor (rate conversion index 0 is always available)
        let video_processor = video_device
            .CreateVideoProcessor(&enumerator, 0)
            .map_err(|e| format!("CreateVideoProcessor failed: {e}"))?;

        // Create NV12 output texture (GPU default usage)
        let output_desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET,
            CPUAccessFlags: D3D11_CPU_ACCESS_FLAG(0),
            MiscFlags: D3D11_RESOURCE_MISC_FLAG(0),
        };
        let nv12_output = device
            .CreateTexture2D(&output_desc, None)
            .map_err(|e| format!("Failed to create NV12 output texture: {e}"))?;

        // Create staging texture pool (CPU-readable, for round-robin readback)
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: D3D11_BIND_FLAG(0),
            CPUAccessFlags: D3D11_CPU_ACCESS_READ,
            MiscFlags: D3D11_RESOURCE_MISC_FLAG(0),
        };

        let mut staging_pool = Vec::with_capacity(STAGING_POOL_SIZE);
        for _ in 0..STAGING_POOL_SIZE {
            let staging = device
                .CreateTexture2D(&staging_desc, None)
                .map_err(|e| format!("Failed to create NV12 staging texture: {e}"))?;
            staging_pool.push(staging);
        }

        info!(
            "D3D11 VideoProcessor created: {}x{}, {} staging textures",
            width, height, STAGING_POOL_SIZE,
        );

        Ok(VideoProcessorState {
            video_context,
            video_processor,
            enumerator,
            nv12_output,
            staging_pool,
            staging_index: 0,
            output_width: width,
            output_height: height,
        })
    }
}

/// Run a frame through the VideoProcessor: BGRA input → NV12 output.
fn run_video_processor(
    video_context: &ID3D11VideoContext,
    video_processor: &ID3D11VideoProcessor,
    enumerator: &ID3D11VideoProcessorEnumerator,
    input_texture: &ID3D11Texture2D,
    output_texture: &ID3D11Texture2D,
    width: u32,
    height: u32,
) -> Result<(), String> {
    unsafe {
        // Get the parent video device from the enumerator
        let video_device: ID3D11VideoDevice = {
            let mut device_ptr = None;
            enumerator.GetDevice(&mut device_ptr);
            let device: ID3D11Device = device_ptr
                .ok_or("GetDevice returned None")?;
            device.cast().map_err(|e| format!("Cast to VideoDevice: {e}"))?
        };

        // Create input view (BGRA)
        let input_view_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: 0,
                },
            },
        };
        let input_view = video_device
            .CreateVideoProcessorInputView(input_texture, &enumerator, &input_view_desc)
            .map_err(|e| format!("CreateVideoProcessorInputView failed: {e}"))?;

        // Create output view (NV12)
        let output_view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let output_view = video_device
            .CreateVideoProcessorOutputView(output_texture, &enumerator, &output_view_desc)
            .map_err(|e| format!("CreateVideoProcessorOutputView failed: {e}"))?;

        // Set up source and dest rectangles
        let src_rect = windows::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        video_context.VideoProcessorSetStreamSourceRect(video_processor, 0, true, &src_rect);
        video_context.VideoProcessorSetStreamDestRect(video_processor, 0, true, &src_rect);
        video_context.VideoProcessorSetOutputTargetRect(video_processor, true, &src_rect);

        // Build input stream
        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            OutputIndex: 0,
            InputFrameOrField: 0,
            PastFrames: 0,
            FutureFrames: 0,
            ppPastSurfaces: std::ptr::null_mut(),
            pInputSurface: std::mem::ManuallyDrop::new(Some(input_view)),
            ppFutureSurfaces: std::ptr::null_mut(),
            ..Default::default()
        };

        video_context
            .VideoProcessorBlt(video_processor, &output_view, 0, &[stream])
            .map_err(|e| format!("VideoProcessorBlt failed: {e}"))?;

        Ok(())
    }
}

/// Enumerate available adapters and their displays.
/// Returns a list of (adapter_name, Vec<display_name>) pairs.
#[allow(dead_code)]
pub fn enumerate_displays() -> Vec<(String, Vec<String>)> {
    win_desktop_duplication::set_process_dpi_awareness();
    win_desktop_duplication::co_init();

    let factory = AdapterFactory::new();
    let mut result = Vec::new();

    for adapter_idx in 0..8 {
        let adapter = match factory.get_adapter_by_idx(adapter_idx) {
            Some(a) => a,
            None => break,
        };
        let adapter_name = format!("Adapter {adapter_idx}");
        let mut displays = Vec::new();

        for display_idx in 0..8 {
            if adapter.get_display_by_idx(display_idx).is_some() {
                displays.push(format!("Display {display_idx}"));
            } else {
                break;
            }
        }

        if !displays.is_empty() {
            result.push((adapter_name, displays));
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nv12_frame_size_calculation() {
        // NV12 = width * height * 3/2
        let width: u32 = 1920;
        let height: u32 = 1080;
        let expected = (width * height * 3 / 2) as usize;
        assert_eq!(expected, 3_110_400);
        // Compare with BGRA
        let bgra_size = (width * height * 4) as usize;
        assert_eq!(bgra_size, 8_294_400);
        // NV12 is ~37.5% of BGRA (62.5% reduction)
        let ratio = expected as f64 / bgra_size as f64;
        assert!((ratio - 0.375).abs() < 0.01);
    }

    #[test]
    fn staging_pool_size_is_reasonable() {
        assert!(STAGING_POOL_SIZE >= 2, "Need at least 2 for pipelining");
        assert!(STAGING_POOL_SIZE <= 5, "More than 5 wastes GPU memory");
    }

    #[test]
    fn with_display_sets_indices() {
        let clock = SyncClock::new();
        let capture = WindowsCapture::with_display(clock, 1, 2);
        assert_eq!(capture.adapter_index, 1);
        assert_eq!(capture.display_index, 2);
    }

    #[test]
    fn default_capture_uses_primary() {
        let clock = SyncClock::new();
        let capture = WindowsCapture::new(clock);
        assert_eq!(capture.adapter_index, 0);
        assert_eq!(capture.display_index, 0);
        assert!(!capture.nv12_active);
    }
}
