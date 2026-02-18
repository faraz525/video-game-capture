use super::metadata::ClipMetadata;
use crate::input::InputEvent;
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
}

/// Write a .gameclip package to disk.
///
/// The .gameclip format is a zip archive containing:
/// - `metadata.json`: Clip metadata
/// - `input.jsonl`: Newline-delimited JSON input events
/// - `video.bin`: Video data (MP4 on Windows, raw frames for mock)
/// - `audio.bin`: Audio data (optional)
/// - `thumbnail.jpg`: First frame as thumbnail
pub fn write_clip(path: &Path, data: &ClipPackageData) -> Result<(), ClipFormatError> {
    let file = File::create(path)?;
    let mut zip = ZipWriter::new(file);
    let options = FileOptions::<()>::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // metadata.json
    zip.start_file("metadata.json", options)?;
    let metadata_json = serde_json::to_string_pretty(&data.metadata)?;
    zip.write_all(metadata_json.as_bytes())?;

    // input.jsonl
    zip.start_file("input.jsonl", options)?;
    for event in &data.input_events {
        let line = serde_json::to_string(event)?;
        zip.write_all(line.as_bytes())?;
        zip.write_all(b"\n")?;
    }

    // video.bin
    zip.start_file("video.bin", options)?;
    zip.write_all(&data.video_data)?;

    // audio.bin (only if we have audio data)
    if !data.audio_data.is_empty() {
        zip.start_file("audio.bin", options)?;
        zip.write_all(&data.audio_data)?;
    }

    // thumbnail.jpg
    if !data.thumbnail.is_empty() {
        zip.start_file("thumbnail.jpg", options)?;
        zip.write_all(&data.thumbnail)?;
    }

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
}

/// Read a .gameclip package from disk.
pub fn read_clip(path: &Path) -> Result<ClipPackageContents, ClipFormatError> {
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

    // input.jsonl (required)
    let input_events: Vec<InputEvent> = {
        let mut entry = archive
            .by_name("input.jsonl")
            .map_err(|_| ClipFormatError::MissingFile("input.jsonl".to_string()))?;
        let mut buf = String::new();
        entry.read_to_string(&mut buf)?;
        buf.lines()
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

    // audio.bin (optional)
    let audio_data = match archive.by_name("audio.bin") {
        Ok(mut entry) => {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            buf
        }
        Err(_) => Vec::new(),
    };

    // thumbnail.jpg (optional)
    let thumbnail = match archive.by_name("thumbnail.jpg") {
        Ok(mut entry) => {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            buf
        }
        Err(_) => Vec::new(),
    };

    Ok(ClipPackageContents {
        metadata,
        input_events,
        video_data,
        audio_data,
        thumbnail,
    })
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
}
