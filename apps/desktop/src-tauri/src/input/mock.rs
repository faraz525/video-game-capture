use super::{
    InputError, InputEvent, InputEventKind, InputRecorder, KeyEvent, MouseButton,
    MouseButtonEvent, MouseMoveEvent,
};
use crate::sync::clock::SyncClock;
use std::time::{Duration, Instant};

const MOCK_KEYS: &[&str] = &["KeyW", "KeyA", "KeyS", "KeyD", "Space", "ShiftLeft", "KeyE"];
const EVENT_INTERVAL_MS: u64 = 50;

/// Mock input recorder that generates synthetic keyboard and mouse events.
///
/// Cycles through WASD + common game keys, mouse movements, and clicks
/// at a fixed interval. Used for development and testing on non-Windows platforms.
pub struct MockInputRecorder {
    clock: SyncClock,
    running: bool,
    event_counter: u64,
    last_event_time: Option<Instant>,
}

impl MockInputRecorder {
    pub fn new(clock: SyncClock) -> Self {
        Self {
            clock,
            running: false,
            event_counter: 0,
            last_event_time: None,
        }
    }

    fn generate_event(&mut self) -> InputEvent {
        let timestamp_us = self.clock.now_us();
        let counter = self.event_counter;
        self.event_counter += 1;

        let kind = match counter % 6 {
            0 | 1 => {
                let key_idx = (counter / 2) as usize % MOCK_KEYS.len();
                InputEventKind::Key(KeyEvent {
                    key: MOCK_KEYS[key_idx].to_string(),
                    pressed: counter.is_multiple_of(2),
                })
            }
            2 => InputEventKind::MouseMove(MouseMoveEvent {
                x: ((counter * 7) % 1920) as f64,
                y: ((counter * 13) % 1080) as f64,
            }),
            3 => InputEventKind::MouseButton(MouseButtonEvent {
                button: MouseButton::Left,
                pressed: true,
                x: ((counter * 7) % 1920) as f64,
                y: ((counter * 13) % 1080) as f64,
            }),
            4 => InputEventKind::MouseButton(MouseButtonEvent {
                button: MouseButton::Left,
                pressed: false,
                x: ((counter * 7) % 1920) as f64,
                y: ((counter * 13) % 1080) as f64,
            }),
            _ => InputEventKind::Key(KeyEvent {
                key: "Space".to_string(),
                pressed: true,
            }),
        };

        InputEvent { timestamp_us, kind }
    }
}

impl InputRecorder for MockInputRecorder {
    fn start(&mut self) -> Result<(), InputError> {
        if self.running {
            return Err(InputError::AlreadyRunning);
        }
        self.running = true;
        self.event_counter = 0;
        self.last_event_time = None;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), InputError> {
        if !self.running {
            return Err(InputError::NotStarted);
        }
        self.running = false;
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn poll_events(&mut self) -> Result<Vec<InputEvent>, InputError> {
        if !self.running {
            return Err(InputError::NotStarted);
        }

        let interval = Duration::from_millis(EVENT_INTERVAL_MS);
        let should_produce = match self.last_event_time {
            None => true,
            Some(last) => last.elapsed() >= interval,
        };

        if !should_produce {
            return Ok(vec![]);
        }

        self.last_event_time = Some(Instant::now());
        let event = self.generate_event();
        Ok(vec![event])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn start_and_stop() {
        let clock = SyncClock::new();
        let mut recorder = MockInputRecorder::new(clock);

        assert!(!recorder.is_running());
        recorder.start().unwrap();
        assert!(recorder.is_running());
        recorder.stop().unwrap();
        assert!(!recorder.is_running());
    }

    #[test]
    fn double_start_fails() {
        let clock = SyncClock::new();
        let mut recorder = MockInputRecorder::new(clock);

        recorder.start().unwrap();
        assert!(recorder.start().is_err());
    }

    #[test]
    fn stop_without_start_fails() {
        let clock = SyncClock::new();
        let mut recorder = MockInputRecorder::new(clock);

        assert!(recorder.stop().is_err());
    }

    #[test]
    fn poll_without_start_fails() {
        let clock = SyncClock::new();
        let mut recorder = MockInputRecorder::new(clock);

        assert!(recorder.poll_events().is_err());
    }

    #[test]
    fn produces_events_on_first_poll() {
        let clock = SyncClock::new();
        let mut recorder = MockInputRecorder::new(clock);

        recorder.start().unwrap();
        let events = recorder.poll_events().unwrap();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn respects_event_interval() {
        let clock = SyncClock::new();
        let mut recorder = MockInputRecorder::new(clock);

        recorder.start().unwrap();
        let _ = recorder.poll_events().unwrap();

        // Immediate second poll should return empty
        let events = recorder.poll_events().unwrap();
        assert!(events.is_empty());

        // After waiting, should produce again
        thread::sleep(Duration::from_millis(EVENT_INTERVAL_MS + 10));
        let events = recorder.poll_events().unwrap();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn generates_different_event_types() {
        let clock = SyncClock::new();
        let mut recorder = MockInputRecorder::new(clock);

        recorder.start().unwrap();

        let mut has_key = false;
        let mut has_mouse_move = false;
        let mut has_mouse_button = false;

        for _ in 0..6 {
            let events = recorder.poll_events().unwrap();
            for event in events {
                match event.kind {
                    InputEventKind::Key(_) => has_key = true,
                    InputEventKind::MouseMove(_) => has_mouse_move = true,
                    InputEventKind::MouseButton(_) => has_mouse_button = true,
                    _ => {}
                }
            }
            thread::sleep(Duration::from_millis(EVENT_INTERVAL_MS + 5));
        }

        assert!(has_key, "should generate key events");
        assert!(has_mouse_move, "should generate mouse move events");
        assert!(has_mouse_button, "should generate mouse button events");
    }

    #[test]
    fn events_have_timestamps() {
        let clock = SyncClock::new();
        let mut recorder = MockInputRecorder::new(clock);

        thread::sleep(Duration::from_millis(1));
        recorder.start().unwrap();

        let events = recorder.poll_events().unwrap();
        assert!(!events.is_empty());
        assert!(events[0].timestamp_us > 0);
    }

    #[test]
    fn events_serialize_to_json() {
        let event = InputEvent {
            timestamp_us: 1234567,
            kind: InputEventKind::Key(KeyEvent {
                key: "KeyW".to_string(),
                pressed: true,
            }),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"timestamp_us\":1234567"));
        assert!(json.contains("\"type\":\"key\""));
        assert!(json.contains("\"key\":\"KeyW\""));
        assert!(json.contains("\"pressed\":true"));

        // Roundtrip
        let deserialized: InputEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, event);
    }
}
