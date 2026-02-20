pub mod mock;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "macos")]
pub mod macos;

use crate::sync::clock::TimestampUs;

/// A single captured video frame with its timestamp and pixel data.
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    /// Microsecond timestamp from SyncClock.
    pub timestamp_us: TimestampUs,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Raw RGBA pixel data (width * height * 4 bytes).
    pub data: Vec<u8>,
}

/// Error type for screen capture operations.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("capture not started")]
    NotStarted,
    #[error("capture already running")]
    AlreadyRunning,
    #[error("no frames available")]
    #[allow(dead_code)]
    NoFrames,
    #[error("platform error: {0}")]
    #[allow(dead_code)]
    Platform(String),
}

/// Configuration for screen capture.
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// Target frames per second.
    pub target_fps: u32,
    /// Capture width in pixels (0 = native resolution).
    pub width: u32,
    /// Capture height in pixels (0 = native resolution).
    pub height: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            target_fps: 60,
            width: 1920,
            height: 1080,
        }
    }
}

/// Platform-abstracted screen capture interface.
///
/// Implementations capture screen frames at a target FPS and make them
/// available via `poll_frame()`. The mock implementation generates
/// synthetic colored frames for development on non-Windows platforms.
pub trait ScreenCapture: Send {
    /// Start capturing frames with the given configuration.
    fn start(&mut self, config: CaptureConfig) -> Result<(), CaptureError>;

    /// Stop capturing frames.
    fn stop(&mut self) -> Result<(), CaptureError>;

    /// Returns true if the capture is currently running.
    #[allow(dead_code)]
    fn is_running(&self) -> bool;

    /// Poll for the next available frame. Returns None if no new frame is ready.
    fn poll_frame(&mut self) -> Result<Option<CapturedFrame>, CaptureError>;
}
