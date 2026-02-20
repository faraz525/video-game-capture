use super::{
    InputError, InputEvent, InputEventKind, InputRecorder, KeyEvent, MouseButton,
    MouseButtonEvent, MouseMoveEvent, MouseScrollEvent,
};
use crate::sync::clock::SyncClock;
use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
use core_graphics::event::{
    CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType, EventField,
};
use log::{error, info, warn};
use std::sync::{Arc, Mutex};
use std::thread;

/// macOS input recorder using CGEventTap.
///
/// Creates a passive (listen-only) event tap on the user session to capture
/// keyboard and mouse events. Requires Accessibility permission
/// (System Settings > Privacy & Security > Accessibility).
pub struct MacOSInputRecorder {
    clock: SyncClock,
    running: bool,
    events: Arc<Mutex<Vec<InputEvent>>>,
    /// Stored run loop handle for the tap thread so `stop()` can break the loop.
    /// `CFRunLoop` is `Send + Sync` per the `core-foundation` crate.
    run_loop: Arc<Mutex<Option<CFRunLoop>>>,
}

impl MacOSInputRecorder {
    pub fn new(clock: SyncClock) -> Self {
        Self {
            clock,
            running: false,
            events: Arc::new(Mutex::new(Vec::new())),
            run_loop: Arc::new(Mutex::new(None)),
        }
    }
}

impl InputRecorder for MacOSInputRecorder {
    fn start(&mut self) -> Result<(), InputError> {
        if self.running {
            return Err(InputError::AlreadyRunning);
        }

        self.running = true;
        let events = Arc::clone(&self.events);
        let clock = self.clock.clone();
        let run_loop_store = Arc::clone(&self.run_loop);

        thread::spawn(move || {
            if let Err(e) = run_event_tap_loop(events, clock, run_loop_store) {
                error!("CGEventTap loop error: {e}");
            }
        });

        Ok(())
    }

