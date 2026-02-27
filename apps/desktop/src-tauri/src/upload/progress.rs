use serde::{Deserialize, Serialize};

/// Stages of the upload process.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum UploadStage {
    /// Preparing clip data (scrubbing, quality check)
    Preparing,
    /// Uploading video data
    UploadingVideo,
    /// Uploading metadata and annotations
    UploadingMetadata,
    /// Committing to repository
    Committing,
    /// Upload completed successfully
    Done,
    /// Upload failed
    Failed { reason: String },
}

/// Progress information for a clip upload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadProgress {
    /// Index of the current clip (0-based)
    pub current_clip: u32,
    /// Total number of clips to upload
    pub total_clips: u32,
    /// Name of the current clip
    pub clip_name: String,
    /// Current stage
    pub stage: UploadStage,
    /// Bytes uploaded so far for the current clip
    pub bytes_uploaded: u64,
    /// Total bytes for the current clip
    pub total_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // T38: UploadProgress serializes to expected JSON shape
    #[test]
    fn upload_progress_serializes() {
        let progress = UploadProgress {
            current_clip: 0,
            total_clips: 3,
            clip_name: "test_clip".to_string(),
            stage: UploadStage::UploadingVideo,
            bytes_uploaded: 1024,
            total_bytes: 4096,
        };

        let json = serde_json::to_string(&progress).unwrap();
        assert!(json.contains("\"current_clip\":0"));
        assert!(json.contains("\"total_clips\":3"));
        assert!(json.contains("\"UploadingVideo\""));
        assert!(json.contains("\"bytes_uploaded\":1024"));
    }

    // T39: UploadStage::Failed contains reason
    #[test]
    fn upload_stage_failed_contains_reason() {
        let stage = UploadStage::Failed {
            reason: "network timeout".to_string(),
        };

        let json = serde_json::to_string(&stage).unwrap();
        assert!(json.contains("\"reason\":\"network timeout\""));
        assert!(json.contains("\"Failed\""));
    }

    #[test]
    fn upload_progress_roundtrip() {
        let progress = UploadProgress {
            current_clip: 2,
            total_clips: 5,
            clip_name: "clip_001".to_string(),
            stage: UploadStage::Done,
            bytes_uploaded: 0,
            total_bytes: 0,
        };

        let json = serde_json::to_string(&progress).unwrap();
        let deserialized: UploadProgress = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.current_clip, 2);
        assert_eq!(deserialized.total_clips, 5);
        assert_eq!(deserialized.stage, UploadStage::Done);
    }
}
