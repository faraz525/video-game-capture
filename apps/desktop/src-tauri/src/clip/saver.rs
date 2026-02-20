use super::format::{write_clip, ClipPackageData};
use super::metadata::{CaptureDevices, ClipMetadata};
use crate::audio::AudioBuffer;
use crate::capture::CapturedFrame;
use crate::input::InputEvent;
use crate::sync::ring_buffer::{RingBuffer, Timestamped};
use std::path::PathBuf;

impl Timestamped for CapturedFrame {
    fn timestamp_us(&self) -> u64 {
        self.timestamp_us
    }
}

impl Timestamped for InputEvent {
    fn timestamp_us(&self) -> u64 {
        self.timestamp_us
    }
}

impl Timestamped for AudioBuffer {
    fn timestamp_us(&self) -> u64 {
        self.timestamp_us
    }
}

/// Error type for clip save operations.
#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("no frames in buffer")]
    NoFrames,
    #[error("format error: {0}")]
    Format(#[from] super::format::ClipFormatError),
}

/// Manages ring buffers for all capture streams and saves clips on demand.
pub struct ClipSaver {
    pub frames: RingBuffer<CapturedFrame>,
    pub input_events: RingBuffer<InputEvent>,
    pub audio_buffers: RingBuffer<AudioBuffer>,
    save_dir: PathBuf,
}

impl ClipSaver {
    /// Create a new ClipSaver with the given buffer duration (in seconds)
    /// and save directory.
    pub fn new(buffer_duration_secs: u32, save_dir: PathBuf) -> Self {
        let duration_us = buffer_duration_secs as u64 * 1_000_000;
        Self {
            frames: RingBuffer::new(duration_us),
            input_events: RingBuffer::new(duration_us),
            audio_buffers: RingBuffer::new(duration_us),
            save_dir,
        }
    }

    /// Push a captured frame into the ring buffer.
    pub fn push_frame(&mut self, frame: CapturedFrame) {
        self.frames.push(frame);
    }

    /// Push an input event into the ring buffer.
    pub fn push_input(&mut self, event: InputEvent) {
        self.input_events.push(event);
    }

    /// Push an audio buffer into the ring buffer.
    pub fn push_audio(&mut self, buffer: AudioBuffer) {
        self.audio_buffers.push(buffer);
    }

    /// Returns the save directory path.
    pub fn save_dir(&self) -> &std::path::Path {
        &self.save_dir
    }

    /// Save the current ring buffer contents as a .gameclip file.
    /// Returns the path to the saved clip.
    #[allow(dead_code)]
    pub fn save_clip(&mut self, game_name: Option<String>) -> Result<PathBuf, SaveError> {
        let frames = self.frames.drain();
        let input_events = self.input_events.drain();
        let audio_buffers = self.audio_buffers.drain();
        let save_dir = self.save_dir.clone();

        Self::save_clip_from_data(frames, input_events, audio_buffers, game_name, &save_dir)
    }

    /// Package pre-drained data into a .gameclip file.
    ///
    /// This static method performs all heavy work (thumbnail, encoding, zip)
    /// without holding the saver mutex, minimizing lock contention with the
    /// capture thread.
    pub fn save_clip_from_data(
        frames: Vec<CapturedFrame>,
        input_events: Vec<InputEvent>,
        audio_buffers: Vec<AudioBuffer>,
        game_name: Option<String>,
        save_dir: &std::path::Path,
    ) -> Result<PathBuf, SaveError> {
        if frames.is_empty() {
            return Err(SaveError::NoFrames);
        }

        let first_frame = &frames[0];
        let last_frame = &frames[frames.len() - 1];
        let duration_us = last_frame.timestamp_us - first_frame.timestamp_us;
        let duration_secs = duration_us as f64 / 1_000_000.0;

        let clip_id = uuid::Uuid::new_v4().to_string();
        let clip_name = format!(
            "clip_{}",
            chrono::Utc::now().format("%Y%m%d_%H%M%S")
        );

        let has_audio = !audio_buffers.is_empty();
        let (audio_sample_rate, audio_channels) = if let Some(buf) = audio_buffers.first() {
            (Some(buf.sample_rate), Some(buf.channels))
        } else {
            (None, None)
        };

        let metadata = ClipMetadata {
            id: clip_id,
            name: clip_name.clone(),
            game: game_name,
            width: first_frame.width,
            height: first_frame.height,
            fps: if duration_secs > 0.0 {
                (frames.len() as f64 / duration_secs).round() as u32
            } else {
                60
            },
            duration_secs,
            input_event_count: input_events.len() as u64,
            has_audio,
            audio_sample_rate,
            audio_channels,
            created_at: chrono::Utc::now(),
            devices: CaptureDevices {
                keyboard: input_events.iter().any(|e| {
                    matches!(e.kind, crate::input::InputEventKind::Key(_))
                }),
                mouse: input_events.iter().any(|e| {
                    matches!(
                        e.kind,
                        crate::input::InputEventKind::MouseMove(_)
                            | crate::input::InputEventKind::MouseButton(_)
                            | crate::input::InputEventKind::MouseScroll(_)
                    )
                }),
                controller: false,
            },
        };

        let video_data = encode_video(&frames, metadata.fps);

        // Concatenate audio buffers as raw PCM bytes
        let audio_data: Vec<u8> = audio_buffers
            .iter()
            .flat_map(|b| {
                b.samples
                    .iter()
                    .flat_map(|s| s.to_le_bytes())
            })
            .collect();

        let thumbnail = generate_thumbnail(first_frame);

        let package = ClipPackageData {
            metadata,
            input_events,
            video_data,
            audio_data,
            thumbnail,
        };

        let filename = format!("{clip_name}.gameclip");
        let clip_path = save_dir.join(filename);

        std::fs::create_dir_all(save_dir)
            .map_err(|e| SaveError::Format(super::format::ClipFormatError::Io(e)))?;

        write_clip(&clip_path, &package)?;
        Ok(clip_path)
    }
}

