use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Metadata for a saved game clip.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClipMetadata {
    /// Unique clip identifier.
    pub id: String,
    /// Display name for the clip.
    pub name: String,
    /// Detected game name (if any).
    pub game: Option<String>,
    /// Capture resolution width.
    pub width: u32,
    /// Capture resolution height.
    pub height: u32,
    /// Target frames per second.
    pub fps: u32,
    /// Clip duration in seconds.
    pub duration_secs: f64,
    /// Number of input events recorded.
    pub input_event_count: u64,
    /// Whether audio was captured.
    pub has_audio: bool,
    /// Audio sample rate (if audio captured).
    pub audio_sample_rate: Option<u32>,
    /// Audio channels (if audio captured).
    pub audio_channels: Option<u16>,
    /// When the clip was created.
    pub created_at: DateTime<Utc>,
    /// Input devices detected during capture.
    pub devices: CaptureDevices,
    /// Whether the video data is encoded as H.264 MP4 (true) or raw RGBA (false).
    /// Defaults to true for backwards compatibility with older clips.
    #[serde(default = "default_video_encoded")]
    pub video_encoded: bool,
}

fn default_video_encoded() -> bool {
    true
}

/// Input devices that were active during capture.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CaptureDevices {
    pub keyboard: bool,
    pub mouse: bool,
    pub controller: bool,
}

impl ClipMetadata {
    #[allow(dead_code)]
    pub fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            game: None,
            width: 1920,
            height: 1080,
            fps: 60,
            duration_secs: 0.0,
            input_event_count: 0,
            has_audio: false,
            audio_sample_rate: None,
            audio_channels: None,
            created_at: Utc::now(),
            devices: CaptureDevices::default(),
            video_encoded: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_serializes_roundtrip() {
        let meta = ClipMetadata {
            id: "clip_001".to_string(),
            name: "Test Clip".to_string(),
            game: Some("TestGame".to_string()),
            width: 1920,
            height: 1080,
            fps: 60,
            duration_secs: 30.0,
            input_event_count: 1500,
            has_audio: true,
            audio_sample_rate: Some(48000),
            audio_channels: Some(2),
            created_at: Utc::now(),
            devices: CaptureDevices {
                keyboard: true,
                mouse: true,
                controller: false,
            },
            video_encoded: true,
        };

        let json = serde_json::to_string_pretty(&meta).unwrap();
        let deserialized: ClipMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(meta, deserialized);
    }

    #[test]
    fn metadata_default_values() {
        let meta = ClipMetadata::new("test".to_string(), "Test".to_string());
        assert_eq!(meta.width, 1920);
        assert_eq!(meta.height, 1080);
        assert_eq!(meta.fps, 60);
        assert_eq!(meta.duration_secs, 0.0);
        assert!(!meta.has_audio);
        assert!(!meta.devices.keyboard);
    }

    #[test]
    fn metadata_json_has_expected_fields() {
        let meta = ClipMetadata::new("clip_001".to_string(), "My Clip".to_string());
        let json = serde_json::to_string(&meta).unwrap();

        assert!(json.contains("\"id\":\"clip_001\""));
        assert!(json.contains("\"name\":\"My Clip\""));
        assert!(json.contains("\"width\":1920"));
        assert!(json.contains("\"created_at\""));
    }
}
