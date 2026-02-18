use super::{
    InputError, InputEvent, InputEventKind, InputRecorder, KeyEvent, MouseButton,
    MouseButtonEvent, MouseMoveEvent, MouseScrollEvent,
};
use crate::sync::clock::SyncClock;
use std::mem::MaybeUninit;
use std::sync::{Arc, Mutex};
use std::thread;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE,
    RAWINPUTHEADER, RIDEV_INPUTSINK, RID_INPUT, RIM_TYPEKEYBOARD, RIM_TYPEMOUSE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostMessageW,
    RegisterClassW, HMENU, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE,
    WNDCLASSW, WM_INPUT, WM_QUIT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::core::PCWSTR;

/// Wrapper around HWND raw pointer to allow Send across threads.
/// HWND is a raw pointer internally, but we only access it under a Mutex,
/// which makes cross-thread sharing safe for our use case (PostMessageW).
#[derive(Clone, Copy)]
struct SendableHwnd(usize);

impl SendableHwnd {
    fn from_hwnd(hwnd: HWND) -> Self {
        Self(hwnd.0 as usize)
    }

    fn to_hwnd(self) -> HWND {
        HWND(self.0 as *mut _)
    }
}

// Safety: We only use the HWND for PostMessageW which is thread-safe.
unsafe impl Send for SendableHwnd {}

/// Windows input recorder using the Raw Input API.
///
/// Creates a hidden message-only window and registers for keyboard + mouse
/// raw input with `RIDEV_INPUTSINK` so events are captured even when the
/// app is not in the foreground (required for game overlay use).
pub struct WindowsInputRecorder {
    clock: SyncClock,
    running: bool,
    events: Arc<Mutex<Vec<InputEvent>>>,
    /// Handle to the hidden message-only window, used to post WM_QUIT on stop.
    hwnd: Arc<Mutex<Option<SendableHwnd>>>,
}

impl WindowsInputRecorder {
    pub fn new(clock: SyncClock) -> Self {
        Self {
            clock,
            running: false,
            events: Arc::new(Mutex::new(Vec::new())),
            hwnd: Arc::new(Mutex::new(None)),
        }
    }
}

impl InputRecorder for WindowsInputRecorder {
    fn start(&mut self) -> Result<(), InputError> {
        if self.running {
            return Err(InputError::AlreadyRunning);
        }

        self.running = true;
        let events = Arc::clone(&self.events);
        let clock = self.clock.clone();
        let hwnd_store = Arc::clone(&self.hwnd);

        thread::spawn(move || {
            if let Err(e) = run_raw_input_loop(events, clock, hwnd_store) {
                eprintln!("[GameClip] Raw input loop error: {e}");
            }
        });

        Ok(())
    }