    fn stop(&mut self) -> Result<(), InputError> {
        if !self.running {
            return Err(InputError::NotStarted);
        }
        self.running = false;

        if let Ok(guard) = self.run_loop.lock() {
            if let Some(ref rl) = *guard {
                rl.stop();
            }
        }

        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn poll_events(&mut self) -> Result<Vec<InputEvent>, InputError> {
        if !self.running {
            return Err(InputError::NotStarted);
        }

        let mut events = self
            .events
            .lock()
            .map_err(|e| InputError::Platform(format!("Failed to lock events: {e}")))?;

        let drained: Vec<InputEvent> = events.drain(..).collect();
        Ok(drained)
    }
}

/// Run the CFRunLoop with a CGEventTap on a dedicated thread.
fn run_event_tap_loop(
    events: Arc<Mutex<Vec<InputEvent>>>,
    clock: SyncClock,
    run_loop_store: Arc<Mutex<Option<CFRunLoop>>>,
) -> Result<(), InputError> {
    let events_of_interest = vec![
        CGEventType::KeyDown,
        CGEventType::KeyUp,
        CGEventType::FlagsChanged,
        CGEventType::LeftMouseDown,
        CGEventType::LeftMouseUp,
        CGEventType::RightMouseDown,
        CGEventType::RightMouseUp,
        CGEventType::OtherMouseDown,
        CGEventType::OtherMouseUp,
        CGEventType::MouseMoved,
        CGEventType::LeftMouseDragged,
        CGEventType::RightMouseDragged,
        CGEventType::ScrollWheel,
    ];

    // Track modifier flags for FlagsChanged handling
    let last_flags: Arc<Mutex<CGEventFlags>> = Arc::new(Mutex::new(CGEventFlags::empty()));

    let cb_events = Arc::clone(&events);
    let cb_clock = clock;
    let cb_flags = Arc::clone(&last_flags);

    let tap = CGEventTap::new(
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        events_of_interest,
        move |_proxy: CGEventTapProxy, event_type: CGEventType, event| {
            process_event(&cb_events, &cb_clock, &cb_flags, _proxy, event_type, event);
            // Return None to pass event through unmodified (listen-only)
            None
        },
    );

    let tap = match tap {
        Ok(tap) => tap,
        Err(()) => {
            warn!(
                "CGEventTap creation failed — Accessibility permission likely denied. \
                 Input recording disabled. Grant access in System Settings > \
                 Privacy & Security > Accessibility."
            );
            return Err(InputError::Platform(
                "CGEventTap creation failed (Accessibility permission denied)".to_string(),
            ));
        }
    };

    let loop_source = tap
        .mach_port
        .create_runloop_source(0)
        .map_err(|()| InputError::Platform("Failed to create run loop source".to_string()))?;

    let current_run_loop = CFRunLoop::get_current();
    current_run_loop.add_source(&loop_source, unsafe { kCFRunLoopCommonModes });
    tap.enable();

    // Store the run loop so stop() can call .stop() from another thread
    if let Ok(mut store) = run_loop_store.lock() {
        *store = Some(CFRunLoop::get_current());
    }

    info!("macOS input recorder started (CGEventTap)");

    // Block until CFRunLoop::stop() is called from stop()
    CFRunLoop::run_current();

    // Cleanup
    if let Ok(mut store) = run_loop_store.lock() {
        *store = None;
    }

    info!("macOS input recorder stopped");
    Ok(())
}

/// Process a single CGEvent and push it to the shared buffer.
fn process_event(
    events: &Arc<Mutex<Vec<InputEvent>>>,
    clock: &SyncClock,
    last_flags: &Arc<Mutex<CGEventFlags>>,
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: &core_graphics::event::CGEvent,
) {
    let timestamp_us = clock.now_us();

    let kind = match event_type {
        CGEventType::KeyDown => {
            let keycode =
                event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
            Some(InputEventKind::Key(KeyEvent {
                key: keycode_to_web_key(keycode),
                pressed: true,
            }))
        }
        CGEventType::KeyUp => {
            let keycode =
                event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
            Some(InputEventKind::Key(KeyEvent {
                key: keycode_to_web_key(keycode),
                pressed: false,
            }))
        }
        CGEventType::FlagsChanged => {
            let keycode =
                event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
            let current_flags = event.get_flags();
            handle_flags_changed(events, last_flags, keycode, current_flags, timestamp_us);
            None
        }
        CGEventType::LeftMouseDown => {
            let loc = event.location();
            Some(InputEventKind::MouseButton(MouseButtonEvent {
                button: MouseButton::Left,
                pressed: true,
                x: loc.x,
                y: loc.y,
            }))
        }
        CGEventType::LeftMouseUp => {
            let loc = event.location();
            Some(InputEventKind::MouseButton(MouseButtonEvent {
                button: MouseButton::Left,
                pressed: false,
                x: loc.x,
                y: loc.y,
            }))
        }
        CGEventType::RightMouseDown => {
            let loc = event.location();
            Some(InputEventKind::MouseButton(MouseButtonEvent {
                button: MouseButton::Right,
                pressed: true,
                x: loc.x,
                y: loc.y,
            }))
        }
        CGEventType::RightMouseUp => {
            let loc = event.location();
            Some(InputEventKind::MouseButton(MouseButtonEvent {
                button: MouseButton::Right,
                pressed: false,
                x: loc.x,
                y: loc.y,
            }))
        }
        CGEventType::OtherMouseDown => {
            let loc = event.location();
            Some(InputEventKind::MouseButton(MouseButtonEvent {
                button: MouseButton::Middle,
                pressed: true,
                x: loc.x,
                y: loc.y,
            }))
        }
        CGEventType::OtherMouseUp => {
            let loc = event.location();
            Some(InputEventKind::MouseButton(MouseButtonEvent {
                button: MouseButton::Middle,
                pressed: false,
                x: loc.x,
                y: loc.y,
            }))
        }
        CGEventType::MouseMoved
        | CGEventType::LeftMouseDragged
        | CGEventType::RightMouseDragged => {
            let loc = event.location();
            Some(InputEventKind::MouseMove(MouseMoveEvent {
                x: loc.x,
                y: loc.y,
            }))
        }
        CGEventType::ScrollWheel => {
            let delta_y = event.get_integer_value_field(
                EventField::SCROLL_WHEEL_EVENT_FIXED_POINT_DELTA_AXIS_1,
            ) as f64
                / 65536.0;
            let delta_x = event.get_integer_value_field(
                EventField::SCROLL_WHEEL_EVENT_FIXED_POINT_DELTA_AXIS_2,
            ) as f64
                / 65536.0;
            Some(InputEventKind::MouseScroll(MouseScrollEvent {
                delta_x,
                delta_y,
            }))
        }
        _ => None,
    };

    if let Some(kind) = kind {
        let input_event = InputEvent { timestamp_us, kind };
        if let Ok(mut buf) = events.lock() {
            buf.push(input_event);
        }
    }
}

/// Handle FlagsChanged events for modifier keys.
///
/// macOS fires `FlagsChanged` instead of `KeyDown`/`KeyUp` for modifier keys.
/// We compare current flags to previous flags to detect press/release transitions.
fn handle_flags_changed(
    events: &Arc<Mutex<Vec<InputEvent>>>,
    last_flags: &Arc<Mutex<CGEventFlags>>,
    keycode: u16,
    current_flags: CGEventFlags,
    timestamp_us: u64,
) {
    let previous_flags = {
        let guard = last_flags.lock().unwrap_or_else(|e| e.into_inner());
        *guard
    };

    // Determine which modifier flag corresponds to this keycode
    let flag = match keycode {
        0x38 | 0x3C => CGEventFlags::CGEventFlagShift,
        0x3B | 0x3E => CGEventFlags::CGEventFlagControl,
        0x3A | 0x3D => CGEventFlags::CGEventFlagAlternate,
        0x37 | 0x36 => CGEventFlags::CGEventFlagCommand,
        0x39 => CGEventFlags::CGEventFlagAlphaShift,
        _ => return,
    };

    let was_set = previous_flags.contains(flag);
    let is_set = current_flags.contains(flag);
    let pressed = !was_set && is_set;
    let released = was_set && !is_set;

    if pressed || released {
        let key = keycode_to_web_key(keycode);
        let input_event = InputEvent {
            timestamp_us,
            kind: InputEventKind::Key(KeyEvent { key, pressed }),
        };
        if let Ok(mut buf) = events.lock() {
            buf.push(input_event);
        }
    }

    if let Ok(mut guard) = last_flags.lock() {
        *guard = current_flags;
    }
}

/// Map a macOS IOKit virtual keycode to a web-style key name.
///
/// These keycodes are defined in `<Carbon/HIToolbox/Events.h>` and are stable
/// across macOS versions. Maps ~70 common gaming keys.
fn keycode_to_web_key(keycode: u16) -> String {
    match keycode {
        // Letters (QWERTY layout — keycodes are positional, not character-based)
        0x00 => "KeyA".to_string(),
        0x01 => "KeyS".to_string(),
        0x02 => "KeyD".to_string(),
        0x03 => "KeyF".to_string(),
        0x04 => "KeyH".to_string(),
        0x05 => "KeyG".to_string(),
        0x06 => "KeyZ".to_string(),
        0x07 => "KeyX".to_string(),
        0x08 => "KeyC".to_string(),
        0x09 => "KeyV".to_string(),
        0x0B => "KeyB".to_string(),
        0x0C => "KeyQ".to_string(),
        0x0D => "KeyW".to_string(),
        0x0E => "KeyE".to_string(),
        0x0F => "KeyR".to_string(),
        0x10 => "KeyY".to_string(),
        0x11 => "KeyT".to_string(),
        0x12 => "Digit1".to_string(),
        0x13 => "Digit2".to_string(),
        0x14 => "Digit3".to_string(),
        0x15 => "Digit4".to_string(),
        0x16 => "Digit6".to_string(),
        0x17 => "Digit5".to_string(),
        0x18 => "Equal".to_string(),
        0x19 => "Digit9".to_string(),
        0x1A => "Digit7".to_string(),
        0x1B => "Minus".to_string(),
        0x1C => "Digit8".to_string(),
        0x1D => "Digit0".to_string(),
        0x1E => "BracketRight".to_string(),
        0x1F => "KeyO".to_string(),
        0x20 => "KeyU".to_string(),
        0x21 => "BracketLeft".to_string(),
        0x22 => "KeyI".to_string(),
        0x23 => "KeyP".to_string(),
        0x25 => "KeyL".to_string(),
        0x26 => "KeyJ".to_string(),
        0x27 => "Quote".to_string(),
        0x28 => "KeyK".to_string(),
        0x29 => "Semicolon".to_string(),
        0x2A => "Backslash".to_string(),
        0x2B => "Comma".to_string(),
        0x2C => "Slash".to_string(),
        0x2D => "KeyN".to_string(),
        0x2E => "KeyM".to_string(),
        0x2F => "Period".to_string(),
        0x32 => "Backquote".to_string(),

        // Whitespace / editing
        0x24 => "Enter".to_string(),
        0x30 => "Tab".to_string(),
        0x31 => "Space".to_string(),
        0x33 => "Backspace".to_string(),
        0x35 => "Escape".to_string(),

        // Modifier keys
        0x37 => "MetaLeft".to_string(),
        0x36 => "MetaRight".to_string(),
        0x38 => "ShiftLeft".to_string(),
        0x39 => "CapsLock".to_string(),
        0x3A => "AltLeft".to_string(),
        0x3B => "ControlLeft".to_string(),
        0x3C => "ShiftRight".to_string(),
        0x3D => "AltRight".to_string(),
        0x3E => "ControlRight".to_string(),
        0x3F => "Fn".to_string(),

        // Function keys
        0x7A => "F1".to_string(),
        0x78 => "F2".to_string(),
        0x63 => "F3".to_string(),
        0x76 => "F4".to_string(),
        0x60 => "F5".to_string(),
        0x61 => "F6".to_string(),
        0x62 => "F7".to_string(),
        0x64 => "F8".to_string(),
        0x65 => "F9".to_string(),
        0x6D => "F10".to_string(),
        0x67 => "F11".to_string(),
        0x6F => "F12".to_string(),

        // Arrow keys
        0x7B => "ArrowLeft".to_string(),
        0x7C => "ArrowRight".to_string(),
        0x7D => "ArrowDown".to_string(),
        0x7E => "ArrowUp".to_string(),

        // Navigation
        0x73 => "Home".to_string(),
        0x77 => "End".to_string(),
        0x74 => "PageUp".to_string(),
        0x79 => "PageDown".to_string(),
        0x75 => "Delete".to_string(),

        // Numpad
        0x52 => "Numpad0".to_string(),
        0x53 => "Numpad1".to_string(),
        0x54 => "Numpad2".to_string(),
        0x55 => "Numpad3".to_string(),
        0x56 => "Numpad4".to_string(),
        0x57 => "Numpad5".to_string(),
        0x58 => "Numpad6".to_string(),
        0x59 => "Numpad7".to_string(),
        0x5B => "Numpad8".to_string(),
        0x5C => "Numpad9".to_string(),
        0x41 => "NumpadDecimal".to_string(),
        0x43 => "NumpadMultiply".to_string(),
        0x45 => "NumpadAdd".to_string(),
        0x4B => "NumpadDivide".to_string(),
        0x4C => "NumpadEnter".to_string(),
        0x4E => "NumpadSubtract".to_string(),
        0x51 => "NumpadEqual".to_string(),
        0x47 => "NumLock".to_string(),

        _ => format!("Unknown(0x{keycode:02X})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keycode_to_web_key_letters() {
        assert_eq!(keycode_to_web_key(0x00), "KeyA");
        assert_eq!(keycode_to_web_key(0x01), "KeyS");
        assert_eq!(keycode_to_web_key(0x02), "KeyD");
        assert_eq!(keycode_to_web_key(0x0D), "KeyW");
        assert_eq!(keycode_to_web_key(0x06), "KeyZ");
    }

    #[test]
    fn keycode_to_web_key_digits() {
        assert_eq!(keycode_to_web_key(0x12), "Digit1");
        assert_eq!(keycode_to_web_key(0x1D), "Digit0");
        assert_eq!(keycode_to_web_key(0x17), "Digit5");
    }

    #[test]
    fn keycode_to_web_key_modifiers() {
        assert_eq!(keycode_to_web_key(0x38), "ShiftLeft");
        assert_eq!(keycode_to_web_key(0x3C), "ShiftRight");
        assert_eq!(keycode_to_web_key(0x3B), "ControlLeft");
        assert_eq!(keycode_to_web_key(0x3E), "ControlRight");
        assert_eq!(keycode_to_web_key(0x3A), "AltLeft");
        assert_eq!(keycode_to_web_key(0x3D), "AltRight");
        assert_eq!(keycode_to_web_key(0x37), "MetaLeft");
        assert_eq!(keycode_to_web_key(0x36), "MetaRight");
    }

    #[test]
    fn keycode_to_web_key_arrows() {
        assert_eq!(keycode_to_web_key(0x7B), "ArrowLeft");
        assert_eq!(keycode_to_web_key(0x7C), "ArrowRight");
        assert_eq!(keycode_to_web_key(0x7D), "ArrowDown");
        assert_eq!(keycode_to_web_key(0x7E), "ArrowUp");
    }

    #[test]
    fn keycode_to_web_key_function_keys() {
        assert_eq!(keycode_to_web_key(0x7A), "F1");
        assert_eq!(keycode_to_web_key(0x6F), "F12");
        assert_eq!(keycode_to_web_key(0x60), "F5");
    }

    #[test]
    fn keycode_to_web_key_common_gaming() {
        assert_eq!(keycode_to_web_key(0x31), "Space");
        assert_eq!(keycode_to_web_key(0x24), "Enter");
        assert_eq!(keycode_to_web_key(0x35), "Escape");
        assert_eq!(keycode_to_web_key(0x30), "Tab");
    }

    #[test]
    fn keycode_to_web_key_unknown() {
        let result = keycode_to_web_key(0xFF);
        assert!(result.starts_with("Unknown("));
    }

    #[test]
    fn double_start_fails() {
        let clock = SyncClock::new();
        let mut recorder = MacOSInputRecorder::new(clock);

        // First start spawns a thread that may fail (no Accessibility permission in CI)
        // but the state should still track as "running"
        let _ = recorder.start();
        assert!(recorder.start().is_err());
    }

    #[test]
    fn stop_without_start_fails() {
        let clock = SyncClock::new();
        let mut recorder = MacOSInputRecorder::new(clock);

        assert!(recorder.stop().is_err());
    }

    #[test]
    fn poll_without_start_fails() {
        let clock = SyncClock::new();
        let mut recorder = MacOSInputRecorder::new(clock);

        assert!(recorder.poll_events().is_err());
    }
}
