pub mod mock;

#[cfg(target_os = "windows")]
pub mod windows;

use crate::sync::clock::TimestampUs;
use serde::{Deserialize, Serialize};

/// A keyboard key event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KeyEvent {
    /// The key code (e.g., "KeyW", "Space", "ShiftLeft").
    pub key: String,
    /// Whether the key was pressed or released.
    pub pressed: bool,
}

/// A mouse button identifier.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// A mouse button event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MouseButtonEvent {
    pub button: MouseButton,
    pub pressed: bool,
    pub x: f64,
    pub y: f64,
}

/// A mouse movement event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MouseMoveEvent {
    pub x: f64,
    pub y: f64,
}

/// A mouse scroll event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MouseScrollEvent {
    pub delta_x: f64,
    pub delta_y: f64,
}

/// An input event with its type and timestamp.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum InputEventKind {
    #[serde(rename = "key")]
    Key(KeyEvent),
    #[serde(rename = "mouse_button")]
    MouseButton(MouseButtonEvent),
    #[serde(rename = "mouse_move")]
    MouseMove(MouseMoveEvent),
    #[serde(rename = "mouse_scroll")]
    MouseScroll(MouseScrollEvent),
}

/// A timestamped input event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InputEvent {
    /// Microsecond timestamp from SyncClock.
    pub timestamp_us: TimestampUs,
    #[serde(flatten)]
    pub kind: InputEventKind,
}

/// Error type for input recording operations.
#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("recorder not started")]
    NotStarted,
    #[error("recorder already running")]
    AlreadyRunning,
    #[error("platform error: {0}")]
    #[allow(dead_code)]
    Platform(String),
}

/// Platform-abstracted input recorder interface.
///
/// Records keyboard and mouse events with timestamps synchronized to
/// the SyncClock. The mock implementation generates random input events
/// for development on non-Windows platforms.
pub trait InputRecorder: Send {
    /// Start recording input events.
    fn start(&mut self) -> Result<(), InputError>;

    /// Stop recording input events.
    fn stop(&mut self) -> Result<(), InputError>;

    /// Returns true if the recorder is currently running.
    #[allow(dead_code)]
    fn is_running(&self) -> bool;

    /// Drain all buffered input events since the last poll.
    fn poll_events(&mut self) -> Result<Vec<InputEvent>, InputError>;
}
