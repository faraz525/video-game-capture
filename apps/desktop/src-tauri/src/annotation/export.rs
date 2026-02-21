use crate::clip::format::read_clip;
use crate::clip::metadata::ClipMetadata;
use log::info;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::frame_actions::{compute_frame_params, index_frame_actions};
use super::quality::score_clip_quality;
use super::types::{ClipAnnotations, DatasetStats, FrameAction, QualityScore};

/// Export format for annotated clips.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum ExportFormat {
    /// MP4 + JSON sidecar files in a flat directory.
    /// Most portable format — works with any ML framework.
    JsonSidecar,
    /// HuggingFace Datasets-compatible directory structure.
    /// Optimized for PyTorch DataLoader integration.
    HuggingfaceDataset,
}

/// Result of an export operation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExportResult {
    /// Path to the exported dataset directory.
    pub output_dir: String,
    /// Number of clips exported.
    pub clips_exported: u32,
    /// Dataset statistics.
    pub stats: DatasetStats,
    /// Export format used.
    pub format: String,
}

/// Export a single clip to JSON sidecar format.
///
/// Produces:
/// ```text
/// output_dir/
/// ├── video.mp4
/// ├── actions.json        # Per-frame action dictionary (GF-Minecraft style)
/// ├── input_events.jsonl   # Raw input events (original)
/// ├── metadata.json       # Extended metadata
/// ├── quality.json        # Quality scores
/// └── annotations.json    # Full annotation data
/// ```
pub fn export_clip_json_sidecar(
    clip_path: &Path,
    output_dir: &Path,
) -> Result<ClipAnnotations, ExportError> {
    let contents = read_clip(clip_path).map_err(ExportError::ClipFormat)?;

    fs::create_dir_all(output_dir).map_err(ExportError::Io)?;

    // Generate annotations
    let (frame_count, fps) = compute_frame_params(
        contents.metadata.duration_secs,
        contents.metadata.fps,
    );
    let first_ts = contents
        .input_events
        .first()
        .map(|e| e.timestamp_us)
        .unwrap_or(0);

    let frame_actions = index_frame_actions(
        &contents.input_events,
        frame_count,
        fps,
        first_ts,
    );
    let quality = score_clip_quality(
        &contents.input_events,
        contents.metadata.duration_secs,
        fps,
        first_ts,
        contents.metadata.game.as_deref(),
    );

    // Write video.mp4 (or video.bin if not encoded)
    let video_filename = if contents.metadata.video_encoded { "video.mp4" } else { "video.bin" };
    fs::write(output_dir.join(video_filename), &contents.video_data)
        .map_err(ExportError::Io)?;

    // Write actions.json — per-frame action dictionary indexed by frame number
    // This matches the GF-Minecraft convention that world model researchers expect
    write_actions_json(&frame_actions, output_dir)?;

    // Write raw input events (original format for researchers who want event-stream)
    write_input_events_jsonl(&contents.input_events, output_dir)?;

    // Write metadata.json with annotation info
    write_metadata_json(&contents.metadata, &frame_actions, &quality, output_dir)?;

    // Write quality.json
    let quality_json = serde_json::to_string_pretty(&quality)
        .map_err(ExportError::Json)?;
    fs::write(output_dir.join("quality.json"), quality_json)
        .map_err(ExportError::Io)?;

    // Write audio if present
    if !contents.audio_data.is_empty() {
        fs::write(output_dir.join("audio.bin"), &contents.audio_data)
            .map_err(ExportError::Io)?;
    }

    // Write thumbnail if present
    if !contents.thumbnail.is_empty() {
        fs::write(output_dir.join("thumbnail.jpg"), &contents.thumbnail)
            .map_err(ExportError::Io)?;
    }

    let annotations = ClipAnnotations {
        manifest: super::types::AnnotationManifest {
            layers: vec!["frame_actions".to_string(), "quality".to_string()],
            pipeline_version: env!("CARGO_PKG_VERSION").to_string(),
            annotated_at: chrono::Utc::now().to_rfc3339(),
        },
        frame_actions: Some(frame_actions),
        quality: Some(quality),
    };

    // Write full annotations
    let annotations_json = serde_json::to_string_pretty(&annotations)
        .map_err(ExportError::Json)?;
    fs::write(output_dir.join("annotations.json"), annotations_json)
        .map_err(ExportError::Io)?;

    info!("Exported clip to {}", output_dir.display());
    Ok(annotations)
}

