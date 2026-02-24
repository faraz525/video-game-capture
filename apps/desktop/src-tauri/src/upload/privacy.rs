use crate::clip::metadata::ClipMetadata;

/// Options for privacy scrubbing.
#[derive(Debug, Clone, Default)]
pub struct ScrubOptions {
    /// Clear audio-related fields from metadata.
    pub strip_audio: bool,
}

/// Scrub PII from clip metadata before upload.
///
/// Returns a new ClipMetadata with sensitive information redacted.
/// The original metadata is never modified (immutability).
///
/// Redactions:
/// - Clip name containing path separators → replaced with "clip_{id}"
/// - Audio fields cleared if strip_audio is true
pub fn scrub_metadata(metadata: &ClipMetadata, options: &ScrubOptions) -> ClipMetadata {
    let name = if contains_path_separator(&metadata.name) {
        format!("clip_{}", metadata.id)
    } else {
        metadata.name.clone()
    };

    let game = metadata.game.as_ref().map(|g| {
        if contains_path_separator(g) {
            "Unknown".to_string()
        } else {
            g.clone()
        }
    });

    let (has_audio, audio_sample_rate, audio_channels) = if options.strip_audio {
        (false, None, None)
    } else {
        (metadata.has_audio, metadata.audio_sample_rate, metadata.audio_channels)
    };

    ClipMetadata {
        id: metadata.id.clone(),
        name,
        game,
        width: metadata.width,
        height: metadata.height,
        fps: metadata.fps,
        duration_secs: metadata.duration_secs,
        input_event_count: metadata.input_event_count,
        has_audio,
        audio_sample_rate,
        audio_channels,
        created_at: metadata.created_at,
        devices: metadata.devices.clone(),
        video_encoded: metadata.video_encoded,
        video_start_timestamp_us: metadata.video_start_timestamp_us,
        annotation_layers: metadata.annotation_layers.clone(),
        format_version: metadata.format_version,
        checksums: metadata.checksums.clone(),
    }
}

fn contains_path_separator(s: &str) -> bool {
    s.contains('/') || s.contains('\\')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::metadata::CaptureDevices;
    use std::collections::HashMap;

    fn make_metadata() -> ClipMetadata {
        ClipMetadata {
            id: "test_123".to_string(),
            name: "my_clip".to_string(),
            game: Some("TestGame".to_string()),
            width: 1920,
            height: 1080,
            fps: 60,
            duration_secs: 30.0,
            input_event_count: 1500,
            has_audio: true,
            audio_sample_rate: Some(48000),
            audio_channels: Some(2),
            created_at: chrono::Utc::now(),
            devices: CaptureDevices {
                keyboard: true,
                mouse: true,
                controller: false,
            },
            video_encoded: true,
            video_start_timestamp_us: 0,
            annotation_layers: vec!["frame_actions".to_string()],
            format_version: 2,
            checksums: HashMap::new(),
        }
    }

    // T30: scrub_metadata returns new object, original unchanged
    #[test]
    fn scrub_returns_new_object_original_unchanged() {
        let original = make_metadata();
        let original_name = original.name.clone();
        let scrubbed = scrub_metadata(&original, &ScrubOptions::default());

        // Original unchanged
        assert_eq!(original.name, original_name);
        // Scrubbed is a separate object
        assert_eq!(scrubbed.id, original.id);
    }

    // T31: clip name with path separator is redacted
    #[test]
    fn clip_name_with_path_separator_is_redacted() {
        let mut meta = make_metadata();
        meta.name = "/Users/faraz525/clips/my_clip".to_string();

        let scrubbed = scrub_metadata(&meta, &ScrubOptions::default());
        assert_eq!(scrubbed.name, "clip_test_123");
        assert!(!scrubbed.name.contains('/'));
    }

    // T32: clean game name passes through unchanged
    #[test]
    fn clean_game_name_passes_through() {
        let meta = make_metadata();
        let scrubbed = scrub_metadata(&meta, &ScrubOptions::default());
        assert_eq!(scrubbed.game, Some("TestGame".to_string()));
    }

    // T33: strip_audio option clears audio fields
    #[test]
    fn strip_audio_clears_audio_fields() {
        let meta = make_metadata();
        assert!(meta.has_audio);

        let scrubbed = scrub_metadata(&meta, &ScrubOptions { strip_audio: true });
        assert!(!scrubbed.has_audio);
        assert!(scrubbed.audio_sample_rate.is_none());
        assert!(scrubbed.audio_channels.is_none());
    }

    // Additional: backslash path separator also redacted
    #[test]
    fn backslash_path_separator_redacted() {
        let mut meta = make_metadata();
        meta.name = "C:\\Users\\faraz\\clip".to_string();

        let scrubbed = scrub_metadata(&meta, &ScrubOptions::default());
        assert_eq!(scrubbed.name, "clip_test_123");
    }

    // Additional: game name with path separator is redacted
    #[test]
    fn game_name_with_path_redacted() {
        let mut meta = make_metadata();
        meta.game = Some("/usr/games/TestGame".to_string());

        let scrubbed = scrub_metadata(&meta, &ScrubOptions::default());
        assert_eq!(scrubbed.game, Some("Unknown".to_string()));
    }
}
