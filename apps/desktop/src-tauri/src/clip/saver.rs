use super::format::{write_clip, ClipPackageData};
use super::metadata::{CaptureDevices, ClipMetadata};
use crate::annotation;
use crate::audio::AudioBuffer;
use crate::capture::CapturedFrame;
use crate::input::InputEvent;
use crate::sync::encoded_ring_buffer::{EncodedChunk, EncodedRingBuffer};
use crate::sync::ring_buffer::{RingBuffer, Timestamped};
use log::{info, warn};
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
    pub encoded_chunks: Option<EncodedRingBuffer>,
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
            encoded_chunks: None,
            save_dir,
        }
    }

    /// Enable the encoded ring buffer for streaming encoding.
    pub fn enable_encoded_buffer(&mut self, buffer_duration_secs: u32) {
        let duration_us = buffer_duration_secs as u64 * 1_000_000;
        self.encoded_chunks = Some(EncodedRingBuffer::new(duration_us));
    }

    /// Push an encoded chunk into the encoded ring buffer.
    pub fn push_encoded_chunk(&mut self, chunk: EncodedChunk) {
        if let Some(ref mut buf) = self.encoded_chunks {
            buf.push(chunk);
        }
    }

    /// Cache the first raw frame for thumbnail generation (used with streaming encoding).
    pub fn cache_first_raw_frame(&mut self, frame: CapturedFrame) {
        if let Some(ref mut buf) = self.encoded_chunks {
            buf.cache_first_frame(frame);
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

        // Drain encoded video data if available
        let encoded_video = self.encoded_chunks.as_mut().map(|buf| {
            let video_data = buf.drain_as_fmp4();
            let first_frame = buf.take_first_frame();
            (video_data, first_frame)
        });

        Self::save_clip_from_data(frames, input_events, audio_buffers, encoded_video, game_name, &save_dir)
    }

    /// Package pre-drained data into a .gameclip file.
    ///
    /// This static method performs all heavy work (thumbnail, encoding, zip)
    /// without holding the saver mutex, minimizing lock contention with the
    /// capture thread.
    ///
    /// `encoded_video`: Optional pre-encoded fMP4 data + first raw frame from
    /// the streaming encoder. When present, skips the FFmpeg batch encoding step.
    pub fn save_clip_from_data(
        frames: Vec<CapturedFrame>,
        input_events: Vec<InputEvent>,
        audio_buffers: Vec<AudioBuffer>,
        encoded_video: Option<(Vec<u8>, Option<CapturedFrame>)>,
        game_name: Option<String>,
        save_dir: &std::path::Path,
    ) -> Result<PathBuf, SaveError> {
        // We need either raw frames or encoded video data
        let has_encoded = encoded_video
            .as_ref()
            .map(|(data, _)| !data.is_empty())
            .unwrap_or(false);

        if frames.is_empty() && !has_encoded {
            return Err(SaveError::NoFrames);
        }

        // Determine frame metadata from raw frames or encoded first frame
        let (ref_frame_width, ref_frame_height, first_ts, last_ts, frame_count) =
            if !frames.is_empty() {
                let first = &frames[0];
                let last = &frames[frames.len() - 1];
                (first.width, first.height, first.timestamp_us, last.timestamp_us, frames.len())
            } else if let Some((_, Some(ref first_frame))) = &encoded_video {
                // Use the cached first frame for dimensions
                (first_frame.width, first_frame.height, first_frame.timestamp_us, first_frame.timestamp_us, 0)
            } else {
                // Encoded data but no first frame cached — use defaults
                (640, 360, 0, 0, 0)
            };

        let duration_us = last_ts.saturating_sub(first_ts);
        let duration_secs = if has_encoded && frame_count == 0 {
            // For streaming-encoded clips, estimate from input events
            if let (Some(first_evt), Some(last_evt)) = (input_events.first(), input_events.last()) {
                (last_evt.timestamp_us.saturating_sub(first_evt.timestamp_us)) as f64 / 1_000_000.0
            } else {
                0.0
            }
        } else {
            duration_us as f64 / 1_000_000.0
        };

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

        let fps_estimate = if duration_secs > 0.0 && frame_count > 0 {
            (frame_count as f64 / duration_secs).round() as u32
        } else {
            60
        };

        let mut metadata = ClipMetadata {
            id: clip_id,
            name: clip_name.clone(),
            game: game_name,
            width: ref_frame_width,
            height: ref_frame_height,
            fps: fps_estimate,
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
            video_encoded: true,
            video_start_timestamp_us: first_ts,
            annotation_layers: Vec::new(),
            format_version: 2,
            checksums: std::collections::HashMap::new(),
        };

        // Select video data source: pre-encoded fMP4 or batch encode from raw frames
        let (video_data, video_encoded, thumbnail_frame) = if let Some((enc_data, first_frame)) = encoded_video {
            if !enc_data.is_empty() {
                info!(
                    "Using pre-encoded streaming video: {} bytes",
                    enc_data.len()
                );
                (enc_data, true, first_frame)
            } else {
                // Empty encoded data — fall back to raw encoding
                info!(
                    "Encoded buffer empty, falling back to batch encoding: {} frames, {:.1}s, {}x{}",
                    frames.len(), duration_secs, ref_frame_width, ref_frame_height
                );
                let (data, encoded) = encode_video(&frames, metadata.fps);
                (data, encoded, None)
            }
        } else {
            info!(
                "Batch encoding clip: {} frames, {:.1}s, {}x{}",
                frames.len(), duration_secs, ref_frame_width, ref_frame_height
            );
            let (data, encoded) = encode_video(&frames, metadata.fps);
            (data, encoded, None)
        };
        metadata.video_encoded = video_encoded;

        // Concatenate audio buffers as raw PCM bytes
        let audio_data: Vec<u8> = audio_buffers
            .iter()
            .flat_map(|b| {
                b.samples
                    .iter()
                    .flat_map(|s| s.to_le_bytes())
            })
            .collect();

        // Generate thumbnail from cached first frame or raw frames
        let thumbnail = if let Some(ref thumb_frame) = thumbnail_frame {
            generate_thumbnail(thumb_frame)
        } else if !frames.is_empty() {
            generate_thumbnail(&frames[0])
        } else {
            Vec::new()
        };

        // Auto-annotate: generate per-frame action labels and quality scores
        let annotations = annotation::annotate_from_events(
            &input_events,
            duration_secs,
            metadata.fps,
            metadata.game.as_deref(),
        );
        let frame_actions = annotations.frame_actions.unwrap_or_default();
        let quality_score = annotations.quality;

        // Update metadata with annotation layers
        if !frame_actions.is_empty() {
            metadata.annotation_layers.push("frame_actions".to_string());
        }
        if quality_score.is_some() {
            metadata.annotation_layers.push("quality".to_string());
        }

        info!(
            "Auto-annotated: {} frame actions, quality={:.2}",
            frame_actions.len(),
            quality_score.as_ref().map(|q| q.overall_score).unwrap_or(0.0),
        );

        let package = ClipPackageData {
            metadata,
            input_events,
            video_data,
            audio_data,
            thumbnail,
            frame_actions,
            quality_score,
        };

        let filename = format!("{clip_name}.gameclip");
        let clip_path = save_dir.join(filename);

        std::fs::create_dir_all(save_dir)
            .map_err(|e| SaveError::Format(super::format::ClipFormatError::Io(e)))?;

        write_clip(&clip_path, &package)?;

        let file_size = std::fs::metadata(&clip_path)
            .map(|m| m.len())
            .unwrap_or(0);
        info!(
            "Clip written: {} ({:.1} KB)",
            clip_path.display(),
            file_size as f64 / 1024.0
        );

        Ok(clip_path)
    }
}