    fn stop(&mut self) -> Result<(), InputError> {
        if !self.running {
            return Err(InputError::NotStarted);
        }
        self.running = false;

        // Post WM_QUIT to break the message loop
        if let Ok(hwnd_guard) = self.hwnd.lock() {
            if let Some(sendable) = *hwnd_guard {
                unsafe {
                    let _ = PostMessageW(Some(sendable.to_hwnd()), WM_QUIT, WPARAM(0), LPARAM(0));
                }
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

        let mut events = self.events.lock().map_err(|e| {
            InputError::Platform(format!("Failed to lock events: {e}"))
        })?;

        let drained: Vec<InputEvent> = events.drain(..).collect();
        Ok(drained)
    }
}

// Thread-local state for the raw input window procedure.
thread_local! {
    static RAW_INPUT_STATE: std::cell::RefCell<Option<RawInputState>> = const { std::cell::RefCell::new(None) };
}

struct RawInputState {
    events: Arc<Mutex<Vec<InputEvent>>>,
    clock: SyncClock,
}

/// Run the Windows message loop for raw input on a dedicated thread.
fn run_raw_input_loop(
    events: Arc<Mutex<Vec<InputEvent>>>,
    clock: SyncClock,
    hwnd_store: Arc<Mutex<Option<SendableHwnd>>>,
) -> Result<(), InputError> {
    unsafe {
        let hinstance = GetModuleHandleW(PCWSTR::null())
            .map_err(|e| InputError::Platform(format!("GetModuleHandle failed: {e}")))?;

        let class_name: Vec<u16> = "GameClipRawInput\0".encode_utf16().collect();

        let wc = WNDCLASSW {
            lpfnWndProc: Some(raw_input_wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };

        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR::null(),
            WINDOW_STYLE::default(),
            0, 0, 0, 0,
            Some(HWND_MESSAGE),
            Some(HMENU::default()),
            Some(hinstance.into()),
            None,
        )
        .map_err(|e| InputError::Platform(format!("CreateWindowEx failed: {e}")))?;

        // Store HWND so stop() can post WM_QUIT
        if let Ok(mut store) = hwnd_store.lock() {
            *store = Some(SendableHwnd::from_hwnd(hwnd));
        }

        // Register for raw keyboard and mouse input
        let devices = [
            RAWINPUTDEVICE {
                usUsagePage: 0x01, // HID_USAGE_PAGE_GENERIC
                usUsage: 0x06,    // HID_USAGE_GENERIC_KEYBOARD
                dwFlags: RIDEV_INPUTSINK,
                hwndTarget: hwnd,
            },
            RAWINPUTDEVICE {
                usUsagePage: 0x01,
                usUsage: 0x02,    // HID_USAGE_GENERIC_MOUSE
                dwFlags: RIDEV_INPUTSINK,
                hwndTarget: hwnd,
            },
        ];

        RegisterRawInputDevices(&devices, size_of::<RAWINPUTDEVICE>() as u32)
            .map_err(|e| InputError::Platform(format!("RegisterRawInputDevices failed: {e}")))?;

        // Set thread-local state for the window procedure
        RAW_INPUT_STATE.with(|state| {
            *state.borrow_mut() = Some(RawInputState {
                events,
                clock,
            });
        });

        // Message loop — exits when WM_QUIT is posted via stop()
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, Some(hwnd), 0, 0).as_bool() {
            let _ = DispatchMessageW(&msg);
        }

        // Clear HWND on exit
        if let Ok(mut store) = hwnd_store.lock() {
            *store = None;
        }
    }

    Ok(())
}

/// Window procedure that handles WM_INPUT messages.
unsafe extern "system" fn raw_input_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_INPUT {
        process_raw_input(lparam);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Process a single WM_INPUT message and push the event.
///
/// Uses `MaybeUninit<RAWINPUT>` for proper alignment instead of casting
/// from a `Vec<u8>` buffer.
unsafe fn process_raw_input(lparam: LPARAM) {
    let hrawinput = HRAWINPUT(lparam.0 as _);
    let mut size: u32 = 0;

    let header_size = size_of::<RAWINPUTHEADER>() as u32;
    if GetRawInputData(hrawinput, RID_INPUT, None, &mut size, header_size) == u32::MAX {
        return;
    }

    // Use properly aligned buffer via MaybeUninit
    let mut raw_input = MaybeUninit::<RAWINPUT>::uninit();
    let buf_size = size_of::<RAWINPUT>() as u32;
    let mut actual_size = buf_size.max(size);

    if GetRawInputData(
        hrawinput,
        RID_INPUT,
        Some(raw_input.as_mut_ptr() as *mut _),
        &mut actual_size,
        header_size,
    ) == u32::MAX
    {
        return;
    }

    let raw = raw_input.assume_init_ref();

    RAW_INPUT_STATE.with(|state| {
        let state = state.borrow();
        let Some(ref state) = *state else { return };

        let timestamp_us = state.clock.now_us();

        let kind = if raw.header.dwType == RIM_TYPEKEYBOARD.0 {
            let kb = raw.data.keyboard;
            let vk = kb.VKey;
            // RI_KEY_MAKE = 0 (key down), RI_KEY_BREAK = 1 (key up)
            // Flags is a u16 in windows 0.59
            let pressed = kb.Flags & 0x01 == 0;

            Some(InputEventKind::Key(KeyEvent {
                key: vk_to_key_string(vk),
                pressed,
            }))
        } else if raw.header.dwType == RIM_TYPEMOUSE.0 {
            let mouse = raw.data.mouse;
            let button_flags = mouse.Anonymous.Anonymous.usButtonFlags;

            if button_flags != 0 {
                parse_mouse_button(button_flags, mouse.lLastX as f64, mouse.lLastY as f64)
            } else if mouse.lLastX != 0 || mouse.lLastY != 0 {
                Some(InputEventKind::MouseMove(MouseMoveEvent {
                    x: mouse.lLastX as f64,
                    y: mouse.lLastY as f64,
                }))
            } else if mouse.Anonymous.Anonymous.usButtonData != 0 {
                let delta = mouse.Anonymous.Anonymous.usButtonData as i16;
                Some(InputEventKind::MouseScroll(MouseScrollEvent {
                    delta_x: 0.0,
                    delta_y: delta as f64 / 120.0,
                }))
            } else {
                None
            }
        } else {
            None
        };

        if let Some(kind) = kind {
            let event = InputEvent { timestamp_us, kind };
            if let Ok(mut events) = state.events.lock() {
                events.push(event);
            }
        }
    });
}

/// Parse mouse button flags into an InputEventKind.
fn parse_mouse_button(flags: u16, x: f64, y: f64) -> Option<InputEventKind> {
    const RI_MOUSE_LEFT_BUTTON_DOWN: u16 = 0x0001;
    const RI_MOUSE_LEFT_BUTTON_UP: u16 = 0x0002;
    const RI_MOUSE_RIGHT_BUTTON_DOWN: u16 = 0x0004;
    const RI_MOUSE_RIGHT_BUTTON_UP: u16 = 0x0008;
    const RI_MOUSE_MIDDLE_BUTTON_DOWN: u16 = 0x0010;
    const RI_MOUSE_MIDDLE_BUTTON_UP: u16 = 0x0020;

    let (button, pressed) = if flags & RI_MOUSE_LEFT_BUTTON_DOWN != 0 {
        (MouseButton::Left, true)
    } else if flags & RI_MOUSE_LEFT_BUTTON_UP != 0 {
        (MouseButton::Left, false)
    } else if flags & RI_MOUSE_RIGHT_BUTTON_DOWN != 0 {
        (MouseButton::Right, true)
    } else if flags & RI_MOUSE_RIGHT_BUTTON_UP != 0 {
        (MouseButton::Right, false)
    } else if flags & RI_MOUSE_MIDDLE_BUTTON_DOWN != 0 {
        (MouseButton::Middle, true)
    } else if flags & RI_MOUSE_MIDDLE_BUTTON_UP != 0 {
        (MouseButton::Middle, false)
    } else {
        return None;
    };

    Some(InputEventKind::MouseButton(MouseButtonEvent {
        button,
        pressed,
        x,
        y,
    }))
}

/// Map a Windows Virtual Key code to a web-style key string.
fn vk_to_key_string(vk: u16) -> String {
    match vk {
        0x41..=0x5A => format!("Key{}", (vk as u8 as char)),
        0x30..=0x39 => format!("Digit{}", (vk as u8 as char)),
        0x70..=0x7B => format!("F{}", vk - 0x70 + 1),
        0x10 => "ShiftLeft".to_string(),
        0x11 => "ControlLeft".to_string(),
        0x12 => "AltLeft".to_string(),
        0xA0 => "ShiftLeft".to_string(),
        0xA1 => "ShiftRight".to_string(),
        0xA2 => "ControlLeft".to_string(),
        0xA3 => "ControlRight".to_string(),
        0xA4 => "AltLeft".to_string(),
        0xA5 => "AltRight".to_string(),
        0x20 => "Space".to_string(),
        0x0D => "Enter".to_string(),
        0x1B => "Escape".to_string(),
        0x09 => "Tab".to_string(),
        0x08 => "Backspace".to_string(),
        0x2E => "Delete".to_string(),
        0x2D => "Insert".to_string(),
        0x24 => "Home".to_string(),
        0x23 => "End".to_string(),
        0x21 => "PageUp".to_string(),
        0x22 => "PageDown".to_string(),
        0x25 => "ArrowLeft".to_string(),
        0x26 => "ArrowUp".to_string(),
        0x27 => "ArrowRight".to_string(),
        0x28 => "ArrowDown".to_string(),
        0xBA => "Semicolon".to_string(),
        0xBB => "Equal".to_string(),
        0xBC => "Comma".to_string(),
        0xBD => "Minus".to_string(),
        0xBE => "Period".to_string(),
        0xBF => "Slash".to_string(),
        0xC0 => "Backquote".to_string(),
        0xDB => "BracketLeft".to_string(),
        0xDC => "Backslash".to_string(),
        0xDD => "BracketRight".to_string(),
        0xDE => "Quote".to_string(),
        0x14 => "CapsLock".to_string(),
        0x5B => "MetaLeft".to_string(),
        0x5C => "MetaRight".to_string(),
        0x60..=0x69 => format!("Numpad{}", vk - 0x60),
        0x6A => "NumpadMultiply".to_string(),
        0x6B => "NumpadAdd".to_string(),
        0x6D => "NumpadSubtract".to_string(),
        0x6E => "NumpadDecimal".to_string(),
        0x6F => "NumpadDivide".to_string(),
        0x90 => "NumLock".to_string(),
        _ => format!("Unknown(0x{vk:02X})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vk_mapping_letters() {
        assert_eq!(vk_to_key_string(0x41), "KeyA");
        assert_eq!(vk_to_key_string(0x57), "KeyW");
        assert_eq!(vk_to_key_string(0x5A), "KeyZ");
    }

    #[test]
    fn vk_mapping_modifiers() {
        assert_eq!(vk_to_key_string(0xA0), "ShiftLeft");
        assert_eq!(vk_to_key_string(0xA1), "ShiftRight");
        assert_eq!(vk_to_key_string(0xA2), "ControlLeft");
    }

    #[test]
    fn vk_mapping_common_keys() {
        assert_eq!(vk_to_key_string(0x20), "Space");
        assert_eq!(vk_to_key_string(0x0D), "Enter");
        assert_eq!(vk_to_key_string(0x1B), "Escape");
    }

    #[test]
    fn vk_mapping_arrows() {
        assert_eq!(vk_to_key_string(0x25), "ArrowLeft");
        assert_eq!(vk_to_key_string(0x26), "ArrowUp");
        assert_eq!(vk_to_key_string(0x27), "ArrowRight");
        assert_eq!(vk_to_key_string(0x28), "ArrowDown");
    }

    #[test]
    fn vk_mapping_function_keys() {
        assert_eq!(vk_to_key_string(0x70), "F1");
        assert_eq!(vk_to_key_string(0x7B), "F12");
    }

    #[test]
    fn parse_mouse_left_click() {
        let result = parse_mouse_button(0x0001, 100.0, 200.0);
        assert!(result.is_some());
        if let Some(InputEventKind::MouseButton(btn)) = result {
            assert_eq!(btn.button, MouseButton::Left);
            assert!(btn.pressed);
        }
    }

    #[test]
    fn parse_mouse_right_release() {
        let result = parse_mouse_button(0x0008, 0.0, 0.0);
        assert!(result.is_some());
        if let Some(InputEventKind::MouseButton(btn)) = result {
            assert_eq!(btn.button, MouseButton::Right);
            assert!(!btn.pressed);
        }
    }
}