/// Encode video frames. Tries FFmpeg (produces MP4) first on all platforms,
/// falls back to raw RGBA concatenation if FFmpeg is unavailable.
fn encode_video(frames: &[CapturedFrame], fps: u32) -> Vec<u8> {
    match super::encoder::encode_frames_to_mp4(frames, fps) {
        Ok(mp4_data) => return mp4_data,
        Err(e) => {
            eprintln!("[GameClip] FFmpeg encoding failed, falling back to raw: {e}");
        }
    }

    // Fallback: raw RGBA concatenation
    frames.iter().flat_map(|f| f.data.iter().copied()).collect()
}

const THUMBNAIL_WIDTH: u32 = 320;
const THUMBNAIL_JPEG_QUALITY: u8 = 80;

/// Generate a JPEG thumbnail from a captured frame.
///
/// Resizes the frame to `THUMBNAIL_WIDTH` pixels wide (maintaining aspect ratio)
/// and encodes as JPEG.
fn generate_thumbnail(frame: &CapturedFrame) -> Vec<u8> {
    use image::{ImageBuffer, RgbaImage};
    use std::io::Cursor;

    let img: Option<RgbaImage> =
        ImageBuffer::from_raw(frame.width, frame.height, frame.data.clone());

    let Some(img) = img else {
        return Vec::new();
    };

    let aspect = frame.height as f64 / frame.width as f64;
    let thumb_height = (THUMBNAIL_WIDTH as f64 * aspect).round() as u32;

    let resized = image::imageops::resize(
        &img,
        THUMBNAIL_WIDTH,
        thumb_height,
        image::imageops::FilterType::Triangle,
    );

    let rgb_img = image::DynamicImage::ImageRgba8(
        image::ImageBuffer::from_raw(THUMBNAIL_WIDTH, thumb_height, resized.into_raw())
            .unwrap_or_else(|| ImageBuffer::new(THUMBNAIL_WIDTH, thumb_height)),
    )
    .to_rgb8();

    let mut buf = Cursor::new(Vec::new());
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, THUMBNAIL_JPEG_QUALITY);
    if image::ImageEncoder::write_image(
        encoder,
        &rgb_img,
        THUMBNAIL_WIDTH,
        thumb_height,
        image::ExtendedColorType::Rgb8,
    )
    .is_err()
    {
        return Vec::new();
    }

    buf.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::CapturedFrame;
    use crate::clip::format::read_clip;
    use crate::input::{InputEventKind, KeyEvent, MouseMoveEvent};
    use tempfile::TempDir;

    fn make_frame(ts: u64) -> CapturedFrame {
        CapturedFrame {
            timestamp_us: ts,
            width: 4,
            height: 4,
            data: vec![255, 0, 0, 255].repeat(16), // 4x4 red
        }
    }

    fn make_key_event(ts: u64, key: &str, pressed: bool) -> InputEvent {
        InputEvent {
            timestamp_us: ts,
            kind: InputEventKind::Key(KeyEvent {
                key: key.to_string(),
                pressed,
            }),
        }
    }

    fn make_mouse_move(ts: u64) -> InputEvent {
        InputEvent {
            timestamp_us: ts,
            kind: InputEventKind::MouseMove(MouseMoveEvent { x: 100.0, y: 200.0 }),
        }
    }

    fn make_audio_buffer(ts: u64) -> AudioBuffer {
        AudioBuffer {
            timestamp_us: ts,
            channels: 2,
            sample_rate: 48000,
            samples: vec![0.0; 2048],
        }
    }

    #[test]
    fn save_clip_produces_valid_file() {
        let dir = TempDir::new().unwrap();
        let mut saver = ClipSaver::new(30, dir.path().to_path_buf());

        for i in 0..10 {
            saver.push_frame(make_frame(i * 16_667));
            saver.push_input(make_key_event(i * 16_667, "KeyW", i % 2 == 0));
            saver.push_audio(make_audio_buffer(i * 16_667));
        }

        let clip_path = saver.save_clip(Some("TestGame".to_string())).unwrap();
        assert!(clip_path.exists());
        assert!(clip_path.extension().unwrap() == "gameclip");

        let contents = read_clip(&clip_path).unwrap();
        assert_eq!(contents.metadata.game, Some("TestGame".to_string()));
        assert_eq!(contents.metadata.width, 4);
        assert_eq!(contents.metadata.height, 4);
        assert_eq!(contents.input_events.len(), 10);
        assert!(contents.metadata.has_audio);
        assert!(contents.metadata.devices.keyboard);
    }

    #[test]
    fn save_empty_buffer_fails() {
        let dir = TempDir::new().unwrap();
        let mut saver = ClipSaver::new(30, dir.path().to_path_buf());

        let result = saver.save_clip(None);
        assert!(result.is_err());
    }

    #[test]
    fn ring_buffer_evicts_old_data() {
        let dir = TempDir::new().unwrap();
        let mut saver = ClipSaver::new(1, dir.path().to_path_buf()); // 1 second buffer

        // Push 2 seconds of frames at ~60fps
        for i in 0..120 {
            saver.push_frame(make_frame(i * 16_667));
        }

        // Buffer should only contain ~1 second (~60 frames)
        assert!(saver.frames.len() < 70, "expected <70 frames, got {}", saver.frames.len());
        assert!(saver.frames.len() > 50, "expected >50 frames, got {}", saver.frames.len());
    }

    #[test]
    fn save_drains_buffers() {
        let dir = TempDir::new().unwrap();
        let mut saver = ClipSaver::new(30, dir.path().to_path_buf());

        saver.push_frame(make_frame(0));
        saver.push_input(make_key_event(0, "KeyW", true));

        saver.save_clip(None).unwrap();

        assert!(saver.frames.is_empty());
        assert!(saver.input_events.is_empty());
        assert!(saver.audio_buffers.is_empty());
    }

    #[test]
    fn devices_detected_from_events() {
        let dir = TempDir::new().unwrap();
        let mut saver = ClipSaver::new(30, dir.path().to_path_buf());

        saver.push_frame(make_frame(0));
        saver.push_frame(make_frame(16_667));
        saver.push_input(make_key_event(0, "KeyW", true));
        saver.push_input(make_mouse_move(8000));

        let clip_path = saver.save_clip(None).unwrap();
        let contents = read_clip(&clip_path).unwrap();

        assert!(contents.metadata.devices.keyboard);
        assert!(contents.metadata.devices.mouse);
        assert!(!contents.metadata.devices.controller);
    }

    #[test]
    fn thumbnail_is_valid_jpeg() {
        let frame = make_frame(0);
        let thumbnail = generate_thumbnail(&frame);

        // JPEG files start with FF D8 FF
        assert!(thumbnail.len() > 3, "thumbnail should not be empty");
        assert_eq!(
            &thumbnail[0..2],
            &[0xFF, 0xD8],
            "thumbnail should be valid JPEG"
        );
    }

    #[test]
    fn clip_without_audio() {
        let dir = TempDir::new().unwrap();
        let mut saver = ClipSaver::new(30, dir.path().to_path_buf());

        saver.push_frame(make_frame(0));
        saver.push_frame(make_frame(16_667));

        let clip_path = saver.save_clip(None).unwrap();
        let contents = read_clip(&clip_path).unwrap();

        assert!(!contents.metadata.has_audio);
        assert!(contents.audio_data.is_empty());
    }
}
