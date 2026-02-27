use crate::audio::AudioCapture;
use crate::audio::AudioConfig;
use crate::capture::{CaptureConfig, CapturedFrame, FramePixelFormat, ScreenCapture};
use crate::clip::saver::{ClipSaver, EncodedVideoData};
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
const POLL_INTERVAL_MS: u64 = 2;

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

/// Returns the native pixel format for the current platform's capture backend.
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn platform_pixel_format() -> FramePixelFormat {
    FramePixelFormat::Bgra
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn platform_pixel_format() -> FramePixelFormat {
    FramePixelFormat::Rgba
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
        // Input recorder failure is non-fatal — capture continues without input overlay
        let input_active = match input.start() {
            Ok(()) => true,
            Err(e) => {
                warn!("Input recorder unavailable, continuing without input capture: {e}");
                false
            }
        };
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
                pixel_format: platform_pixel_format(),
            };
            match enc.start(streaming_config) {
                Ok(()) => {
                    info!("Streaming encoder active — frames will be encoded in real-time");
                    let gop_frames = settings.capture_fps * crate::clip::streaming::GOP_MULTIPLIER;
                    let fragment_duration_us =
                        (gop_frames as u64 * 1_000_000) / settings.capture_fps as u64;
                    // Enable encoded buffer in saver
                    let mut s = lock_or_recover(&saver);
                    s.enable_encoded_buffer(
                        settings.buffer_duration_secs,
                        fragment_duration_us,
                        settings.capture_fps,
                    );
                    streaming_encoder = Some(Box::new(enc));
                }
                Err(e) => {
                    warn!("Streaming encoder unavailable, falling back to raw buffer: {e}");
                }
            }
        }

        // Track the first frame's SyncClock timestamp for offsetting
        // synthetic chunk timestamps from the streaming encoder.
        let mut encoder_ts_offset: Option<u64> = None;

        // Frame pacing state: ensures the encoder receives frames at the
        // target rate. ScreenCaptureKit may deliver fewer frames than
        // configured (e.g., ~10-15fps for static desktop content), but
        // FFmpeg's `-r fps` input flag assumes each raw frame is exactly
        // 1/fps seconds apart. Without pacing, lower capture rates cause
        // the video to play faster than real-time (2x for half-rate).
        let frame_interval_us = 1_000_000u64 / settings.capture_fps as u64;
        let mut last_captured_frame: Option<CapturedFrame> = None;
        let mut next_encoder_push_us: Option<u64> = None;
        // Burst cap: max frames pushed in one iteration to avoid overwhelming
        // the encoder after pauses. Allows recovery from ~200ms gaps.
        let max_burst = (settings.capture_fps / 5).max(2) as u64;

        // Capture diagnostics: logged every 10 seconds
        let mut stats_capture_count: u64 = 0;
        let mut stats_push_count: u64 = 0;
        let mut stats_dup_count: u64 = 0;
        let mut last_stats_us: u64 = 0;
        const STATS_INTERVAL_US: u64 = 10_000_000;

        info!(
            "Capture engine started ({}x{} @ {}fps, streaming={}, frame_interval={}us)",
            settings.capture_width,
            settings.capture_height,
            settings.capture_fps,
            streaming_encoder.is_some(),
            frame_interval_us,
        );

        while running.load(Ordering::Relaxed) {
            let now_us = clock.now_us();
            let mut pending_chunks = Vec::new();
            let mut encoder_dead = false;

            // Step 1: Drain ALL available frames from capture source (FIFO).
            // Previous design polled once per iteration, losing real frames
            // that piled up in the buffer between 5ms sleeps.
            let mut drained_frames: Vec<CapturedFrame> = Vec::new();
            loop {
                match screen.poll_frame() {
                    Ok(Some(frame)) => drained_frames.push(frame),
                    Ok(None) => break,
                    Err(e) => {
                        warn!("Screen poll error: {e}");
                        break;
                    }
                }
            }
            let real_frame_count = drained_frames.len() as u64;
            stats_capture_count += real_frame_count;

            // Step 2: Feed frames to streaming encoder using two-phase pacing.
            //
            // Phase A: Push every real captured frame to the encoder, advancing
            //          the schedule by one frame interval per push. This ensures
            //          real motion data is never discarded.
            // Phase B: If the schedule still demands frames AND no real frames
            //          were available this iteration, duplicate the last frame
            //          (static content fill). If real frames were available,
            //          skip duplication — the schedule will catch up naturally.
            if streaming_encoder.is_some() {
                // Store drained frames; keep last for possible duplication
                if !drained_frames.is_empty() {
                    // Keep the very last drained frame as the duplication source
                    last_captured_frame = Some(drained_frames.last().unwrap().clone());
                }

                // Scoped borrow: use enc for push + poll, release before
                // potential take() in the encoder_dead branch below.
                if let Some(ref mut enc) = streaming_encoder {
                    // Initialize schedule on first frame
                    if !drained_frames.is_empty() && next_encoder_push_us.is_none() {
                        next_encoder_push_us = Some(now_us);
                    }

                    // If schedule drifted more than 1 second behind (e.g.,
                    // system pause), reset to avoid a burst of catch-up frames.
                    if next_encoder_push_us
                        .is_some_and(|due| now_us.saturating_sub(due) > 1_000_000)
                    {
                        warn!(
                            "Frame schedule drifted >1s behind, resetting (was {}us behind)",
                            now_us.saturating_sub(
                                next_encoder_push_us.unwrap_or(now_us)
                            )
                        );
                        next_encoder_push_us = Some(now_us);
                    }

                    // Phase A: Push every real frame, advancing schedule per push.
                    for frame in &drained_frames {
                        if encoder_dead {
                            break;
                        }
                        if let Err(e) = enc.push_frame(frame) {
                            warn!("Streaming encoder push failed: {e}");
                            encoder_dead = true;
                            break;
                        }
                        stats_push_count += 1;
                        next_encoder_push_us =
                            next_encoder_push_us.map(|d| d + frame_interval_us);
                    }

                    // Phase B: Duplicate only when no real frames arrived AND
                    // the schedule demands it (static content fill).
                    if !encoder_dead && real_frame_count == 0 {
                        if let Some(ref frame) = last_captured_frame {
                            let mut dup_pushes = 0u64;
                            while next_encoder_push_us.is_some_and(|due| due <= now_us)
                                && dup_pushes < max_burst
                            {
                                if let Err(e) = enc.push_frame(frame) {
                                    warn!("Streaming encoder push failed: {e}");
                                    encoder_dead = true;
                                    break;
                                }
                                dup_pushes += 1;
                                stats_push_count += 1;
                                stats_dup_count += 1;
                                next_encoder_push_us =
                                    next_encoder_push_us.map(|d| d + frame_interval_us);
                            }
                        }
                    }

                    if !encoder_dead && encoder_ts_offset.is_none() {
                        encoder_ts_offset = enc.first_frame_timestamp_us();
                    }

                    // Poll all available encoded chunks and offset timestamps
                    if !encoder_dead {
                        loop {
                            match enc.poll_chunk() {
                                Ok(Some(mut chunk)) => {
                                    if let Some(offset) = encoder_ts_offset {
                                        chunk.timestamp_us =
                                            chunk.timestamp_us.saturating_add(offset);
                                    }
                                    pending_chunks.push(chunk);
                                }
                                Ok(None) => break,
                                Err(e) => {
                                    warn!("Streaming encoder poll failed: {e}");
                                    encoder_dead = true;
                                    break;
                                }
                            }
                        }
                    }
                }
                // enc borrow released here

                if encoder_dead {
                    // Encoder died: clean up and clear stale data
                    warn!("Streaming encoder died, falling back to raw frame buffer");
                    if let Some(mut dead_enc) = streaming_encoder.take() {
                        if let Err(e) = dead_enc.stop() {
                            warn!("Error stopping dead encoder: {e}");
                        }
                    }
                    encoder_ts_offset = None;
                    next_encoder_push_us = None;
                    {
                        let mut s = lock_or_recover(&saver);
                        if let Some(ref mut buf) = s.encoded_chunks {
                            buf.clear();
                        }
                        // Push current frame to raw buffer as fallback
                        if let Some(frame) = last_captured_frame.take() {
                            s.push_frame(frame);
                        }
                    }
                } else {
                    // Cache thumbnail on new frames, push encoded chunks
                    let needs_lock = real_frame_count > 0 || !pending_chunks.is_empty();
                    if needs_lock {
                        let mut s = lock_or_recover(&saver);
                        if real_frame_count > 0 {
                            if let Some(ref frame) = last_captured_frame {
                                s.cache_first_raw_frame(frame.clone());
                            }
                        }
                        for chunk in pending_chunks.drain(..) {
                            s.push_encoded_chunk(chunk);
                        }
                    }
                }
            } else {
                // No streaming encoder — push all drained frames to raw buffer
                if !drained_frames.is_empty() {
                    let mut s = lock_or_recover(&saver);
                    for frame in drained_frames {
                        s.push_frame(frame);
                    }
                }
            }

            if input_active {
                if let Ok(events) = input.poll_events() {
                    if !events.is_empty() {
                        let mut s = lock_or_recover(&saver);
                        for event in events {
                            s.push_input(event);
                        }
                    }
                }
            }

            if let Ok(Some(buffer)) = audio.poll_buffer() {
                let mut s = lock_or_recover(&saver);
                s.push_audio(buffer);
            }

            // Periodic capture rate diagnostics
            if now_us.saturating_sub(last_stats_us) >= STATS_INTERVAL_US && last_stats_us > 0 {
                let elapsed = (now_us - last_stats_us) as f64 / 1_000_000.0;
                info!(
                    "Capture stats: capture={:.1}fps, encoder_push={:.1}fps (target={}fps), \
                     duplicated={} frames ({:.0}%)",
                    stats_capture_count as f64 / elapsed,
                    stats_push_count as f64 / elapsed,
                    settings.capture_fps,
                    stats_dup_count,
                    if stats_push_count > 0 {
                        stats_dup_count as f64 / stats_push_count as f64 * 100.0
                    } else {
                        0.0
                    },
                );
                stats_capture_count = 0;
                stats_push_count = 0;
                stats_dup_count = 0;
                last_stats_us = now_us;
            } else if last_stats_us == 0 {
                last_stats_us = now_us;
            }

            thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
        }

        // Clean shutdown — log session stats
        if let Some(mut enc) = streaming_encoder {
            let dropped = enc.dropped_frame_count();
            if dropped > 0 {
                warn!("Capture session: {dropped} frames dropped due to encoder backpressure");
            }
            if let Err(e) = enc.stop() {
                warn!("Error stopping streaming encoder: {e}");
            }
        }
        let _ = screen.stop();
        if input_active {
            let _ = input.stop();
        }
        let _ = audio.stop();
        info!("Capture engine stopped");
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
        let encoding_fps = s.encoding_fps();
        let encoded_video = s.encoded_chunks.as_mut().map(|buf| {
            let time_span = buf.time_span_us();
            let video_data = buf.drain_as_fmp4();
            let first_frame = buf.take_first_frame();
            EncodedVideoData {
                fmp4_bytes: video_data,
                first_frame,
                time_span_us: time_span,
                encoding_fps,
            }
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
