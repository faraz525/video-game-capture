pub mod mock;

#[cfg(target_os = "windows")]
pub mod windows;

use crate::sync::clock::TimestampUs;

/// A captured audio buffer with its timestamp and PCM data.
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    /// Microsecond timestamp from SyncClock.
    pub timestamp_us: TimestampUs,
    /// Number of audio channels (1 = mono, 2 = stereo).
    pub channels: u16,
    /// Sample rate in Hz (e.g., 44100, 48000).
    pub sample_rate: u32,
    /// Interleaved f32 PCM samples in range [-1.0, 1.0].
    pub samples: Vec<f32>,
}

/// Configuration for audio capture.
#[derive(Debug, Clone)]
pub struct AudioConfig {
    /// Number of audio channels.
    pub channels: u16,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Buffer size in samples per channel per callback.
    pub buffer_size: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            channels: 2,
            sample_rate: 48000,
            buffer_size: 1024,
        }
    }
}

/// Error type for audio capture operations.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("audio capture not started")]
    NotStarted,
    #[error("audio capture already running")]
    AlreadyRunning,
    #[error("platform error: {0}")]
    #[allow(dead_code)]
    Platform(String),
}

/// Platform-abstracted audio capture interface.
///
/// Captures system/game audio via loopback. The mock implementation
/// generates a sine wave tone for development on non-Windows platforms.
pub trait AudioCapture: Send {
    /// Start capturing audio with the given configuration.
    fn start(&mut self, config: AudioConfig) -> Result<(), AudioError>;

    /// Stop capturing audio.
    fn stop(&mut self) -> Result<(), AudioError>;

    /// Returns true if audio capture is currently running.
    #[allow(dead_code)]
    fn is_running(&self) -> bool;

    /// Poll for the next available audio buffer. Returns None if no new data is ready.
    fn poll_buffer(&mut self) -> Result<Option<AudioBuffer>, AudioError>;
}
