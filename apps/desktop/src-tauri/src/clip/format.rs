use super::metadata::ClipMetadata;
use crate::annotation::types::{FrameAction, QualityScore};
use crate::input::InputEvent;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use zip::write::FileOptions;
use zip::ZipWriter;

/// Error type for clip format operations.
#[derive(Debug, thiserror::Error)]
pub enum ClipFormatError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("missing required file: {0}")]
    MissingFile(String),
    #[error("checksum mismatch for {file}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        file: String,
        expected: String,
        actual: String,
    },
}

/// Data needed to package a .gameclip file.
pub struct ClipPackageData {
    pub metadata: ClipMetadata,
    pub input_events: Vec<InputEvent>,
    /// Raw video bytes (MP4/H.264). For mock, this is just raw RGBA frame data.
    pub video_data: Vec<u8>,
    /// Raw audio bytes. For mock, this is raw PCM f32 data.
    pub audio_data: Vec<u8>,
    /// JPEG thumbnail bytes.
    pub thumbnail: Vec<u8>,
    /// Per-frame action annotations (optional).
    pub frame_actions: Vec<FrameAction>,
    /// Quality score annotations (optional).
    pub quality_score: Option<QualityScore>,
}

/// Options for writing a .gameclip file.
#[derive(Debug, Clone)]
pub struct WriteOptions {
    /// Compute SHA-256 checksums for all entries (produces v2 format).
    pub compute_checksums: bool,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            compute_checksums: true,
        }
    }
}

/// Options for reading a .gameclip file.
#[derive(Debug, Clone, Default)]
pub struct ReadOptions {
    /// Verify SHA-256 checksums on read (v2 clips only).
    pub verify_checksums: bool,
}

/// Compute SHA-256 hex digest of bytes.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Write a .gameclip package to disk (convenience wrapper with default options).
///
/// The .gameclip format is a zip archive containing:
/// - `metadata.json`: Clip metadata (written last, with checksums)
/// - `input.jsonl`: Newline-delimited JSON input events
/// - `video.bin`: Video data (MP4 on Windows, raw frames for mock)
/// - `audio.bin`: Audio data (optional)
/// - `thumbnail.jpg`: First frame as thumbnail
/// - `frame_actions.jsonl`: Per-frame action data (optional)
/// - `quality.json`: Quality score data (optional)
pub fn write_clip(path: &Path, data: &ClipPackageData) -> Result<(), ClipFormatError> {
    write_clip_with_options(path, data, &WriteOptions::default())
}

/// Write a .gameclip package with explicit options.
///
/// Two-pass write when checksums enabled:
/// 1. Serialize all entry bytes and compute SHA-256 for each
/// 2. Write metadata.json last with checksums populated
pub fn write_clip_with_options(
    path: &Path,
    data: &ClipPackageData,
    options: &WriteOptions,
) -> Result<(), ClipFormatError> {
    // Pre-serialize all entries and collect checksums
    let mut checksums: HashMap<String, String> = HashMap::new();

    // input.jsonl
    let input_bytes = {
        let mut buf = Vec::new();
        for event in &data.input_events {
            let line = serde_json::to_string(event)?;
            buf.extend_from_slice(line.as_bytes());
            buf.push(b'\n');
        }
        buf
    };
    if options.compute_checksums {
        checksums.insert("input.jsonl".to_string(), sha256_hex(&input_bytes));
    }

    // video.bin
    if options.compute_checksums {
        checksums.insert("video.bin".to_string(), sha256_hex(&data.video_data));
    }

    // audio.bin
    if options.compute_checksums && !data.audio_data.is_empty() {
        checksums.insert("audio.bin".to_string(), sha256_hex(&data.audio_data));
    }

    // thumbnail.jpg
    if options.compute_checksums && !data.thumbnail.is_empty() {
        checksums.insert("thumbnail.jpg".to_string(), sha256_hex(&data.thumbnail));
    }

    // frame_actions.jsonl
    let frame_actions_bytes = if !data.frame_actions.is_empty() {
        let mut buf = Vec::new();
        for action in &data.frame_actions {
            let line = serde_json::to_string(action)?;
            buf.extend_from_slice(line.as_bytes());
            buf.push(b'\n');
        }
        if options.compute_checksums {
            checksums.insert("frame_actions.jsonl".to_string(), sha256_hex(&buf));
        }
        Some(buf)
    } else {
        None
    };

    // quality.json
    let quality_bytes = if let Some(ref quality) = data.quality_score {
        let bytes = serde_json::to_string_pretty(quality)?.into_bytes();
        if options.compute_checksums {
            checksums.insert("quality.json".to_string(), sha256_hex(&bytes));
        }
        Some(bytes)
    } else {
        None
    };

    // Build final metadata with version and checksums
    let metadata = ClipMetadata {
        format_version: if options.compute_checksums { 2 } else { data.metadata.format_version },
        checksums: if options.compute_checksums { checksums } else { data.metadata.checksums.clone() },
        ..data.metadata.clone()
    };

    // Now write the zip
    let file = File::create(path)?;
    let mut zip = ZipWriter::new(file);
    let zip_options = FileOptions::<()>::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // Write all data entries first
    zip.start_file("input.jsonl", zip_options)?;
    zip.write_all(&input_bytes)?;

    zip.start_file("video.bin", zip_options)?;
    zip.write_all(&data.video_data)?;

    if !data.audio_data.is_empty() {
        zip.start_file("audio.bin", zip_options)?;
        zip.write_all(&data.audio_data)?;
    }

    if !data.thumbnail.is_empty() {
        zip.start_file("thumbnail.jpg", zip_options)?;
        zip.write_all(&data.thumbnail)?;
    }

    if let Some(ref bytes) = frame_actions_bytes {
        zip.start_file("frame_actions.jsonl", zip_options)?;
        zip.write_all(bytes)?;
    }

    if let Some(ref bytes) = quality_bytes {
        zip.start_file("quality.json", zip_options)?;
        zip.write_all(bytes)?;
    }

    // Write metadata.json LAST (with checksums populated)
    zip.start_file("metadata.json", zip_options)?;
    let metadata_json = serde_json::to_string_pretty(&metadata)?;
    zip.write_all(metadata_json.as_bytes())?;

    zip.finish()?;
    Ok(())
}

