use crate::audio::AudioCapture;
use crate::audio::AudioConfig;
use crate::capture::{CaptureConfig, ScreenCapture};
use crate::clip::saver::ClipSaver;
use crate::clip::streaming::{FfmpegStreamingEncoder, StreamingConfig, StreamingEncoder};
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
    pub upload_cancel: Mutex<Arc<AtomicBool>>,
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
    /// HuggingFace upload configuration.
    #[serde(default)]
    pub huggingface: crate::upload::hf_client::HuggingFaceConfig,
}

impl Default for AppSettings {
    fn default() -> Self {
        let save_dir = dirs_default_clips_dir();
        Self {
            buffer_duration_secs: DEFAULT_BUFFER_SECS,
            save_directory: save_dir,
            hotkey: "Ctrl+Shift+R".to_string(),
            capture_fps: 30,
            capture_width: 1920,
            capture_height: 1080,
            huggingface: crate::upload::hf_client::HuggingFaceConfig::default(),
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
        upload_cancel: Mutex::new(Arc::new(AtomicBool::new(false))),
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

        // Try to start streaming encoder for in-capture encoding
        let mut streaming_encoder: Option<Box<dyn StreamingEncoder>> = None;
        {
            let mut enc = FfmpegStreamingEncoder::new();
            let streaming_config = StreamingConfig {
                width: settings.capture_width,
                height: settings.capture_height,
                fps: settings.capture_fps,
            };
            match enc.start(streaming_config) {
                Ok(()) => {
                    info!("Streaming encoder active — frames will be encoded in real-time");
                    // Enable encoded buffer in saver
                    let mut s = lock_or_recover(&saver);
                    s.enable_encoded_buffer(settings.buffer_duration_secs);
                    streaming_encoder = Some(Box::new(enc));
                }
                Err(e) => {
                    warn!("Streaming encoder unavailable, falling back to raw buffer: {e}");
                }
            }
        }

        info!(
            "Capture engine started ({}x{} @ {}fps, streaming={})",
            settings.capture_width,
            settings.capture_height,
            settings.capture_fps,
            streaming_encoder.is_some()
        );

        while running.load(Ordering::Relaxed) {
            // Collect encoded chunks outside the lock to batch the push
            let mut pending_chunks = Vec::new();

            if let Ok(Some(frame)) = screen.poll_frame() {
                // Feed frame to streaming encoder if available
                if let Some(ref mut enc) = streaming_encoder {
                    if let Err(e) = enc.push_frame(&frame) {
                        warn!("Streaming encoder push failed: {e}");
                    }

                    // Poll all available encoded chunks
                    while let Ok(Some(chunk)) = enc.poll_chunk() {
                        pending_chunks.push(chunk);
                    }

                    // Single lock acquisition for frame cache + encoded chunks
                    let mut s = lock_or_recover(&saver);
                    // Cache first raw frame for thumbnail (buffer handles dedup)
                    s.cache_first_raw_frame(frame.clone());
                    for chunk in pending_chunks.drain(..) {
                        s.push_encoded_chunk(chunk);
                    }
                } else {
                    // No streaming encoder — use raw frame buffer
                    let mut s = lock_or_recover(&saver);
                    s.push_frame(frame);
                }
            } else if let Some(ref mut enc) = streaming_encoder {
                // No frame this tick, but still poll for encoded chunks
                while let Ok(Some(chunk)) = enc.poll_chunk() {
                    pending_chunks.push(chunk);
                }
                if !pending_chunks.is_empty() {
                    let mut s = lock_or_recover(&saver);
                    for chunk in pending_chunks {
                        s.push_encoded_chunk(chunk);
                    }
                }
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
        if let Some(mut enc) = streaming_encoder {
            if let Err(e) = enc.stop() {
                warn!("Error stopping streaming encoder: {e}");
            }
        }
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
    let (frames, input_events, audio_buffers, encoded_video, save_dir) = {
        let mut s = lock_or_recover(saver);
        let frames = s.frames.drain();
        let input_events = s.input_events.drain();
        let audio_buffers = s.audio_buffers.drain();
        let save_dir = s.save_dir().to_path_buf();

        // Drain encoded video data if streaming encoder was active
        let encoded_video = s.encoded_chunks.as_mut().map(|buf| {
            let video_data = buf.drain_as_fmp4();
            let first_frame = buf.take_first_frame();
            (video_data, first_frame)
        });

        (frames, input_events, audio_buffers, encoded_video, save_dir)
    };
    // Lock released here — capture thread resumes immediately

    let game_name = detect_current_game();
    ClipSaver::save_clip_from_data(
        frames,
        input_events,
        audio_buffers,
        encoded_video,
        game_name,
        &save_dir,
    )
    .map_err(|e| e.to_string())
}

/// Detect the currently running game (if any).
///
/// Uses foreground window detection on Windows, falls back to process scan
/// on all platforms.
fn detect_current_game() -> Option<String> {
    crate::game::detector::detect_current_game()
}
