use super::{CaptureConfig, CaptureError, CapturedFrame, ScreenCapture};
use crate::sync::clock::SyncClock;
use std::time::{Duration, Instant};
use win_desktop_duplication::devices::AdapterFactory;
use win_desktop_duplication::tex_reader::TextureReader;
use win_desktop_duplication::{DesktopDuplicationApi, OutputDuplication};

/// Windows screen capture using DXGI Desktop Duplication API.
///
/// Captures the primary monitor's framebuffer at the configured FPS.
/// Frames arrive in BGRA format from DXGI and are converted to RGBA.
pub struct WindowsCapture {
    clock: SyncClock,
    config: Option<CaptureConfig>,
    running: bool,
    duplication: Option<OutputDuplication>,
    reader: Option<TextureReader>,
    last_frame_time: Option<Instant>,
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
        }
    }

    /// Initialize or reinitialize the DXGI duplication session.
    fn init_duplication(&mut self) -> Result<(), CaptureError> {
        let adapter = AdapterFactory::new()
            .get_adapter_by_idx(0)
            .map_err(|e| CaptureError::Platform(format!("Failed to get adapter: {e}")))?;

        let output = adapter
            .get_display_by_idx(0)
            .map_err(|e| CaptureError::Platform(format!("Failed to get display: {e}")))?;

        let duplication = DesktopDuplicationApi::new(adapter, output.clone())
            .map_err(|e| CaptureError::Platform(format!("Failed to create duplication: {e}")))?;

        let reader = TextureReader::new(duplication.get_device(), duplication.get_context());

        self.duplication = Some(duplication);
        self.reader = Some(reader);
        Ok(())
    }

    /// Convert BGRA pixel data to RGBA in-place.
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

        // Non-blocking acquire (0ms timeout)
        let texture = match duplication.acquire_next_frame_now() {
            Ok(tex) => tex,
            Err(e) => {
                let err_msg = format!("{e}");
                // Handle AccessLost by reinitializing
                if err_msg.contains("AccessLost") || err_msg.contains("DXGI_ERROR_ACCESS_LOST") {
                    eprintln!("[GameClip] DXGI access lost, reinitializing...");
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
        let mut data = reader.get_data(&mut duplication.get_device(), &texture);

        if data.is_empty() {
            return Ok(None);
        }

        // Use configured dimensions; DXGI delivers at native resolution
        // so config should always specify width/height.
        let total_pixels = data.len() / 4;
        let (width, height) = if config.width > 0 && config.height > 0 {
            (config.width, config.height)
        } else if total_pixels > 0 {
            // Infer assuming 16:9 aspect ratio
            let w = ((total_pixels as f64 * 16.0 / 9.0).sqrt()).round() as u32;
            let h = total_pixels as u32 / w.max(1);
            (w, h)
        } else {
            return Ok(None);
        };

        Self::bgra_to_rgba(&mut data);

        self.last_frame_time = Some(Instant::now());

        Ok(Some(CapturedFrame {
            timestamp_us: self.clock.now_us(),
            width,
            height,
            data,
        }))
    }
}
