pub mod export;
pub mod frame_actions;
pub mod quality;
pub mod types;

use crate::clip::format::read_clip;
use crate::input::InputEvent;
use log::info;
use std::path::Path;

use frame_actions::{compute_frame_params, index_frame_actions};
use quality::score_clip_quality;
use types::{AnnotationManifest, ClipAnnotations, FrameAction, QualityScore};

/// Run the full annotation pipeline on a clip file.
///
/// Generates:
/// - Per-frame action snapshots (frame_actions)
/// - Quality and interest scores (quality)
///
/// This is the core function that transforms raw .gameclip capture data
/// into ML-ready annotated data suitable for world model training.
pub fn annotate_clip(clip_path: &Path) -> Result<ClipAnnotations, AnnotationError> {
    let contents = read_clip(clip_path).map_err(AnnotationError::ClipFormat)?;

    let annotations = annotate_from_events(
        &contents.input_events,
        contents.metadata.duration_secs,
        contents.metadata.fps,
    );

    info!(
        "Annotated clip {}: {} frame actions, quality={:.2}",
        clip_path.display(),
        annotations.frame_actions.as_ref().map(|a| a.len()).unwrap_or(0),
        annotations.quality.as_ref().map(|q| q.overall_score).unwrap_or(0.0),
    );

    Ok(annotations)
}

/// Run annotation pipeline directly from input events and metadata.
///
/// This avoids reading from disk when the data is already in memory
/// (e.g., during clip save).
pub fn annotate_from_events(
    input_events: &[InputEvent],
    duration_secs: f64,
    fps: u32,
) -> ClipAnnotations {
    let first_ts = input_events.first().map(|e| e.timestamp_us).unwrap_or(0);
    let (frame_count, fps) = compute_frame_params(duration_secs, fps);

    let frame_actions = index_frame_actions(input_events, frame_count, fps, first_ts);
    let quality = score_clip_quality(input_events, duration_secs, fps, first_ts);

    ClipAnnotations {
        manifest: AnnotationManifest {
            layers: vec!["frame_actions".to_string(), "quality".to_string()],
            pipeline_version: env!("CARGO_PKG_VERSION").to_string(),
            annotated_at: chrono::Utc::now().to_rfc3339(),
        },
        frame_actions: Some(frame_actions),
        quality: Some(quality),
    }
}

/// Convenience: get just the frame actions for a clip.
pub fn get_frame_actions(clip_path: &Path) -> Result<Vec<FrameAction>, AnnotationError> {
    let contents = read_clip(clip_path).map_err(AnnotationError::ClipFormat)?;
    let first_ts = contents.input_events.first().map(|e| e.timestamp_us).unwrap_or(0);
    let (frame_count, fps) = compute_frame_params(
        contents.metadata.duration_secs,
        contents.metadata.fps,
    );

    Ok(index_frame_actions(&contents.input_events, frame_count, fps, first_ts))
}

/// Convenience: get just the quality score for a clip.
pub fn get_quality_score(clip_path: &Path) -> Result<QualityScore, AnnotationError> {
    let contents = read_clip(clip_path).map_err(AnnotationError::ClipFormat)?;
    let first_ts = contents.input_events.first().map(|e| e.timestamp_us).unwrap_or(0);

    Ok(score_clip_quality(
        &contents.input_events,
        contents.metadata.duration_secs,
        contents.metadata.fps,
        first_ts,
    ))
}

