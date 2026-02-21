use serde::{Deserialize, Serialize};

/// Per-frame action snapshot — the format world model researchers need.
///
/// Each record represents the complete input state at a single video frame,
/// converting the event-stream (press/release timestamps) into a per-frame
/// discrete action vector suitable for training action-conditioned world models.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FrameAction {
    /// Zero-indexed frame number.
    pub frame: u64,
    /// Timestamp in microseconds (matches video frame timing).
    pub timestamp_us: u64,
    /// Keys currently held down at this frame.
    pub keys_held: Vec<String>,
    /// Mouse buttons currently held down at this frame.
    pub mouse_buttons_held: Vec<String>,
    /// Mouse X position (absolute screen coordinates).
    pub mouse_x: f64,
    /// Mouse Y position (absolute screen coordinates).
    pub mouse_y: f64,
    /// Accumulated mouse X delta since previous frame (raw movement).
    pub mouse_dx: f64,
    /// Accumulated mouse Y delta since previous frame (raw movement).
    pub mouse_dy: f64,
    /// Accumulated scroll X delta since previous frame.
    pub scroll_dx: f64,
    /// Accumulated scroll Y delta since previous frame.
    pub scroll_dy: f64,
}

/// Clip-level quality and interest scoring.
///
/// Surfaces the most valuable training examples by measuring action density,
/// input complexity, and detecting highlight moments.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QualityScore {
    /// Overall quality score (0.0 to 1.0).
    pub overall_score: f64,
    /// Average input events per second.
    pub action_density: f64,
    /// Ratio of frames with active key/mouse input (0.0 to 1.0).
    pub input_activity_ratio: f64,
    /// Average number of simultaneous keys held.
    pub avg_simultaneous_keys: f64,
    /// Peak number of simultaneous keys held.
    pub peak_simultaneous_keys: u32,
    /// Average mouse movement speed (pixels/second).
    pub avg_mouse_speed: f64,
    /// Peak mouse movement speed (pixels/second).
    pub peak_mouse_speed: f64,
    /// Number of distinct keys used throughout the clip.
    pub unique_keys_used: u32,
    /// Detected highlight segments with high input intensity.
    pub highlights: Vec<HighlightSegment>,
    /// Flags for edge cases that are valuable for training.
    pub edge_case_flags: Vec<String>,
}

/// A segment of the clip identified as a highlight moment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HighlightSegment {
    /// Start timestamp in microseconds.
    pub start_us: u64,
    /// End timestamp in microseconds.
    pub end_us: u64,
    /// Type of highlight detected.
    pub highlight_type: String,
    /// Confidence score (0.0 to 1.0).
    pub confidence: f64,
}

/// Manifest of which annotation layers are present in a clip.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AnnotationManifest {
    /// Which annotation layers are present.
    pub layers: Vec<String>,
    /// Version of the annotation pipeline that produced these.
    pub pipeline_version: String,
    /// When the annotations were generated.
    pub annotated_at: String,
}

/// Full annotation data for a clip.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClipAnnotations {
    /// Annotation manifest.
    pub manifest: AnnotationManifest,
    /// Per-frame action snapshots (if generated).
    pub frame_actions: Option<Vec<FrameAction>>,
    /// Quality and interest scores (if generated).
    pub quality: Option<QualityScore>,
}

/// Summary statistics for an annotated dataset export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetStats {
    /// Total number of clips.
    pub total_clips: u32,
    /// Total duration in seconds.
    pub total_duration_secs: f64,
    /// Total number of frames with action labels.
    pub total_frames: u64,
    /// Total number of raw input events.
    pub total_input_events: u64,
    /// Games represented in the dataset.
    pub games: Vec<String>,
    /// Average quality score across all clips.
    pub avg_quality_score: f64,
    /// Resolution breakdown.
    pub resolutions: Vec<String>,
    /// FPS breakdown.
    pub fps_values: Vec<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_action_serializes_roundtrip() {
        let action = FrameAction {
            frame: 42,
            timestamp_us: 700_000,
            keys_held: vec!["KeyW".to_string(), "ShiftLeft".to_string()],
            mouse_buttons_held: vec!["left".to_string()],
            mouse_x: 540.0,
            mouse_y: 320.0,
            mouse_dx: 12.5,
            mouse_dy: -3.2,
            scroll_dx: 0.0,
            scroll_dy: 0.0,
        };

        let json = serde_json::to_string(&action).unwrap();
        let deserialized: FrameAction = serde_json::from_str(&json).unwrap();
        assert_eq!(action, deserialized);
    }

    #[test]
    fn quality_score_serializes_roundtrip() {
        let score = QualityScore {
            overall_score: 0.85,
            action_density: 42.5,
            input_activity_ratio: 0.73,
            avg_simultaneous_keys: 2.1,
            peak_simultaneous_keys: 5,
            avg_mouse_speed: 450.0,
            peak_mouse_speed: 2400.0,
            unique_keys_used: 12,
            highlights: vec![HighlightSegment {
                start_us: 5_000_000,
                end_us: 8_000_000,
                highlight_type: "input_burst".to_string(),
                confidence: 0.9,
            }],
            edge_case_flags: vec!["rapid_camera_movement".to_string()],
        };

        let json = serde_json::to_string(&score).unwrap();
        let deserialized: QualityScore = serde_json::from_str(&json).unwrap();
        assert_eq!(score, deserialized);
    }

    #[test]
    fn annotation_manifest_default() {
        let manifest = AnnotationManifest::default();
        assert!(manifest.layers.is_empty());
        assert!(manifest.pipeline_version.is_empty());
    }
}
