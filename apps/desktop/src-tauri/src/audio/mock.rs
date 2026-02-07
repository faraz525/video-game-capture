use super::{AudioBuffer, AudioCapture, AudioConfig, AudioError};
use crate::sync::clock::SyncClock;
use std::f32::consts::PI;
use std::time::{Duration, Instant};

const SINE_FREQUENCY_HZ: f32 = 440.0;

/// Mock audio capture that generates a 440Hz sine wave tone.
///
/// Produces stereo PCM audio buffers at the configured sample rate.
/// Used for development and testing on non-Windows platforms.
pub struct MockAudioCapture {
    clock: SyncClock,
    config: Option<AudioConfig>,
    running: bool,
    sample_position: u64,
    last_buffer_time: Option<Instant>,
}

impl MockAudioCapture {
    pub fn new(clock: SyncClock) -> Self {
        Self {
            clock,
            config: None,
            running: false,
            sample_position: 0,
            last_buffer_time: None,
        }
    }

    fn generate_buffer(&mut self, config: &AudioConfig) -> AudioBuffer {
        let timestamp_us = self.clock.now_us();
        let num_samples = config.buffer_size as usize;
        let channels = config.channels as usize;
        let mut samples = Vec::with_capacity(num_samples * channels);

        for i in 0..num_samples {
            let t = (self.sample_position + i as u64) as f32 / config.sample_rate as f32;
            let value = (2.0 * PI * SINE_FREQUENCY_HZ * t).sin() * 0.3;

            for _ in 0..channels {
                samples.push(value);
            }
        }

        self.sample_position += num_samples as u64;

        AudioBuffer {
            timestamp_us,
            channels: config.channels,
            sample_rate: config.sample_rate,
            samples,
        }
    }
}

impl AudioCapture for MockAudioCapture {
    fn start(&mut self, config: AudioConfig) -> Result<(), AudioError> {
        if self.running {
            return Err(AudioError::AlreadyRunning);
        }
        self.config = Some(config);
        self.running = true;
        self.sample_position = 0;
        self.last_buffer_time = None;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), AudioError> {
        if !self.running {
            return Err(AudioError::NotStarted);
        }
        self.running = false;
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn poll_buffer(&mut self) -> Result<Option<AudioBuffer>, AudioError> {
        if !self.running {
            return Err(AudioError::NotStarted);
        }

        let config = self.config.as_ref().ok_or(AudioError::NotStarted)?;
        let buffer_duration_ms =
            (config.buffer_size as f64 / config.sample_rate as f64 * 1000.0) as u64;
        let interval = Duration::from_millis(buffer_duration_ms);

        let should_produce = match self.last_buffer_time {
            None => true,
            Some(last) => last.elapsed() >= interval,
        };

        if !should_produce {
            return Ok(None);
        }

        let config_clone = config.clone();
        let buffer = self.generate_buffer(&config_clone);
        self.last_buffer_time = Some(Instant::now());
        Ok(Some(buffer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn start_and_stop() {
        let clock = SyncClock::new();
        let mut capture = MockAudioCapture::new(clock);

        assert!(!capture.is_running());
        capture.start(AudioConfig::default()).unwrap();
        assert!(capture.is_running());
        capture.stop().unwrap();
        assert!(!capture.is_running());
    }

    #[test]
    fn double_start_fails() {
        let clock = SyncClock::new();
        let mut capture = MockAudioCapture::new(clock);

        capture.start(AudioConfig::default()).unwrap();
        assert!(capture.start(AudioConfig::default()).is_err());
    }

    #[test]
    fn stop_without_start_fails() {
        let clock = SyncClock::new();
        let mut capture = MockAudioCapture::new(clock);

        assert!(capture.stop().is_err());
    }

    #[test]
    fn poll_without_start_fails() {
        let clock = SyncClock::new();
        let mut capture = MockAudioCapture::new(clock);

        assert!(capture.poll_buffer().is_err());
    }

    #[test]
    fn produces_buffer_on_first_poll() {
        let clock = SyncClock::new();
        let mut capture = MockAudioCapture::new(clock);

        let config = AudioConfig {
            channels: 2,
            sample_rate: 48000,
            buffer_size: 1024,
        };
        capture.start(config).unwrap();

        let buffer = capture.poll_buffer().unwrap();
        assert!(buffer.is_some());

        let buf = buffer.unwrap();
        assert_eq!(buf.channels, 2);
        assert_eq!(buf.sample_rate, 48000);
        assert_eq!(buf.samples.len(), 1024 * 2); // buffer_size * channels
    }

    #[test]
    fn samples_are_in_valid_range() {
        let clock = SyncClock::new();
        let mut capture = MockAudioCapture::new(clock);

        capture.start(AudioConfig::default()).unwrap();
        let buf = capture.poll_buffer().unwrap().unwrap();

        for sample in &buf.samples {
            assert!(
                *sample >= -1.0 && *sample <= 1.0,
                "sample {sample} out of range"
            );
        }
    }

    #[test]
    fn respects_buffer_interval() {
        let clock = SyncClock::new();
        let mut capture = MockAudioCapture::new(clock);

        let config = AudioConfig {
            channels: 1,
            sample_rate: 48000,
            buffer_size: 4800, // 100ms buffers
        };
        capture.start(config).unwrap();

        // First poll produces
        assert!(capture.poll_buffer().unwrap().is_some());

        // Immediate second poll should return None
        assert!(capture.poll_buffer().unwrap().is_none());

        // After waiting, should produce
        thread::sleep(Duration::from_millis(110));
        assert!(capture.poll_buffer().unwrap().is_some());
    }

    #[test]
    fn buffers_have_timestamps() {
        let clock = SyncClock::new();
        let mut capture = MockAudioCapture::new(clock);

        capture.start(AudioConfig::default()).unwrap();
        let buf = capture.poll_buffer().unwrap().unwrap();
        assert!(buf.timestamp_us > 0);
    }
}