/// Annotation pipeline error type.
#[derive(Debug, thiserror::Error)]
pub enum AnnotationError {
    #[error("clip format error: {0}")]
    ClipFormat(crate::clip::format::ClipFormatError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::format::{write_clip, ClipPackageData};
    use crate::clip::metadata::{CaptureDevices, ClipMetadata};
    use crate::input::*;
    use tempfile::TempDir;

    fn make_test_clip(dir: &std::path::Path) -> std::path::PathBuf {
        let mut events = Vec::new();
        for i in 0..60u64 {
            let ts = i * 16_667; // ~60fps
            events.push(InputEvent {
                timestamp_us: ts,
                kind: InputEventKind::Key(KeyEvent {
                    key: if i % 3 == 0 { "KeyW" } else if i % 3 == 1 { "KeyA" } else { "Space" }.to_string(),
                    pressed: i % 2 == 0,
                }),
            });
            events.push(InputEvent {
                timestamp_us: ts + 5000,
                kind: InputEventKind::MouseMove(MouseMoveEvent {
                    x: 100.0 + i as f64 * 3.0,
                    y: 200.0 + (i as f64 * 0.5).sin() * 50.0,
                }),
            });
        }

        let metadata = ClipMetadata {
            id: "test_annotate_001".to_string(),
            name: "test_annotate".to_string(),
            game: Some("Counter-Strike 2".to_string()),
            width: 1920,
            height: 1080,
            fps: 60,
            duration_secs: 1.0,
            input_event_count: events.len() as u64,
            has_audio: false,
            audio_sample_rate: None,
            audio_channels: None,
            created_at: chrono::Utc::now(),
            devices: CaptureDevices {
                keyboard: true,
                mouse: true,
                controller: false,
            },
            video_encoded: false,
            annotation_layers: Vec::new(),
        };

        let data = ClipPackageData {
            metadata,
            input_events: events,
            video_data: vec![0u8; 256],
            audio_data: vec![],
            thumbnail: vec![],
            frame_actions: vec![],
            quality_score: None,
        };

        let path = dir.join("test_annotate.gameclip");
        write_clip(&path, &data).unwrap();
        path
    }

    #[test]
    fn annotate_clip_produces_all_layers() {
        let dir = TempDir::new().unwrap();
        let clip_path = make_test_clip(dir.path());

        let annotations = annotate_clip(&clip_path).unwrap();

        assert!(annotations.frame_actions.is_some());
        assert!(annotations.quality.is_some());
        assert!(annotations.manifest.layers.contains(&"frame_actions".to_string()));
        assert!(annotations.manifest.layers.contains(&"quality".to_string()));
        assert!(!annotations.manifest.pipeline_version.is_empty());
    }

    #[test]
    fn frame_actions_match_expected_count() {
        let dir = TempDir::new().unwrap();
        let clip_path = make_test_clip(dir.path());

        let actions = get_frame_actions(&clip_path).unwrap();

        // 1 second at 60fps = 60 frames
        assert_eq!(actions.len(), 60);

        // Frame numbers sequential
        for (i, action) in actions.iter().enumerate() {
            assert_eq!(action.frame, i as u64);
        }
    }

    #[test]
    fn quality_score_reasonable() {
        let dir = TempDir::new().unwrap();
        let clip_path = make_test_clip(dir.path());

        let quality = get_quality_score(&clip_path).unwrap();

        assert!(quality.overall_score > 0.0, "should have nonzero quality score");
        assert!(quality.overall_score <= 1.0, "score should be <= 1.0");
        assert!(quality.action_density > 0.0, "should have nonzero action density");
        assert!(quality.unique_keys_used >= 3, "should detect multiple unique keys");
    }

    #[test]
    fn annotate_from_events_no_disk() {
        let events = vec![
            InputEvent {
                timestamp_us: 0,
                kind: InputEventKind::Key(KeyEvent {
                    key: "KeyW".to_string(),
                    pressed: true,
                }),
            },
            InputEvent {
                timestamp_us: 500_000,
                kind: InputEventKind::Key(KeyEvent {
                    key: "KeyW".to_string(),
                    pressed: false,
                }),
            },
        ];

        let annotations = annotate_from_events(&events, 1.0, 30);

        assert!(annotations.frame_actions.is_some());
        let actions = annotations.frame_actions.unwrap();
        assert_eq!(actions.len(), 30); // 1 second at 30fps

        // First frame should have W held
        assert!(actions[0].keys_held.contains(&"KeyW".to_string()));
    }

    #[test]
    fn nonexistent_clip_returns_error() {
        let result = annotate_clip(Path::new("/tmp/nonexistent.gameclip"));
        assert!(result.is_err());
    }
}
