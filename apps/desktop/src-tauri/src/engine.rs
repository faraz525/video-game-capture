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
///
/// Prefers NV12 when available (macOS always, Windows when D3D11 VideoProcessor
/// succeeds). Falls back to BGRA/RGBA otherwise.
fn capture_pixel_format(screen: &dyn ScreenCapture) -> FramePixelFormat {
    if screen.nv12_active() {
        FramePixelFormat::Nv12
    } else {
        platform_default_pixel_format()
    }
}

/// Compile-time default pixel format (used before runtime detection is available).
#[cfg(target_os = "windows")]
fn platform_default_pixel_format() -> FramePixelFormat {
    FramePixelFormat::Bgra
}

#[cfg(target_os = "macos")]
fn platform_default_pixel_format() -> FramePixelFormat {
    FramePixelFormat::Nv12
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn platform_default_pixel_format() -> FramePixelFormat {
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
            adapter_index: 0,
            display_index: 0,
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

        // Try to start streaming encoder for in-capture encoding.
        // On Windows with NV12 active, try MediaFoundation GPU encoder first
        // (zero-copy), falling back to FFmpeg.
        let active_pixel_format = capture_pixel_format(screen.as_ref());
        let mut streaming_encoder: Option<Box<dyn StreamingEncoder>> = None;
        let mut force_software_codec = false;
        #[cfg(target_os = "windows")]
        let streaming_allowed = if active_pixel_format == FramePixelFormat::Nv12 {
            true
        } else {
            warn!(
                "Windows capture running in BGRA fallback mode; disabling live streaming encoder \
                 to avoid geometry mismatch with desktop-duplication native resolution"
            );
            false
        };

        #[cfg(not(target_os = "windows"))]
        let streaming_allowed = true;

        if streaming_allowed {
            let streaming_config = StreamingConfig {
                width: settings.capture_width,
                height: settings.capture_height,
                fps: settings.capture_fps,
                pixel_format: active_pixel_format,
            };

            // Phase 4: Try MediaFoundation zero-copy GPU encoder on Windows
            #[cfg(target_os = "windows")]
            if active_pixel_format == FramePixelFormat::Nv12 {
                let mut mf_enc = crate::clip::mf_encoder::MfStreamingEncoder::new();
                match mf_enc.start(streaming_config.clone()) {
                    Ok(()) => {
                        info!("MediaFoundation GPU encoder active — zero-copy encoding");
                        let gop_frames =
                            settings.capture_fps * crate::clip::streaming::GOP_MULTIPLIER;
                        let fragment_duration_us =
                            (gop_frames as u64 * 1_000_000) / settings.capture_fps as u64;
                        let mut s = lock_or_recover(&saver);
                        s.enable_encoded_buffer(
                            settings.buffer_duration_secs,
                            fragment_duration_us,
                            settings.capture_fps,
                        );
                        streaming_encoder = Some(Box::new(mf_enc));
                    }
                    Err(e) => {
                        warn!("MediaFoundation encoder unavailable, trying FFmpeg: {e}");
                    }
                }
            }

            // Fallback: FFmpeg-based encoder (all platforms)
            if streaming_encoder.is_none() {
                let mut enc = FfmpegStreamingEncoder::new();
                match enc.start(streaming_config) {
                    Ok(()) => {
                        info!("Streaming encoder active — frames will be encoded in real-time");
                        let gop_frames =
                            settings.capture_fps * crate::clip::streaming::GOP_MULTIPLIER;
                        let fragment_duration_us =
                            (gop_frames as u64 * 1_000_000) / settings.capture_fps as u64;
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
        // Frame data is wrapped in Arc to share with the encoder channel
        // via an 8-byte pointer bump instead of ~8MB data copy per push.
        let mut last_frame_data: Option<Arc<Vec<u8>>> = None;
        let mut last_frame_meta: Option<(u32, u32, u64, crate::capture::FramePixelFormat)> = None;
        let mut next_encoder_push_us: Option<u64> = None;
        // Prevents re-cloning ~8MB every iteration for cache_first_raw_frame
        // which internally guards with `if self.first_raw_frame.is_none()`.
        let mut thumbnail_cached = false;
        // Burst cap: max frames pushed in one iteration to avoid overwhelming
        // the encoder after pauses. Allows recovery from ~200ms gaps.
        let max_burst = (settings.capture_fps / 5).max(2) as u64;

        // Capture diagnostics: logged every 5 seconds
        let mut stats_capture_count: u64 = 0;
        let mut stats_push_count: u64 = 0;
        let mut stats_dup_count: u64 = 0;
        let mut stats_skip_count: u64 = 0;
        let mut stats_drop_count: u64 = 0;
        let mut last_stats_us: u64 = 0;
        const STATS_INTERVAL_US: u64 = 5_000_000;

        // Session-level totals for shutdown summary
        let mut session_push_total: u64 = 0;
        let session_start_us = clock.now_us();

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

            // Step 2: Feed frames to streaming encoder with rate limiting.
            //
            // SCK may deliver at the display refresh rate (~60fps) due to an FFI
            // bug that prevents minimum_frame_interval from being applied. The
            // compute_pacing() function gates pushes to target_fps, using the
            // latest real frame (or duplicating the last-captured frame for
            // static content). Excess real frames are skipped, not pushed.
            if streaming_encoder.is_some() {
                // Move the latest real frame into Arc-wrapped storage.
                // `pop()` moves ownership (0 copies) instead of cloning ~8MB.
                if !drained_frames.is_empty() {
                    let mut frame = drained_frames.pop().unwrap();
                    last_frame_meta = Some((frame.width, frame.height, frame.timestamp_us, frame.pixel_format));
                    // Move data into Arc — single allocation, shared via 8-byte pointer bumps
                    last_frame_data = Some(Arc::new(std::mem::take(&mut frame.data)));
                }

                // Scoped borrow: use enc for push + poll, release before
                // potential take() in the encoder_dead branch below.
                if let Some(ref mut enc) = streaming_encoder {
                    // Unified rate-limited push loop.
                    //
                    // Instead of pushing every real frame (Phase A) then
                    // duplicating (Phase B), we use compute_pacing() to
                    // decide how many pushes the schedule allows. This
                    // rate-limits to target_fps regardless of SCK delivery
                    // rate, fixing the 2x playback speed when SCK delivers
                    // at 60fps (display refresh rate).
                    let pacing = compute_pacing(
                        now_us,
                        next_encoder_push_us,
                        last_frame_data.is_some(),
                        max_burst,
                        frame_interval_us,
                    );
                    if pacing.was_reset {
                        warn!(
                            "Frame schedule drifted >1s behind, resetting (was {}us behind)",
                            now_us.saturating_sub(
                                next_encoder_push_us.unwrap_or(now_us)
                            )
                        );
                    }
                    next_encoder_push_us = pacing.next_due_us;

                    if let Some(ref data_arc) = last_frame_data {
                        let ts = last_frame_meta.map(|(_, _, ts, _)| ts).unwrap_or(0);
                        for _ in 0..pacing.pushes {
                            // Arc::clone is ~8 bytes instead of ~8MB data copy
                            if let Err(e) = enc.push_frame(Arc::clone(data_arc), ts) {
                                warn!("Streaming encoder push failed: {e}");
                                encoder_dead = true;
                                break;
                            }
                            stats_push_count += 1;
                            if real_frame_count == 0 {
                                stats_dup_count += 1;
                            }
                        }
                    }

                    // Track real frames skipped due to rate limiting:
                    // we only use the latest of N real frames per iteration.
                    if real_frame_count > 1 {
                        stats_skip_count += real_frame_count - 1;
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
                    // Encoder died: clean up and attempt restart with software codecs.
                    // If already on software codecs, fall back to raw frame buffer.
                    if let Some(mut dead_enc) = streaming_encoder.take() {
                        if let Err(e) = dead_enc.stop() {
                            warn!("Error stopping dead encoder: {e}");
                        }
                    }
                    encoder_ts_offset = None;
                    next_encoder_push_us = None;
                    thumbnail_cached = false;
                    {
                        let mut s = lock_or_recover(&saver);
                        if let Some(ref mut buf) = s.encoded_chunks {
                            buf.clear();
                        }
                    }

                    if !force_software_codec {
                        // First failure: restart with software-only codecs
                        warn!("Encoder stalled/died, restarting with software-only codecs");
                        force_software_codec = true;
                        let mut enc = FfmpegStreamingEncoder::new();
                        enc.set_force_software(true);
                        let streaming_config = StreamingConfig {
                            width: settings.capture_width,
                            height: settings.capture_height,
                            fps: settings.capture_fps,
                            pixel_format: active_pixel_format,
                        };
                        match enc.start(streaming_config) {
                            Ok(()) => {
                                info!("Streaming encoder restarted with software codec");
                                let gop_frames = settings.capture_fps
                                    * crate::clip::streaming::GOP_MULTIPLIER;
                                let fragment_duration_us = (gop_frames as u64 * 1_000_000)
                                    / settings.capture_fps as u64;
                                let mut s = lock_or_recover(&saver);
                                s.enable_encoded_buffer(
                                    settings.buffer_duration_secs,
                                    fragment_duration_us,
                                    settings.capture_fps,
                                );
                                streaming_encoder = Some(Box::new(enc));
                            }
                            Err(e) => {
                                warn!(
                                    "Software encoder restart failed, falling back to raw: {e}"
                                );
                                // Reconstruct frame for raw buffer fallback
                                if let (Some(data_arc), Some((w, h, ts, pf))) =
                                    (last_frame_data.take(), last_frame_meta.take())
                                {
                                    let data = Arc::try_unwrap(data_arc)
                                        .unwrap_or_else(|arc| (*arc).clone());
                                    let mut s = lock_or_recover(&saver);
                                    s.push_frame(CapturedFrame {
                                        timestamp_us: ts,
                                        width: w,
                                        height: h,
                                        data,
                                        pixel_format: pf,
                                    });
                                }
                            }
                        }
                    } else {
                        // Already on software codecs — fall back to raw buffer
                        warn!("Software encoder also died, falling back to raw frame buffer");
                        if let (Some(data_arc), Some((w, h, ts, pf))) =
                            (last_frame_data.take(), last_frame_meta.take())
                        {
                            let data = Arc::try_unwrap(data_arc)
                                .unwrap_or_else(|arc| (*arc).clone());
                            let mut s = lock_or_recover(&saver);
                            s.push_frame(CapturedFrame {
                                timestamp_us: ts,
                                width: w,
                                height: h,
                                data,
                                pixel_format: pf,
                            });
                        }
                    }
                } else {
                    // Cache thumbnail once, push encoded chunks
                    let needs_lock = (real_frame_count > 0 && !thumbnail_cached) || !pending_chunks.is_empty();
                    if needs_lock {
                        let mut s = lock_or_recover(&saver);
                        if real_frame_count > 0 && !thumbnail_cached {
                            if let (Some(ref data_arc), Some((w, h, ts, pf))) = (&last_frame_data, &last_frame_meta) {
                                // One-time ~8MB clone for thumbnail (not per-iteration)
                                s.cache_first_raw_frame(CapturedFrame {
                                    timestamp_us: *ts,
                                    width: *w,
                                    height: *h,
                                    data: (**data_arc).clone(),
                                    pixel_format: *pf,
                                });
                                thumbnail_cached = true;
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

            // Track encoder drops for this interval
            if let Some(ref enc) = streaming_encoder {
                stats_drop_count = enc.dropped_frame_count();
            }

            // Periodic capture rate diagnostics (every 5 seconds)
            if now_us.saturating_sub(last_stats_us) >= STATS_INTERVAL_US && last_stats_us > 0 {
                let elapsed = (now_us - last_stats_us) as f64 / 1_000_000.0;
                let expected_frames = elapsed * settings.capture_fps as f64;
                let pacing_ratio = if expected_frames > 0.0 {
                    stats_push_count as f64 / expected_frames
                } else {
                    0.0
                };
                let capture_fps = stats_capture_count as f64 / elapsed;
                let push_fps = stats_push_count as f64 / elapsed;
                let drop_rate = if stats_push_count > 0 {
                    stats_drop_count as f64 / (stats_push_count + stats_drop_count) as f64
                } else {
                    0.0
                };
                info!(
                    "Capture stats: capture={capture_fps:.1}fps, \
                     encoder_push={push_fps:.1}fps (target={}fps), \
                     skipped={}, duplicated={}, dropped={}, \
                     drop_rate={drop_rate:.1}%, pacing_ratio={pacing_ratio:.3}",
                    settings.capture_fps,
                    stats_skip_count,
                    stats_dup_count,
                    stats_drop_count,
                    drop_rate = drop_rate * 100.0,
                );
                session_push_total += stats_push_count;
                stats_capture_count = 0;
                stats_push_count = 0;
                stats_dup_count = 0;
                stats_skip_count = 0;
                stats_drop_count = 0;
                last_stats_us = now_us;
            } else if last_stats_us == 0 {
                last_stats_us = now_us;
            }

            thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
        }

        // Clean shutdown — log session stats
        // Include any un-flushed stats from the last partial interval
        session_push_total += stats_push_count;

        let session_elapsed = clock.now_us().saturating_sub(session_start_us) as f64 / 1_000_000.0;
        if session_elapsed > 0.0 {
            let expected = session_elapsed * settings.capture_fps as f64;
            let session_ratio = if expected > 0.0 {
                session_push_total as f64 / expected
            } else {
                0.0
            };
            info!(
                "Session summary: {session_push_total} frames pushed in {session_elapsed:.1}s, \
                 pacing_ratio={session_ratio:.3}"
            );
            if session_ratio > 1.01 {
                warn!(
                    "Session pacing ratio {session_ratio:.3} > 1.01 — video may play slower than real-time"
                );
            }
        }

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

/// Result of the per-iteration pacing computation.
#[derive(Debug, PartialEq)]
struct PacingResult {
    /// Number of frames to push to the encoder this iteration.
    pushes: u64,
    /// Updated schedule timestamp (next push due time).
    next_due_us: Option<u64>,
    /// Whether the schedule was reset due to excessive drift (>1s behind).
    was_reset: bool,
}

/// Compute how many frames to push to the encoder this iteration.
///
/// Rate-limits encoder pushes to the target fps regardless of capture source
/// delivery rate. Uses a schedule-based approach: pushes are only allowed
/// when wall-clock time has advanced past `next_due_us`.
///
/// - `now_us`: current wall-clock time in microseconds
/// - `next_due_us`: when the next push is scheduled (None = uninitialized)
/// - `has_frame`: whether a frame is available to push (real or last-captured)
/// - `max_burst`: max pushes per iteration to prevent encoder overload
/// - `frame_interval_us`: target interval between pushes (1_000_000 / fps)
fn compute_pacing(
    now_us: u64,
    next_due_us: Option<u64>,
    has_frame: bool,
    max_burst: u64,
    frame_interval_us: u64,
) -> PacingResult {
    // Can't push without a frame (real or last-captured)
    if !has_frame {
        return PacingResult {
            pushes: 0,
            next_due_us,
            was_reset: false,
        };
    }

    // Initialize schedule on first available frame
    let due = match next_due_us {
        Some(d) => d,
        None => {
            return PacingResult {
                pushes: 1,
                next_due_us: Some(now_us + frame_interval_us),
                was_reset: false,
            };
        }
    };

    // Reset if schedule drifted > 1 second behind wall time
    let (due, was_reset) = if now_us.saturating_sub(due) > 1_000_000 {
        (now_us, true)
    } else {
        (due, false)
    };

    // Count how many pushes the schedule allows right now
    let mut pushes = 0u64;
    let mut next = due;
    while next <= now_us && pushes < max_burst {
        pushes += 1;
        next += frame_interval_us;
    }

    PacingResult {
        pushes,
        next_due_us: Some(next),
        was_reset,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET_FPS: u64 = 30;
    const FRAME_INTERVAL_US: u64 = 1_000_000 / TARGET_FPS; // 33_333
    const MAX_BURST: u64 = 6; // (30 / 5).max(2)

    #[test]
    fn rate_limiter_caps_pushes_at_target_fps() {
        // Simulate 60fps capture with 30fps target over 10 seconds.
        // The rate limiter should cap pushes to ~300, not pass all 600 through.
        let capture_fps = 60u64;
        let duration_secs = 10u64;
        let total_captured = capture_fps * duration_secs;
        let capture_interval_us = 1_000_000 / capture_fps;

        let mut next_due: Option<u64> = None;
        let mut total_pushes = 0u64;

        for i in 0..total_captured {
            let now_us = i * capture_interval_us;
            let result =
                compute_pacing(now_us, next_due, true, MAX_BURST, FRAME_INTERVAL_US);
            total_pushes += result.pushes;
            next_due = result.next_due_us;
        }

        let expected = TARGET_FPS * duration_secs; // 300
        assert!(
            total_pushes >= expected - 10 && total_pushes <= expected + 10,
            "Expected ~{expected} pushes at 30fps over 10s, got {total_pushes} \
             (ratio: {:.3})",
            total_pushes as f64 / expected as f64,
        );
    }

    #[test]
    fn rate_limiter_allows_exact_rate() {
        // Capture at exactly target fps — all frames should be pushed.
        let duration_secs = 10u64;
        let total_captured = TARGET_FPS * duration_secs;

        let mut next_due: Option<u64> = None;
        let mut total_pushes = 0u64;

        for i in 0..total_captured {
            let now_us = i * FRAME_INTERVAL_US;
            let result =
                compute_pacing(now_us, next_due, true, MAX_BURST, FRAME_INTERVAL_US);
            total_pushes += result.pushes;
            next_due = result.next_due_us;
        }

        let expected = TARGET_FPS * duration_secs; // 300
        assert!(
            total_pushes >= expected - 2 && total_pushes <= expected + 2,
            "Expected ~{expected} pushes at exact target rate, got {total_pushes}",
        );
    }

    #[test]
    fn rate_limiter_duplicates_for_static_content() {
        // Simulate: 1 real frame at t=0, then no real frames for 1 second.
        // The limiter should allow ~30 pushes (duplicates) over that second.
        let first = compute_pacing(0, None, true, MAX_BURST, FRAME_INTERVAL_US);
        assert_eq!(first.pushes, 1, "First frame should produce 1 push");
        let mut next_due = first.next_due_us;
        let mut total_pushes = first.pushes;

        // Simulate polling every 2ms for 1 second, with a frame always available
        // (last_captured_frame exists) but no new real frames.
        let poll_interval_us = 2_000u64;
        let iterations = 1_000_000 / poll_interval_us;
        for i in 1..=iterations {
            let now_us = i * poll_interval_us;
            let result =
                compute_pacing(now_us, next_due, true, MAX_BURST, FRAME_INTERVAL_US);
            total_pushes += result.pushes;
            next_due = result.next_due_us;
        }

        // Should be ~30 pushes (1 real + ~29 duplicates)
        assert!(
            total_pushes >= 28 && total_pushes <= 32,
            "Expected ~30 pushes over 1s of static content, got {total_pushes}",
        );
    }

    #[test]
    fn rate_limiter_resets_on_drift() {
        // Start normally, then jump 2 seconds forward (simulating system pause).
        let first = compute_pacing(0, None, true, MAX_BURST, FRAME_INTERVAL_US);
        let next_due = first.next_due_us;

        // Jump 2 seconds into the future — should trigger drift reset, not
        // produce a burst of 60 catch-up frames.
        let result = compute_pacing(
            2_000_000,
            next_due,
            true,
            MAX_BURST,
            FRAME_INTERVAL_US,
        );
        assert!(result.was_reset, "Should detect >1s drift and reset");
        assert!(
            result.pushes <= MAX_BURST,
            "After drift reset, pushes ({}) should be <= max_burst ({})",
            result.pushes,
            MAX_BURST,
        );
    }

    #[test]
    fn rate_limiter_no_pushes_without_frame() {
        // No frame available — should never push regardless of schedule.
        let result = compute_pacing(100_000, Some(0), false, MAX_BURST, FRAME_INTERVAL_US);
        assert_eq!(result.pushes, 0);
        // Schedule should be preserved for when a frame becomes available
        assert_eq!(result.next_due_us, Some(0));
    }

    #[test]
    fn rate_limiter_initializes_on_first_frame() {
        // First call with no schedule should initialize and push 1 frame.
        let result = compute_pacing(50_000, None, true, MAX_BURST, FRAME_INTERVAL_US);
        assert_eq!(result.pushes, 1);
        assert_eq!(result.next_due_us, Some(50_000 + FRAME_INTERVAL_US));
        assert!(!result.was_reset);
    }

    #[test]
    fn rate_limiter_no_init_without_frame() {
        // No frame and no schedule — should not initialize.
        let result = compute_pacing(50_000, None, false, MAX_BURST, FRAME_INTERVAL_US);
        assert_eq!(result.pushes, 0);
        assert_eq!(result.next_due_us, None);
    }

    #[test]
    fn rate_limiter_burst_cap_enforced() {
        // Schedule is 500ms behind (15 frames due) but max_burst=6.
        let due_us = 0u64;
        let now_us = 500_000; // 500ms later, within 1s drift threshold
        let result =
            compute_pacing(now_us, Some(due_us), true, MAX_BURST, FRAME_INTERVAL_US);
        assert_eq!(
            result.pushes, MAX_BURST,
            "Should cap at max_burst={MAX_BURST}, not push all 15 due frames",
        );
        assert!(!result.was_reset);
    }
}