/// Export multiple clips to a HuggingFace Datasets-compatible structure.
///
/// Produces:
/// ```text
/// output_dir/
/// ├── metadata.csv
/// ├── data/
/// │   ├── clip_001/
/// │   │   ├── video.mp4
/// │   │   ├── actions.jsonl
/// │   │   └── metadata.json
/// │   ├── clip_002/
/// │   └── ...
/// └── README.md
/// ```
pub fn export_dataset_huggingface(
    clip_paths: &[PathBuf],
    output_dir: &Path,
) -> Result<ExportResult, ExportError> {
    let data_dir = output_dir.join("data");
    fs::create_dir_all(&data_dir).map_err(ExportError::Io)?;

    let mut csv_rows: Vec<String> = vec![
        "clip_id,game,duration_secs,fps,width,height,input_event_count,quality_score,path".to_string()
    ];

    let mut total_duration = 0.0f64;
    let mut total_frames = 0u64;
    let mut total_events = 0u64;
    let mut games: Vec<String> = Vec::new();
    let mut resolutions: Vec<String> = Vec::new();
    let mut fps_values: Vec<u32> = Vec::new();
    let mut quality_scores: Vec<f64> = Vec::new();
    let mut exported = 0u32;

    for clip_path in clip_paths {
        let clip_dir_name = clip_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        let clip_output_dir = data_dir.join(clip_dir_name);

        match export_clip_json_sidecar(clip_path, &clip_output_dir) {
            Ok(annotations) => {
                let contents = read_clip(clip_path).map_err(ExportError::ClipFormat)?;
                let meta = &contents.metadata;

                let quality_score = annotations
                    .quality
                    .as_ref()
                    .map(|q| q.overall_score)
                    .unwrap_or(0.0);

                let (fc, _) = compute_frame_params(meta.duration_secs, meta.fps);

                csv_rows.push(format!(
                    "{},{},{:.2},{},{},{},{},{:.3},data/{}/",
                    meta.id,
                    meta.game.as_deref().unwrap_or("unknown"),
                    meta.duration_secs,
                    meta.fps,
                    meta.width,
                    meta.height,
                    meta.input_event_count,
                    quality_score,
                    clip_dir_name,
                ));

                total_duration += meta.duration_secs;
                total_frames += fc;
                total_events += meta.input_event_count;
                quality_scores.push(quality_score);

                if let Some(game) = &meta.game {
                    if !games.contains(game) {
                        games.push(game.clone());
                    }
                }

                let res = format!("{}x{}", meta.width, meta.height);
                if !resolutions.contains(&res) {
                    resolutions.push(res);
                }

                if !fps_values.contains(&meta.fps) {
                    fps_values.push(meta.fps);
                }

                exported += 1;
            }
            Err(e) => {
                log::warn!("Skipping clip {}: {e}", clip_path.display());
            }
        }
    }

    // Write metadata.csv
    let csv_content = csv_rows.join("\n") + "\n";
    fs::write(output_dir.join("metadata.csv"), csv_content).map_err(ExportError::Io)?;

    // Write README.md (dataset card)
    write_dataset_readme(output_dir, exported, total_duration, &games)?;

    let avg_quality = if quality_scores.is_empty() {
        0.0
    } else {
        quality_scores.iter().sum::<f64>() / quality_scores.len() as f64
    };

    let stats = DatasetStats {
        total_clips: exported,
        total_duration_secs: total_duration,
        total_frames,
        total_input_events: total_events,
        games,
        avg_quality_score: avg_quality,
        resolutions,
        fps_values,
    };

    // Write dataset stats
    let stats_json = serde_json::to_string_pretty(&stats).map_err(ExportError::Json)?;
    fs::write(output_dir.join("dataset_info.json"), stats_json).map_err(ExportError::Io)?;

    Ok(ExportResult {
        output_dir: output_dir.to_string_lossy().to_string(),
        clips_exported: exported,
        stats,
        format: "huggingface_dataset".to_string(),
    })
}

/// Write per-frame actions as a JSON dictionary indexed by frame number.
/// Matches the GF-Minecraft convention.
fn write_actions_json(actions: &[FrameAction], output_dir: &Path) -> Result<(), ExportError> {
    let mut file = fs::File::create(output_dir.join("actions.jsonl"))
        .map_err(ExportError::Io)?;

    for action in actions {
        let line = serde_json::to_string(action).map_err(ExportError::Json)?;
        file.write_all(line.as_bytes()).map_err(ExportError::Io)?;
        file.write_all(b"\n").map_err(ExportError::Io)?;
    }

    Ok(())
}

/// Write raw input events as JSONL.
fn write_input_events_jsonl(
    events: &[crate::input::InputEvent],
    output_dir: &Path,
) -> Result<(), ExportError> {
    let mut file = fs::File::create(output_dir.join("input_events.jsonl"))
        .map_err(ExportError::Io)?;

    for event in events {
        let line = serde_json::to_string(event).map_err(ExportError::Json)?;
        file.write_all(line.as_bytes()).map_err(ExportError::Io)?;
        file.write_all(b"\n").map_err(ExportError::Io)?;
    }

    Ok(())
}