/// Encode video frames to H.264 MP4. Returns (data, is_encoded).
///
/// Tries FFmpeg first. On failure, falls back to raw RGBA concatenation
/// and returns `false` for the encoded flag so the frontend knows
/// the video needs re-encoding before playback.
fn encode_video(frames: &[CapturedFrame], fps: u32) -> (Vec<u8>, bool) {
    match super::encoder::encode_frames_to_mp4(frames, fps) {
        Ok(mp4_data) => return (mp4_data, true),
        Err(e) => {
            warn!("FFmpeg encoding failed, falling back to raw RGBA: {e}");
        }
    }

    // Fallback: raw RGBA concatenation
    let raw = frames.iter().flat_map(|f| f.data.iter().copied()).collect();
    (raw, false)
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
    use crate::sync::encoded_ring_buffer::{ChunkType, EncodedChunk};
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
    fn save_clip_at_640x360_produces_encoded_video() {
        let dir = TempDir::new().unwrap();
        let mut saver = ClipSaver::new(30, dir.path().to_path_buf());

        let frame_interval = 33_333; // ~30fps
        for i in 0..30 {
            let ts = i * frame_interval;
            saver.push_frame(CapturedFrame {
                timestamp_us: ts,
                width: 640,
                height: 360,
                data: vec![255, 0, 0, 255].repeat(640 * 360), // solid red
            });
            saver.push_input(make_key_event(ts, "KeyW", i % 2 == 0));
        }

        let clip_path = saver.save_clip(Some("TestGame".to_string())).unwrap();
        let contents = read_clip(&clip_path).unwrap();

        assert_eq!(contents.metadata.width, 640);
        assert_eq!(contents.metadata.height, 360);
        // If FFmpeg is available, video should be encoded as MP4
        // (starts with ftyp box). If not, it's raw RGBA.
        if contents.metadata.video_encoded {
            assert!(
                contents.video_data.len() >= 8 && &contents.video_data[4..8] == b"ftyp",
                "encoded video should be valid MP4"
            );
        }
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

    // T11: save with encoded chunks sets video_encoded=true
    #[test]
    fn save_with_encoded_chunks_sets_video_encoded() {
        let dir = TempDir::new().unwrap();

        // Build fake fMP4 data (ftyp + moov + moof + mdat)
        let mut fmp4_data = Vec::new();
        // ftyp box
        fmp4_data.extend_from_slice(&[0, 0, 0, 12]);
        fmp4_data.extend_from_slice(b"ftyp");
        fmp4_data.extend_from_slice(&[0, 0, 0, 0]);
        // moov box
        fmp4_data.extend_from_slice(&[0, 0, 0, 12]);
        fmp4_data.extend_from_slice(b"moov");
        fmp4_data.extend_from_slice(&[0, 0, 0, 0]);
        // moof box
        fmp4_data.extend_from_slice(&[0, 0, 0, 12]);
        fmp4_data.extend_from_slice(b"moof");
        fmp4_data.extend_from_slice(&[0, 0, 0, 0]);
        // mdat box
        fmp4_data.extend_from_slice(&[0, 0, 0, 12]);
        fmp4_data.extend_from_slice(b"mdat");
        fmp4_data.extend_from_slice(&[1, 2, 3, 4]);

        let first_frame = Some(make_frame(0));
        let input_events = vec![make_key_event(0, "KeyW", true), make_key_event(100_000, "KeyW", false)];

        let clip_path = ClipSaver::save_clip_from_data(
            vec![], // no raw frames
            input_events,
            vec![],
            Some((fmp4_data, first_frame)),
            Some("TestGame".to_string()),
            dir.path(),
        )
        .unwrap();

        let contents = read_clip(&clip_path).unwrap();
        assert!(contents.metadata.video_encoded, "should be marked as encoded");
        // Verify the video data starts with ftyp
        assert!(
            contents.video_data.len() >= 8 && &contents.video_data[4..8] == b"ftyp",
            "video data should be fMP4"
        );
    }

    // T12: save without encoded chunks falls back to raw RGBA
    #[test]
    fn save_without_encoded_chunks_falls_back_to_raw() {
        let dir = TempDir::new().unwrap();

        let frames = vec![make_frame(0), make_frame(16_667)];
        let input_events = vec![make_key_event(0, "KeyW", true)];

        let clip_path = ClipSaver::save_clip_from_data(
            frames,
            input_events,
            vec![],
            None, // no encoded video
            None,
            dir.path(),
        )
        .unwrap();

        let contents = read_clip(&clip_path).unwrap();
        // Should still produce a valid clip (either encoded via FFmpeg or raw)
        assert!(!contents.video_data.is_empty());
    }
}
