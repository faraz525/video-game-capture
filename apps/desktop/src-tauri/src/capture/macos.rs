use super::{CaptureConfig, CaptureError, CapturedFrame, ScreenCapture};
use crate::sync::clock::SyncClock;
use log::{error, info, warn};
use screencapturekit::prelude::*;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const MAX_BUFFERED_FRAMES: usize = 10;

/// macOS screen capture using Apple's ScreenCaptureKit framework.
///
/// ScreenCaptureKit delivers frames via callbacks on a dispatch queue.
/// This implementation bridges to the polling-based `ScreenCapture` trait
/// using a shared `Arc<Mutex<VecDeque>>` buffer: the callback pushes,
/// `poll_frame()` pops.
pub struct MacOSCapture {
    clock: SyncClock,
    config: Option<CaptureConfig>,
    running: bool,
    stream: Option<SendableStream>,
    frame_buffer: Arc<Mutex<VecDeque<CapturedFrame>>>,
}

/// Wrapper to make SCStream sendable across threads.
///
/// SCStream is only accessed from the capture thread after being moved there,
/// so this is safe. The callback handler uses its own Arc<Mutex> buffer.
struct SendableStream(SCStream);

unsafe impl Send for SendableStream {}

/// Callback handler that receives frames from ScreenCaptureKit's dispatch queue
/// and pushes them into the shared buffer.
struct FrameHandler {
    buffer: Arc<Mutex<VecDeque<CapturedFrame>>>,
    clock: SyncClock,
    width: u32,
    height: u32,
}

impl SCStreamOutputTrait for FrameHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
        if !matches!(of_type, SCStreamOutputType::Screen) {
            return;
        }

        let pixel_buffer = match sample.get_image_buffer() {
            Some(pb) => pb,
            None => return,
        };

        let src_width = pixel_buffer.width();
        let src_height = pixel_buffer.height();
        let bytes_per_row = pixel_buffer.bytes_per_row();

        let guard = match pixel_buffer.lock_base_address(true) {
            Ok(g) => g,
            Err(e) => {
                warn!("Failed to lock pixel buffer: {e:?}");
                return;
            }
        };

        let base_ptr = guard.get_base_address();
        if base_ptr.is_null() {
            warn!("Pixel buffer base address is null");
            return;
        }
        let total_bytes = bytes_per_row * src_height;
        let slice = unsafe { std::slice::from_raw_parts(base_ptr, total_bytes) };

        let width = self.width;
        let height = self.height;

        if src_width < width as usize || src_height < height as usize {
            warn!(
                "Frame too small: {}x{} < {}x{}",
                src_width, src_height, width, height
            );
            return;
        }

        // Copy pixel data, stripping any row padding from GPU alignment.
        // ScreenCaptureKit delivers BGRA; we convert to RGBA in-place.
        let row_bytes = (width as usize) * 4;
        let mut data = Vec::with_capacity(row_bytes * height as usize);

        for y in 0..height as usize {
            let row_start = y * bytes_per_row;
            let row_end = row_start + row_bytes;
            if row_end > slice.len() {
                break;
            }
            data.extend_from_slice(&slice[row_start..row_end]);
        }

        // BGRA → RGBA: swap B and R channels
        for pixel in data.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }

        let frame = CapturedFrame {
            timestamp_us: self.clock.now_us(),
            width,
            height,
            data,
        };

        if let Ok(mut buf) = self.buffer.lock() {
            // Cap buffer size to prevent unbounded memory growth
            while buf.len() >= MAX_BUFFERED_FRAMES {
                buf.pop_front();
            }
            buf.push_back(frame);
        }
    }
}

impl MacOSCapture {
    pub fn new(clock: SyncClock) -> Self {
        Self {
            clock,
            config: None,
            running: false,
            stream: None,
            frame_buffer: Arc::new(Mutex::new(VecDeque::new())),
        }
    }
}

impl ScreenCapture for MacOSCapture {
    fn start(&mut self, config: CaptureConfig) -> Result<(), CaptureError> {
        if self.running {
            return Err(CaptureError::AlreadyRunning);
        }

        // Get shareable content (triggers permission prompt on first call)
        let content = SCShareableContent::get()
            .map_err(|e| CaptureError::Platform(format!("Failed to get shareable content: {e:?}")))?;

        let displays = content.displays();
        let display = displays
            .first()
            .ok_or_else(|| CaptureError::Platform("No displays found".to_string()))?;

        info!(
            "Capturing display: {}x{} (configured: {}x{} @ {}fps)",
            display.width(),
            display.height(),
            config.width,
            config.height,
            config.target_fps
        );

        // Use configured dimensions, or fall back to display native size.
        // Always pass explicit dimensions to avoid getting 2x Retina frames.
        let width = if config.width > 0 {
            config.width
        } else {
            display.width()
        };
        let height = if config.height > 0 {
            config.height
        } else {
            display.height()
        };

        let filter = SCContentFilter::builder()
            .display(display)
            .exclude_windows(&[])
            .build();

        let stream_config = SCStreamConfiguration::builder()
            .width(width)
            .height(height)
            .minimum_frame_interval(1, config.target_fps as i32)
            .pixel_format(PixelFormat::BGRA)
            .shows_cursor(true)
            .build();

        let handler = FrameHandler {
            buffer: Arc::clone(&self.frame_buffer),
            clock: self.clock.clone(),
            width,
            height,
        };

        let mut stream = SCStream::new(&filter, &stream_config);
        stream.add_output_handler(handler, SCStreamOutputType::Screen);

        stream
            .start_capture()
            .map_err(|e| CaptureError::Platform(format!("Failed to start capture: {e:?}")))?;

        self.stream = Some(SendableStream(stream));
        self.config = Some(CaptureConfig {
            target_fps: config.target_fps,
            width,
            height,
        });
        self.running = true;

        info!("macOS screen capture started ({}x{} @ {}fps)", width, height, config.target_fps);

        Ok(())
    }

    fn stop(&mut self) -> Result<(), CaptureError> {
        if !self.running {
            return Err(CaptureError::NotStarted);
        }

        if let Some(SendableStream(ref mut stream)) = self.stream {
            if let Err(e) = stream.stop_capture() {
                error!("Error stopping capture: {e:?}");
            }
        }

        self.stream = None;
        self.running = false;

        // Clear any remaining buffered frames
        if let Ok(mut buf) = self.frame_buffer.lock() {
            buf.clear();
        }

        info!("macOS screen capture stopped");
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn poll_frame(&mut self) -> Result<Option<CapturedFrame>, CaptureError> {
        if !self.running {
            return Err(CaptureError::NotStarted);
        }

        // ScreenCaptureKit's minimum_frame_interval already rate-limits delivery,
        // so no software rate-limiter needed here.

        let mut buf = self
            .frame_buffer
            .lock()
            .map_err(|e| CaptureError::Platform(format!("Buffer lock poisoned: {e}")))?;

        // Take the most recent frame, discard older ones
        if buf.len() > 1 {
            let latest = buf.pop_back();
            buf.clear();
            Ok(latest)
        } else {
            Ok(buf.pop_front())
        }
    }
}