/// Write extended metadata JSON with annotation summary.
fn write_metadata_json(
    metadata: &ClipMetadata,
    frame_actions: &[FrameAction],
    quality: &QualityScore,
    output_dir: &Path,
) -> Result<(), ExportError> {
    #[derive(serde::Serialize)]
    struct ExtendedMetadata<'a> {
        #[serde(flatten)]
        clip: &'a ClipMetadata,
        annotation_layers: Vec<&'static str>,
        total_frame_actions: usize,
        quality_score: f64,
        action_density: f64,
        highlight_count: usize,
        pipeline_version: &'static str,
    }

    let extended = ExtendedMetadata {
        clip: metadata,
        annotation_layers: vec!["frame_actions", "quality"],
        total_frame_actions: frame_actions.len(),
        quality_score: quality.overall_score,
        action_density: quality.action_density,
        highlight_count: quality.highlights.len(),
        pipeline_version: env!("CARGO_PKG_VERSION"),
    };

    let json = serde_json::to_string_pretty(&extended).map_err(ExportError::Json)?;
    fs::write(output_dir.join("metadata.json"), json).map_err(ExportError::Io)?;

    Ok(())
}

/// Write a HuggingFace dataset README/card.
fn write_dataset_readme(
    output_dir: &Path,
    clip_count: u32,
    total_duration: f64,
    games: &[String],
) -> Result<(), ExportError> {
    let games_str = if games.is_empty() {
        "Unknown".to_string()
    } else {
        games.join(", ")
    };

    let readme = format!(
r#"---
license: cc-by-4.0
task_categories:
  - video-classification
  - reinforcement-learning
tags:
  - world-models
  - action-conditioned
  - gaming
  - gameplay
pretty_name: GameClip Annotated Dataset
---

# GameClip Annotated Dataset

Action-conditioned gameplay video dataset with per-frame input annotations,
suitable for training world models, game-playing agents, and action-conditioned
video prediction models.

## Dataset Summary

- **Clips:** {clip_count}
- **Total Duration:** {total_duration:.1}s
- **Games:** {games_str}
- **Annotation Layers:** frame_actions, quality

## Data Format

Each clip directory contains:

| File | Description |
|------|-------------|
| `video.mp4` | H.264 encoded gameplay video |
| `actions.jsonl` | Per-frame action snapshots (keys held, mouse position/delta, buttons) |
| `input_events.jsonl` | Raw input events with microsecond timestamps |
| `metadata.json` | Clip metadata + annotation summary |
| `quality.json` | Quality/interest scores and highlight segments |
| `annotations.json` | Full annotation data |

## Frame Action Format

Each line in `actions.jsonl` is a JSON object:

```json
{{
  "frame": 0,
  "timestamp_us": 0,
  "keys_held": ["KeyW", "ShiftLeft"],
  "mouse_buttons_held": ["left"],
  "mouse_x": 540.0,
  "mouse_y": 320.0,
  "mouse_dx": 12.5,
  "mouse_dy": -3.2,
  "scroll_dx": 0.0,
  "scroll_dy": 0.0
}}
```

## Usage

```python
import json
from pathlib import Path

dataset_dir = Path("data/clip_001")

# Load per-frame actions
actions = [json.loads(line) for line in (dataset_dir / "actions.jsonl").open()]

# Load metadata
metadata = json.loads((dataset_dir / "metadata.json").read_text())

# Load quality scores
quality = json.loads((dataset_dir / "quality.json").read_text())

print(f"Clip: {{metadata['name']}}, Game: {{metadata.get('game', 'unknown')}}")
print(f"Frames: {{len(actions)}}, Quality: {{quality['overall_score']:.2f}}")
```

## License

CC-BY-4.0

## Generated By

GameClip Annotation Pipeline v{version}
"#,
        clip_count = clip_count,
        total_duration = total_duration,
        games_str = games_str,
        version = env!("CARGO_PKG_VERSION"),
    );

    fs::write(output_dir.join("README.md"), readme).map_err(ExportError::Io)?;
    Ok(())
}

