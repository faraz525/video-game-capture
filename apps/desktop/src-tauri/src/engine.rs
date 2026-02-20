use crate::audio::AudioCapture;
use crate::audio::AudioConfig;
use crate::capture::{CaptureConfig, ScreenCapture};
use crate::clip::saver::ClipSaver;
use crate::input::InputRecorder;
use crate::sync::clock::SyncClock;
use log::{error, info, warn};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const DEFAULT_BUFFER_SECS: u32 = 30;
const POLL_INTERVAL_MS: u64 = 5;

/// Application-wide capture engine state, shared across threads.
pub struct EngineState {
    pub saver: Arc<Mutex<ClipSaver>>,
    pub running: Arc<AtomicBool>,
    pub settings: Mutex<AppSettings>,
}

/// User-configurable settings.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppSettings {
    pub buffer_duration_secs: u32,
    pub save_directory: String,
    pub hotkey: String,
    pub capture_fps: u32,
    pub capture_width: u32,
    pub capture_height: u32,
}

impl Default for AppSettings {
    fn default() -> Self {
        let save_dir = dirs_default_clips_dir();
        Self {
            buffer_duration_secs: DEFAULT_BUFFER_SECS,
            save_directory: save_dir,
            hotkey: "Ctrl+Shift+R".to_string(),
            capture_fps: 30,
            capture_width: 640,
            capture_height: 360,
        }
    }
}

fn dirs_default_clips_dir() -> String {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    let path = PathBuf::from(home).join("GameClip").join("clips");
    path.to_string_lossy().to_string()
}

/// Create the platform-appropriate screen capture implementation.
#[cfg(target_os = "windows")]
fn create_screen_capture(clock: SyncClock) -> Box<dyn ScreenCapture> {
    Box::new(crate::capture::windows::WindowsCapture::new(clock))
}

#[cfg(target_os = "macos")]
fn create_screen_capture(clock: SyncClock) -> Box<dyn ScreenCapture> {
    Box::new(crate::capture::macos::MacOSCapture::new(clock))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn create_screen_capture(clock: SyncClock) -> Box<dyn ScreenCapture> {
    Box::new(crate::capture::mock::MockCapture::new(clock))
}

/// Create the platform-appropriate input recorder implementation.
#[cfg(target_os = "windows")]
fn create_input_recorder(clock: SyncClock) -> Box<dyn InputRecorder> {
    Box::new(crate::input::windows::WindowsInputRecorder::new(clock))
}

#[cfg(target_os = "macos")]
fn create_input_recorder(clock: SyncClock) -> Box<dyn InputRecorder> {
    Box::new(crate::input::macos::MacOSInputRecorder::new(clock))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn create_input_recorder(clock: SyncClock) -> Box<dyn InputRecorder> {
    Box::new(crate::input::mock::MockInputRecorder::new(clock))
}

/// Create the platform-appropriate audio capture implementation.
#[cfg(target_os = "windows")]
fn create_audio_capture(clock: SyncClock) -> Box<dyn AudioCapture> {
    Box::new(crate::audio::windows::WindowsAudioCapture::new(clock))
}

#[cfg(not(target_os = "windows"))]
fn create_audio_capture(clock: SyncClock) -> Box<dyn AudioCapture> {
    Box::new(crate::audio::mock::MockAudioCapture::new(clock))
}

pub fn create_engine_state() -> EngineState {
    let settings = AppSettings::default();
    let saver = ClipSaver::new(
        settings.buffer_duration_secs,
        PathBuf::from(&settings.save_directory),
    );

    EngineState {
        saver: Arc::new(Mutex::new(saver)),
        running: Arc::new(AtomicBool::new(false)),
        settings: Mutex::new(settings),
    }
}

/// Start the background capture loop using platform-appropriate implementations.
///
/// The capture thread pushes data directly into the shared `ClipSaver` ring buffers,
/// so `save_clip()` simply drains whatever is buffered.
pub fn start_capture(state: &EngineState) -> Result<(), Box<dyn std::error::Error>> {
    if state.running.load(Ordering::SeqCst) {
        return Ok(());
    }
    state.running.store(true, Ordering::SeqCst);

    let settings = state.settings.lock().map_err(|e| e.to_string())?.clone();
    let saver = Arc::clone(&state.saver);
    let running = Arc::clone(&state.running);

    thread::spawn(move || {
        let clock = SyncClock::new();
        let mut screen = create_screen_capture(clock.clone());
        let mut input = create_input_recorder(clock.clone());
        let mut audio = create_audio_capture(clock.clone());

        let config = CaptureConfig {
            target_fps: settings.capture_fps,
            width: settings.capture_width,
            height: settings.capture_height,
        };
        let audio_config = AudioConfig::default();

        if let Err(e) = screen.start(config) {
            error!("Failed to start capture: {e}");
            running.store(false, Ordering::SeqCst);
            return;
        }
        if let Err(e) = input.start() {
            error!("Failed to start input recorder: {e}");
            running.store(false, Ordering::SeqCst);
            return;
        }
        if let Err(e) = audio.start(audio_config) {
            error!("Failed to start audio capture: {e}");
            running.store(false, Ordering::SeqCst);
            return;
        }

        info!("Capture engine started ({}x{} @ {}fps)", settings.capture_width, settings.capture_height, settings.capture_fps);

        while running.load(Ordering::Relaxed) {
            if let Ok(Some(frame)) = screen.poll_frame() {
                let mut s = lock_or_recover(&saver);
                s.push_frame(frame);
            }

            if let Ok(events) = input.poll_events() {
                if !events.is_empty() {
                    let mut s = lock_or_recover(&saver);
                    for event in events {
                        s.push_input(event);
                    }
                }
            }

            if let Ok(Some(buffer)) = audio.poll_buffer() {
                let mut s = lock_or_recover(&saver);
                s.push_audio(buffer);
            }

            thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
        }

        // Clean shutdown
        let _ = screen.stop();
        let _ = input.stop();
        let _ = audio.stop();
    });

    Ok(())
}

/// Lock the saver mutex, recovering from poison if necessary.
fn lock_or_recover(saver: &Arc<Mutex<ClipSaver>>) -> std::sync::MutexGuard<'_, ClipSaver> {
    saver.lock().unwrap_or_else(|poisoned| {
        warn!("Saver mutex poisoned, recovering");
        poisoned.into_inner()
    })
}

/// Save a clip from the current ring buffer contents.
///
/// Drains the ring buffers quickly (minimizing lock contention with the capture
/// thread), then performs the heavy work (thumbnail generation, encoding, zip
/// packaging) outside the lock.
///
/// Takes an `Arc<Mutex<ClipSaver>>` directly so callers can clone it and
/// spawn this on a background thread without holding a reference to EngineState.
pub fn save_clip(saver: &Arc<Mutex<ClipSaver>>) -> Result<PathBuf, String> {
    let (frames, input_events, audio_buffers, save_dir) = {
        let mut s = lock_or_recover(saver);
        let frames = s.frames.drain();
        let input_events = s.input_events.drain();
        let audio_buffers = s.audio_buffers.drain();
        let save_dir = s.save_dir().to_path_buf();
        (frames, input_events, audio_buffers, save_dir)
    };
    // Lock released here — capture thread resumes immediately

    let game_name = detect_current_game();
    ClipSaver::save_clip_from_data(frames, input_events, audio_buffers, game_name, &save_dir)
        .map_err(|e| e.to_string())
}

/// Detect the currently running game (if any).
///
/// Uses foreground window detection on Windows, falls back to process scan
/// on all platforms.
fn detect_current_game() -> Option<String> {
    crate::game::detector::detect_current_game()
}