/// Contents read back from a .gameclip file.
#[allow(dead_code)]
pub struct ClipPackageContents {
    pub metadata: ClipMetadata,
    pub input_events: Vec<InputEvent>,
    pub video_data: Vec<u8>,
    pub audio_data: Vec<u8>,
    pub thumbnail: Vec<u8>,
    pub frame_actions: Vec<FrameAction>,
    pub quality_score: Option<QualityScore>,
}

/// Read a .gameclip package from disk (convenience wrapper, no verification).
pub fn read_clip(path: &Path) -> Result<ClipPackageContents, ClipFormatError> {
    read_clip_with_options(path, &ReadOptions::default())
}

/// Read a .gameclip package with explicit options.
///
/// When `verify_checksums` is true, verifies SHA-256 checksums for all entries
/// that have them in the metadata. V1 clips (empty checksums) skip verification.
pub fn read_clip_with_options(
    path: &Path,
    options: &ReadOptions,
) -> Result<ClipPackageContents, ClipFormatError> {
    let file = File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    // metadata.json (required)
    let metadata: ClipMetadata = {
        let mut entry = archive
            .by_name("metadata.json")
            .map_err(|_| ClipFormatError::MissingFile("metadata.json".to_string()))?;
        let mut buf = String::new();
        entry.read_to_string(&mut buf)?;
        serde_json::from_str(&buf)?
    };

    // Helper: read an entry and optionally verify checksum
    let verify = options.verify_checksums && !metadata.checksums.is_empty();

    // input.jsonl (required)
    let input_bytes = {
        let mut entry = archive
            .by_name("input.jsonl")
            .map_err(|_| ClipFormatError::MissingFile("input.jsonl".to_string()))?;
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        buf
    };
    if verify {
        verify_checksum("input.jsonl", &input_bytes, &metadata.checksums)?;
    }
    let input_events: Vec<InputEvent> = {
        let text = String::from_utf8_lossy(&input_bytes);
        text.lines()
            .filter(|line| !line.is_empty())
            .map(serde_json::from_str)
            .collect::<Result<Vec<_>, _>>()?
    };

    // video.bin (required)
    let video_data = {
        let mut entry = archive
            .by_name("video.bin")
            .map_err(|_| ClipFormatError::MissingFile("video.bin".to_string()))?;
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        buf
    };
    if verify {
        verify_checksum("video.bin", &video_data, &metadata.checksums)?;
    }

    // audio.bin (optional)
    let audio_data = match archive.by_name("audio.bin") {
        Ok(mut entry) => {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            if verify {
                verify_checksum("audio.bin", &buf, &metadata.checksums)?;
            }
            buf
        }
        Err(_) => Vec::new(),
    };

    // thumbnail.jpg (optional)
    let thumbnail = match archive.by_name("thumbnail.jpg") {
        Ok(mut entry) => {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            if verify {
                verify_checksum("thumbnail.jpg", &buf, &metadata.checksums)?;
            }
            buf
        }
        Err(_) => Vec::new(),
    };

    // frame_actions.jsonl (optional annotation layer)
    let frame_actions = match archive.by_name("frame_actions.jsonl") {
        Ok(mut entry) => {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            if verify {
                verify_checksum("frame_actions.jsonl", &buf, &metadata.checksums)?;
            }
            let text = String::from_utf8_lossy(&buf);
            text.lines()
                .filter(|line| !line.is_empty())
                .map(serde_json::from_str)
                .collect::<Result<Vec<_>, _>>()?
        }
        Err(_) => Vec::new(),
    };

    // quality.json (optional annotation layer)
    let quality_score = match archive.by_name("quality.json") {
        Ok(mut entry) => {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            if verify {
                verify_checksum("quality.json", &buf, &metadata.checksums)?;
            }
            let text = String::from_utf8_lossy(&buf);
            Some(serde_json::from_str(&text)?)
        }
        Err(_) => None,
    };

    Ok(ClipPackageContents {
        metadata,
        input_events,
        video_data,
        audio_data,
        thumbnail,
        frame_actions,
        quality_score,
    })
}

