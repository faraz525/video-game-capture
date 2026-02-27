use super::{CaptureConfig, CaptureError, CapturedFrame, FramePixelFormat, ScreenCapture};
use crate::sync::clock::SyncClock;
use std::time::{Duration, Instant};

/// Mock screen capture that generates synthetic colored frames.
///
/// Produces solid-color frames that cycle through red, green, blue, and yellow
/// at the configured FPS. Used for development and testing on platforms without
/// a native capture implementation.
#[allow(dead_code)]
pub struct MockCapture {
    config: Option<CaptureConfig>,
    clock: SyncClock,
    running: bool,
    frame_count: u64,
    last_frame_time: Option<Instant>,
}

#[allow(dead_code)]
impl MockCapture {
    pub fn new(clock: SyncClock) -> Self {
        Self {
            config: None,
            clock,
            running: false,
            frame_count: 0,
            last_frame_time: None,
        }
    }

    fn generate_frame(&self, config: &CaptureConfig) -> CapturedFrame {
        let colors: [(u8, u8, u8); 4] = [
            (255, 0, 0),     // red
            (0, 255, 0),     // green
            (0, 0, 255),     // blue
            (255, 255, 0),   // yellow
        ];

        let (r, g, b) = colors[(self.frame_count as usize) % colors.len()];
        let pixel_count = (config.width * config.height) as usize;
        let mut data = Vec::with_capacity(pixel_count * 4);

        for _ in 0..pixel_count {
            data.push(r);
            data.push(g);
            data.push(b);
            data.push(255); // alpha
        }

        CapturedFrame {
            timestamp_us: self.clock.now_us(),
            width: config.width,
            height: config.height,
            data,
            pixel_format: FramePixelFormat::Rgba,
        }
    }
}

impl ScreenCapture for MockCapture {
    fn start(&mut self, config: CaptureConfig) -> Result<(), CaptureError> {
        if self.running {
            return Err(CaptureError::AlreadyRunning);
        }
        self.config = Some(config);
        self.running = true;
        self.frame_count = 0;
        self.last_frame_time = None;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), CaptureError> {
        if !self.running {
            return Err(CaptureError::NotStarted);
        }
        self.running = false;
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
        let frame_interval = Duration::from_secs_f64(1.0 / config.target_fps as f64);

        let should_produce = match self.last_frame_time {
            None => true,
            Some(last) => last.elapsed() >= frame_interval,
        };

        if !should_produce {
            return Ok(None);
        }

        let frame = self.generate_frame(config);
        self.frame_count += 1;
        self.last_frame_time = Some(Instant::now());
        Ok(Some(frame))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn start_and_stop() {
        let clock = SyncClock::new();
        let mut capture = MockCapture::new(clock);

        assert!(!capture.is_running());
        capture.start(CaptureConfig::default()).unwrap();
        assert!(capture.is_running());
        capture.stop().unwrap();
        assert!(!capture.is_running());
    }

    #[test]
    fn double_start_fails() {
        let clock = SyncClock::new();
        let mut capture = MockCapture::new(clock);

        capture.start(CaptureConfig::default()).unwrap();
        assert!(capture.start(CaptureConfig::default()).is_err());
    }

    #[test]
    fn stop_without_start_fails() {
        let clock = SyncClock::new();
        let mut capture = MockCapture::new(clock);

        assert!(capture.stop().is_err());
    }

    #[test]
    fn poll_without_start_fails() {
        let clock = SyncClock::new();
        let mut capture = MockCapture::new(clock);

        assert!(capture.poll_frame().is_err());
    }

    #[test]
    fn produces_frames_at_target_fps() {
        let clock = SyncClock::new();
        let mut capture = MockCapture::new(clock);

        let config = CaptureConfig {
            target_fps: 60,
            width: 64,
            height: 64,
        };
        capture.start(config).unwrap();

        // First poll should produce a frame immediately
        let frame = capture.poll_frame().unwrap();
        assert!(frame.is_some());
        let f = frame.unwrap();
        assert_eq!(f.width, 64);
        assert_eq!(f.height, 64);
        assert_eq!(f.data.len(), 64 * 64 * 4);

        // Immediate second poll should return None (too soon)
        let frame = capture.poll_frame().unwrap();
        assert!(frame.is_none());

        // After waiting one frame interval, should produce again
        thread::sleep(Duration::from_millis(17)); // ~60fps
        let frame = capture.poll_frame().unwrap();
        assert!(frame.is_some());
    }

    #[test]
    fn frames_have_correct_rgba_data() {
        let clock = SyncClock::new();
        let mut capture = MockCapture::new(clock);

        let config = CaptureConfig {
            target_fps: 60,
            width: 2,
            height: 2,
        };
        capture.start(config).unwrap();

        let frame = capture.poll_frame().unwrap().unwrap();
        // First frame should be red (255, 0, 0, 255)
        assert_eq!(&frame.data[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn frames_cycle_colors() {
        let clock = SyncClock::new();
        let mut capture = MockCapture::new(clock);

        let config = CaptureConfig {
            target_fps: 1000, // high fps so we don't have to sleep
            width: 1,
            height: 1,
        };
        capture.start(config).unwrap();

        // Frame 0: red
        let f0 = capture.poll_frame().unwrap().unwrap();
        assert_eq!(&f0.data[0..3], &[255, 0, 0]);

        thread::sleep(Duration::from_millis(2));

        // Frame 1: green
        let f1 = capture.poll_frame().unwrap().unwrap();
        assert_eq!(&f1.data[0..3], &[0, 255, 0]);

        thread::sleep(Duration::from_millis(2));

        // Frame 2: blue
        let f2 = capture.poll_frame().unwrap().unwrap();
        assert_eq!(&f2.data[0..3], &[0, 0, 255]);

        thread::sleep(Duration::from_millis(2));

        // Frame 3: yellow
        let f3 = capture.poll_frame().unwrap().unwrap();
        assert_eq!(&f3.data[0..3], &[255, 255, 0]);
    }

    #[test]
    fn frame_timestamps_increase() {
        let clock = SyncClock::new();
        let mut capture = MockCapture::new(clock);

        let config = CaptureConfig {
            target_fps: 1000,
            width: 1,
            height: 1,
        };
        capture.start(config).unwrap();

        let f0 = capture.poll_frame().unwrap().unwrap();
        thread::sleep(Duration::from_millis(2));
        let f1 = capture.poll_frame().unwrap().unwrap();

        assert!(
            f1.timestamp_us > f0.timestamp_us,
            "timestamps should increase: {} > {}",
            f1.timestamp_us,
            f0.timestamp_us
        );
    }
}