/// Export error type.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("clip format error: {0}")]
    ClipFormat(crate::clip::format::ClipFormatError),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::format::{write_clip, ClipPackageData};
    use crate::clip::metadata::{CaptureDevices, ClipMetadata};
    use crate::input::*;
    use tempfile::TempDir;

    fn make_test_clip(dir: &Path) -> PathBuf {
        let mut input_events = Vec::new();

        // Simulate gameplay
        for i in 0..30u64 {
            let ts = i * 33_333; // ~30fps
            input_events.push(InputEvent {
                timestamp_us: ts,
                kind: InputEventKind::Key(KeyEvent {
                    key: "KeyW".to_string(),
                    pressed: i % 2 == 0,
                }),
            });
            input_events.push(InputEvent {
                timestamp_us: ts + 5000,
                kind: InputEventKind::MouseMove(MouseMoveEvent {
                    x: 100.0 + i as f64 * 5.0,
                    y: 200.0,
                }),
            });
        }

        let metadata = ClipMetadata {
            id: "test_clip_001".to_string(),
            name: "test_clip".to_string(),
            game: Some("TestGame".to_string()),
            width: 640,
            height: 360,
            fps: 30,
            duration_secs: 1.0,
            input_event_count: input_events.len() as u64,
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
            input_events,
            video_data: vec![0xFF; 1024],
            audio_data: vec![],
            thumbnail: vec![],
            frame_actions: vec![],
            quality_score: None,
        };

        let path = dir.join("test.gameclip");
        write_clip(&path, &data).unwrap();
        path
    }

    #[test]
    fn export_json_sidecar_produces_files() {
        let dir = TempDir::new().unwrap();
        let clip_path = make_test_clip(dir.path());
        let export_dir = dir.path().join("export");

        let annotations = export_clip_json_sidecar(&clip_path, &export_dir).unwrap();

        // Verify all expected files exist
        assert!(export_dir.join("video.bin").exists()); // not encoded, so video.bin
        assert!(export_dir.join("actions.jsonl").exists());
        assert!(export_dir.join("input_events.jsonl").exists());
        assert!(export_dir.join("metadata.json").exists());
        assert!(export_dir.join("quality.json").exists());
        assert!(export_dir.join("annotations.json").exists());

        // Verify annotations contain data
        assert!(annotations.frame_actions.is_some());
        assert!(annotations.quality.is_some());
        assert!(annotations.manifest.layers.contains(&"frame_actions".to_string()));
    }

    #[test]
    fn export_actions_jsonl_readable() {
        let dir = TempDir::new().unwrap();
        let clip_path = make_test_clip(dir.path());
        let export_dir = dir.path().join("export");

        export_clip_json_sidecar(&clip_path, &export_dir).unwrap();

        let actions_content = fs::read_to_string(export_dir.join("actions.jsonl")).unwrap();
        let actions: Vec<FrameAction> = actions_content
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();

        assert!(!actions.is_empty());
        assert_eq!(actions[0].frame, 0);
        // Frame numbers should be sequential
        for (i, action) in actions.iter().enumerate() {
            assert_eq!(action.frame, i as u64);
        }
    }

    #[test]
    fn export_quality_json_readable() {
        let dir = TempDir::new().unwrap();
        let clip_path = make_test_clip(dir.path());
        let export_dir = dir.path().join("export");

        export_clip_json_sidecar(&clip_path, &export_dir).unwrap();

        let quality_content = fs::read_to_string(export_dir.join("quality.json")).unwrap();
        let quality: QualityScore = serde_json::from_str(&quality_content).unwrap();

        assert!(quality.overall_score >= 0.0 && quality.overall_score <= 1.0);
        assert!(quality.action_density > 0.0);
    }

    #[test]
    fn export_metadata_has_annotation_info() {
        let dir = TempDir::new().unwrap();
        let clip_path = make_test_clip(dir.path());
        let export_dir = dir.path().join("export");

        export_clip_json_sidecar(&clip_path, &export_dir).unwrap();

        let meta_content = fs::read_to_string(export_dir.join("metadata.json")).unwrap();
        let meta: serde_json::Value = serde_json::from_str(&meta_content).unwrap();

        assert!(meta["annotation_layers"].is_array());
        assert!(meta["total_frame_actions"].as_u64().unwrap() > 0);
        assert!(meta["quality_score"].is_f64());
        assert!(meta["pipeline_version"].is_string());
    }

    #[test]
    fn export_huggingface_dataset() {
        let dir = TempDir::new().unwrap();
        let clip1 = make_test_clip(dir.path());

        let export_dir = dir.path().join("dataset");
        let result = export_dataset_huggingface(&[clip1], &export_dir).unwrap();

        assert_eq!(result.clips_exported, 1);
        assert!(export_dir.join("metadata.csv").exists());
        assert!(export_dir.join("README.md").exists());
        assert!(export_dir.join("dataset_info.json").exists());
        assert!(export_dir.join("data").exists());

        // Check CSV has header + 1 data row
        let csv = fs::read_to_string(export_dir.join("metadata.csv")).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 2); // header + 1 row

        // Check dataset stats
        let stats_content = fs::read_to_string(export_dir.join("dataset_info.json")).unwrap();
        let stats: DatasetStats = serde_json::from_str(&stats_content).unwrap();
        assert_eq!(stats.total_clips, 1);
        assert!(stats.total_frames > 0);
    }
}
