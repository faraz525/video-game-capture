use super::{CaptureConfig, CaptureError, CapturedFrame, FramePixelFormat, ScreenCapture};
use crate::sync::clock::SyncClock;
use log::{error, info, warn};
use std::time::{Duration, Instant};
use win_desktop_duplication::devices::AdapterFactory;
use win_desktop_duplication::tex_reader::TextureReader;
use win_desktop_duplication::DesktopDuplicationApi;

/// Windows screen capture using DXGI Desktop Duplication API.
///
/// Captures the primary monitor's framebuffer at the configured FPS.
/// Frames arrive in BGRA format from DXGI.
pub struct WindowsCapture {
    clock: SyncClock,
    config: Option<CaptureConfig>,
    running: bool,
    duplication: Option<DesktopDuplicationApi>,
    reader: Option<TextureReader>,
    last_frame_time: Option<Instant>,
    /// Reusable buffer for frame data. Avoids per-frame allocation of ~8MB.
    /// `clear()` resets length to 0 but retains capacity across frames.
    frame_buffer: Vec<u8>,
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
        }
    }

    /// Initialize or reinitialize the DXGI duplication session.
    fn init_duplication(&mut self) -> Result<(), CaptureError> {
        win_desktop_duplication::set_process_dpi_awareness();
        win_desktop_duplication::co_init();

        let adapter = AdapterFactory::new()
            .get_adapter_by_idx(0)
            .ok_or_else(|| CaptureError::Platform("Failed to get adapter".to_string()))?;

        let output = adapter
            .get_display_by_idx(0)
            .ok_or_else(|| CaptureError::Platform("Failed to get display".to_string()))?;

        let duplication = DesktopDuplicationApi::new(adapter, output)
            .map_err(|e| CaptureError::Platform(format!("Failed to create duplication: {e:?}")))?;

        let (device, ctx) = duplication.get_device_and_ctx();
        let reader = TextureReader::new(device, ctx);

        self.duplication = Some(duplication);
        self.reader = Some(reader);
        Ok(())
    }

    /// Convert BGRA pixel data to RGBA in-place.
    /// Retained for potential future use (e.g., raw RGBA fallback path).
    #[allow(dead_code)]
    fn bgra_to_rgba(data: &mut [u8]) {
        for pixel in data.chunks_exact_mut(4) {
            pixel.swap(0, 2); // swap B and R
        }
    }
}

impl ScreenCapture for WindowsCapture {
    fn start(&mut self, config: CaptureConfig) -> Result<(), CaptureError> {
        if self.running {
            return Err(CaptureError::AlreadyRunning);
        }

        self.init_duplication()?;

        // Pre-allocate frame buffer based on configured dimensions.
        // At 1080p BGRA this is ~8MB — allocated once, reused every frame.
        let frame_bytes = config.width as usize * config.height as usize * 4;
        if self.frame_buffer.capacity() < frame_bytes {
            self.frame_buffer = Vec::with_capacity(frame_bytes);
        }
        info!(
            "Windows capture pre-allocated {}MB frame buffer",
            frame_bytes / (1024 * 1024)
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
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn poll_frame(&mut self) -> Result<Option<CapturedFrame>, CaptureError> {
        if !self.running {
            return Err(CaptureError::NotStarted);
        }

        let config = self.config.as_ref().ok_or(CaptureError::NotStarted)?;

        // Rate limiting: skip if not enough time has elapsed since last frame
        let frame_interval = Duration::from_secs_f64(1.0 / config.target_fps as f64);
        if let Some(last) = self.last_frame_time {
            if last.elapsed() < frame_interval {
                return Ok(None);
            }
        }

        let duplication = self
            .duplication
            .as_mut()
            .ok_or(CaptureError::NotStarted)?;

        // Non-blocking acquire
        let texture = match duplication.acquire_next_frame_now() {
            Ok(tex) => tex,
            Err(e) => {
                let err_msg = format!("{e:?}");
                // Handle AccessLost by reinitializing
                if err_msg.contains("AccessLost") || err_msg.contains("DXGI_ERROR_ACCESS_LOST") {
                    warn!("DXGI access lost, reinitializing...");
                    if let Err(reinit_err) = self.init_duplication() {
                        return Err(CaptureError::Platform(format!(
                            "Reinit failed: {reinit_err}"
                        )));
                    }
                    return Ok(None);
                }
                // Timeout / no new frame is normal
                return Ok(None);
            }
        };

        let reader = self.reader.as_mut().ok_or(CaptureError::NotStarted)?;

        // Reuse the pre-allocated buffer: clear resets len to 0, keeps capacity.
        self.frame_buffer.clear();
        if let Err(e) = reader.get_data(&mut self.frame_buffer, &texture) {
            error!("TextureReader error: {e:?}");
            return Ok(None);
        }

        if self.frame_buffer.is_empty() {
            return Ok(None);
        }

        // Determine frame dimensions.
        // Primary: use configured dimensions from CaptureConfig.
        // Fallback: derive from pixel count (BGRA = 4 bytes/pixel).
        let total_pixels = self.frame_buffer.len() / 4;
        let (width, height) = if config.width > 0 && config.height > 0 {
            (config.width, config.height)
        } else if total_pixels > 0 {
            // Derive from data size. For non-16:9 monitors, assume square-ish
            // and compute based on actual pixel count.
            let w_est = (total_pixels as f64).sqrt();
            // Try common aspect ratios: 16:9, 16:10, 4:3, 21:9
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

        // Clone the data from the reusable buffer into the frame.
        // This is the one unavoidable copy per frame — the buffer must be
        // reusable for the next poll_frame call while the frame is consumed
        // by the ring buffer.
        Ok(Some(CapturedFrame {
            timestamp_us: self.clock.now_us(),
            width,
            height,
            data: self.frame_buffer.clone(),
            pixel_format: FramePixelFormat::Bgra,
        }))
    }
}