/// Verify a single entry's checksum against the metadata checksums map.
///
/// If the entry has no checksum recorded, silently passes (graceful for v1 clips).
fn verify_checksum(
    name: &str,
    data: &[u8],
    checksums: &HashMap<String, String>,
) -> Result<(), ClipFormatError> {
    if let Some(expected) = checksums.get(name) {
        let actual = sha256_hex(data);
        if actual != *expected {
            return Err(ClipFormatError::ChecksumMismatch {
                file: name.to_string(),
                expected: expected.clone(),
                actual,
            });
        }
    }
    Ok(())
}

/// Migrate a v1 .gameclip file to v2 format (with checksums).
///
/// Reads the clip, then rewrites it with checksums computed for all entries.
/// The original file is overwritten.
#[allow(dead_code)]
pub fn migrate_v1_to_v2(path: &Path) -> Result<(), ClipFormatError> {
    let contents = read_clip(path)?;

    let package = ClipPackageData {
        metadata: contents.metadata,
        input_events: contents.input_events,
        video_data: contents.video_data,
        audio_data: contents.audio_data,
        thumbnail: contents.thumbnail,
        frame_actions: contents.frame_actions,
        quality_score: contents.quality_score,
    };

    write_clip_with_options(path, &package, &WriteOptions { compute_checksums: true })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{InputEventKind, KeyEvent};
    use tempfile::TempDir;

    fn make_test_data() -> ClipPackageData {
        let metadata = ClipMetadata::new("test_clip".to_string(), "Test Clip".to_string());

        let input_events = vec![
            InputEvent {
                timestamp_us: 1000,
                kind: InputEventKind::Key(KeyEvent {
                    key: "KeyW".to_string(),
                    pressed: true,
                }),
            },
            InputEvent {
                timestamp_us: 2000,
                kind: InputEventKind::Key(KeyEvent {
                    key: "KeyW".to_string(),
                    pressed: false,
                }),
            },
        ];

        ClipPackageData {
            metadata,
            input_events,
            video_data: vec![0xFF, 0x00, 0x00, 0xFF], // fake frame
            audio_data: vec![0x00; 1024],
            thumbnail: vec![0xFF, 0xD8, 0xFF], // fake JPEG header
            frame_actions: vec![],
            quality_score: None,
        }
    }

    #[test]
    fn write_and_read_roundtrip() {
        let dir = TempDir::new().unwrap();
        let clip_path = dir.path().join("test.gameclip");

        let data = make_test_data();
        write_clip(&clip_path, &data).unwrap();

        let contents = read_clip(&clip_path).unwrap();
        assert_eq!(contents.metadata.id, "test_clip");
        assert_eq!(contents.metadata.name, "Test Clip");
        assert_eq!(contents.input_events.len(), 2);
        assert_eq!(contents.video_data, vec![0xFF, 0x00, 0x00, 0xFF]);
        assert_eq!(contents.audio_data.len(), 1024);
        assert_eq!(contents.thumbnail, vec![0xFF, 0xD8, 0xFF]);
    }

    #[test]
    fn input_events_preserved() {
        let dir = TempDir::new().unwrap();
        let clip_path = dir.path().join("test.gameclip");

        let data = make_test_data();
        write_clip(&clip_path, &data).unwrap();

        let contents = read_clip(&clip_path).unwrap();
        assert_eq!(contents.input_events[0].timestamp_us, 1000);
        assert_eq!(contents.input_events[1].timestamp_us, 2000);

        if let InputEventKind::Key(ref key) = contents.input_events[0].kind {
            assert_eq!(key.key, "KeyW");
            assert!(key.pressed);
        } else {
            panic!("expected key event");
        }
    }

    #[test]
    fn file_is_valid_zip() {
        let dir = TempDir::new().unwrap();
        let clip_path = dir.path().join("test.gameclip");

        let data = make_test_data();
        write_clip(&clip_path, &data).unwrap();

        let file = File::open(&clip_path).unwrap();
        let archive = zip::ZipArchive::new(file).unwrap();
        let names: Vec<&str> = archive.file_names().collect();

        assert!(names.contains(&"metadata.json"));
        assert!(names.contains(&"input.jsonl"));
        assert!(names.contains(&"video.bin"));
        assert!(names.contains(&"audio.bin"));
        assert!(names.contains(&"thumbnail.jpg"));
    }

    #[test]
    fn handles_no_audio() {
        let dir = TempDir::new().unwrap();
        let clip_path = dir.path().join("test.gameclip");

        let mut data = make_test_data();
        data.audio_data = vec![];

        write_clip(&clip_path, &data).unwrap();

        let contents = read_clip(&clip_path).unwrap();
        assert!(contents.audio_data.is_empty());
    }

    #[test]
    fn handles_no_thumbnail() {
        let dir = TempDir::new().unwrap();
        let clip_path = dir.path().join("test.gameclip");

        let mut data = make_test_data();
        data.thumbnail = vec![];

        write_clip(&clip_path, &data).unwrap();

        let contents = read_clip(&clip_path).unwrap();
        assert!(contents.thumbnail.is_empty());
    }

    #[test]
    fn read_nonexistent_file_fails() {
        let result = read_clip(Path::new("/tmp/nonexistent.gameclip"));
        assert!(result.is_err());
    }

    // T17: write produces format_version=2 in output
    #[test]
    fn write_produces_format_version_2() {
        let dir = TempDir::new().unwrap();
        let clip_path = dir.path().join("test.gameclip");

        let data = make_test_data();
        write_clip(&clip_path, &data).unwrap();

        let contents = read_clip(&clip_path).unwrap();
        assert_eq!(contents.metadata.format_version, 2);
    }

    // T18: write populates checksums for all written entries
    #[test]
    fn write_populates_checksums_for_all_entries() {
        let dir = TempDir::new().unwrap();
        let clip_path = dir.path().join("test.gameclip");

        let data = make_test_data();
        write_clip(&clip_path, &data).unwrap();

        let contents = read_clip(&clip_path).unwrap();
        assert!(contents.metadata.checksums.contains_key("input.jsonl"));
        assert!(contents.metadata.checksums.contains_key("video.bin"));
        assert!(contents.metadata.checksums.contains_key("audio.bin"));
        assert!(contents.metadata.checksums.contains_key("thumbnail.jpg"));
    }

    // T19: checksums are valid 64-char hex strings
    #[test]
    fn checksums_are_valid_hex() {
        let dir = TempDir::new().unwrap();
        let clip_path = dir.path().join("test.gameclip");

        let data = make_test_data();
        write_clip(&clip_path, &data).unwrap();

        let contents = read_clip(&clip_path).unwrap();
        for (name, checksum) in &contents.metadata.checksums {
            assert_eq!(
                checksum.len(), 64,
                "checksum for {name} should be 64 hex chars, got {}", checksum.len()
            );
            assert!(
                checksum.chars().all(|c| c.is_ascii_hexdigit()),
                "checksum for {name} should be valid hex"
            );
        }
    }

    // T20: read with verify=true passes on intact file
    #[test]
    fn read_with_verify_passes_on_intact_file() {
        let dir = TempDir::new().unwrap();
        let clip_path = dir.path().join("test.gameclip");

        let data = make_test_data();
        write_clip(&clip_path, &data).unwrap();

        let opts = ReadOptions { verify_checksums: true };
        let result = read_clip_with_options(&clip_path, &opts);
        assert!(result.is_ok());
    }

    // T21: read with verify=false skips check (fast path)
    #[test]
    fn read_with_verify_false_skips_check() {
        let dir = TempDir::new().unwrap();
        let clip_path = dir.path().join("test.gameclip");

        let data = make_test_data();
        write_clip(&clip_path, &data).unwrap();

        // Even if we tamper, verify=false should succeed
        let opts = ReadOptions { verify_checksums: false };
        let result = read_clip_with_options(&clip_path, &opts);
        assert!(result.is_ok());
    }

    // T22: corrupted video.bin detected by checksum
    #[test]
    fn corrupted_video_detected_by_checksum() {
        let dir = TempDir::new().unwrap();
        let clip_path = dir.path().join("test.gameclip");

        let data = make_test_data();
        write_clip(&clip_path, &data).unwrap();

        // Tamper with the zip file — overwrite video.bin content
        // We need to read the zip, modify video.bin, and rewrite
        {
            let file = File::open(&clip_path).unwrap();
            let mut archive = zip::ZipArchive::new(file).unwrap();

            let tampered_path = dir.path().join("tampered.gameclip");
            let out_file = File::create(&tampered_path).unwrap();
            let mut zip = ZipWriter::new(out_file);
            let opts = FileOptions::<()>::default()
                .compression_method(zip::CompressionMethod::Deflated);

            for i in 0..archive.len() {
                let mut entry = archive.by_index(i).unwrap();
                let name = entry.name().to_string();
                zip.start_file(&name, opts).unwrap();

                let mut buf = Vec::new();
                entry.read_to_end(&mut buf).unwrap();

                if name == "video.bin" {
                    // Tamper the data
                    buf = vec![0xDE, 0xAD, 0xBE, 0xEF];
                }
                zip.write_all(&buf).unwrap();
            }
            zip.finish().unwrap();

            // Replace original with tampered
            std::fs::rename(&tampered_path, &clip_path).unwrap();
        }

        let opts = ReadOptions { verify_checksums: true };
        let result = read_clip_with_options(&clip_path, &opts);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(
            matches!(err, ClipFormatError::ChecksumMismatch { ref file, .. } if file == "video.bin"),
            "expected ChecksumMismatch for video.bin, got: {err}"
        );
    }

    // T23: v1 JSON without format_version deserializes with default=1
    #[test]
    fn v1_json_without_format_version_deserializes() {
        let json = r#"{
            "id": "v1_clip",
            "name": "V1 Clip",
            "width": 1920,
            "height": 1080,
            "fps": 60,
            "duration_secs": 10.0,
            "input_event_count": 100,
            "has_audio": false,
            "created_at": "2024-01-01T00:00:00Z",
            "devices": {"keyboard": true, "mouse": false, "controller": false},
            "video_encoded": true,
            "video_start_timestamp_us": 0
        }"#;

        let meta: ClipMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(meta.format_version, 1);
        assert!(meta.checksums.is_empty());
    }

    // T24: v1 clip with empty checksums passes verify (graceful skip)
    #[test]
    fn v1_clip_empty_checksums_passes_verify() {
        let dir = TempDir::new().unwrap();
        let clip_path = dir.path().join("test.gameclip");

        // Write without checksums
        let data = make_test_data();
        write_clip_with_options(
            &clip_path,
            &data,
            &WriteOptions { compute_checksums: false },
        ).unwrap();

        // Reading with verify should still pass (empty checksums = skip)
        let opts = ReadOptions { verify_checksums: true };
        let result = read_clip_with_options(&clip_path, &opts);
        assert!(result.is_ok());
        assert!(result.unwrap().metadata.checksums.is_empty());
    }

    // T25: migrate_v1_to_v2 produces a v2 clip with checksums
    #[test]
    fn migrate_v1_to_v2_produces_v2_clip() {
        let dir = TempDir::new().unwrap();
        let clip_path = dir.path().join("test.gameclip");

        // Write a v1 clip (no checksums)
        let data = make_test_data();
        write_clip_with_options(
            &clip_path,
            &data,
            &WriteOptions { compute_checksums: false },
        ).unwrap();

        // Verify it's v1-like (no checksums)
        let pre = read_clip(&clip_path).unwrap();
        assert!(pre.metadata.checksums.is_empty());

        // Migrate
        migrate_v1_to_v2(&clip_path).unwrap();

        // Verify it's now v2
        let post = read_clip(&clip_path).unwrap();
        assert_eq!(post.metadata.format_version, 2);
        assert!(!post.metadata.checksums.is_empty());
    }

    // T26: migrated clip passes checksum verification
    #[test]
    fn migrated_clip_passes_verification() {
        let dir = TempDir::new().unwrap();
        let clip_path = dir.path().join("test.gameclip");

        let data = make_test_data();
        write_clip_with_options(
            &clip_path,
            &data,
            &WriteOptions { compute_checksums: false },
        ).unwrap();

        migrate_v1_to_v2(&clip_path).unwrap();

        let opts = ReadOptions { verify_checksums: true };
        let result = read_clip_with_options(&clip_path, &opts);
        assert!(result.is_ok());
    }

    // T27: all existing format.rs roundtrip tests still pass (regression)
    // Covered by the existing tests above (write_and_read_roundtrip, etc.)
}
