use super::{CaptureConfig, CaptureError, CapturedFrame, FramePixelFormat, ScreenCapture};
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
        // ScreenCaptureKit delivers native BGRA — we keep it as-is to avoid
        // per-frame swizzling on the capture hot path. Downstream consumers
        // (FFmpeg, thumbnail) handle the pixel format via the tag.
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

        let frame = CapturedFrame {
            timestamp_us: self.clock.now_us(),
            width,
            height,
            data,
            pixel_format: FramePixelFormat::Bgra,
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

        // NOTE: SCStreamConfiguration's minimum_frame_interval is NOT set here.
        // The screencapturekit v1.1.0 crate has an FFI ABI mismatch: the Rust
        // FFI declares (i64, i32, u32, i64) parameters but the Swift bridge
        // expects a single Double (seconds). On arm64, the Double goes to a
        // float register while Rust sends integers to integer registers, so the
        // call is silently ignored. SCK therefore delivers at the display refresh
        // rate (~60fps). The engine's rate-limited push loop in engine.rs handles
        // this by only pushing frames at target_fps to the encoder.
        let stream_config = SCStreamConfiguration::builder()
            .width(width)
            .height(height)
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

        // Return frames in FIFO order so the engine can drain all buffered
        // frames per iteration. The MAX_BUFFERED_FRAMES cap in the SCK callback
        // provides backpressure; discarding here would lose real motion data.

        let mut buf = self
            .frame_buffer
            .lock()
            .map_err(|e| CaptureError::Platform(format!("Buffer lock poisoned: {e}")))?;

        Ok(buf.pop_front())
    }
}
