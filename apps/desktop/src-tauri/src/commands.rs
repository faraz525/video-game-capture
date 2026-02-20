use crate::clip::format::read_clip;
use crate::clip::metadata::ClipMetadata;
use crate::engine::{AppSettings, EngineState};
use crate::input::InputEvent;
use base64::Engine as _;
use log::{debug, info, warn};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;

/// Serializable clip summary for the frontend.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ClipSummary {
    pub id: String,
    pub name: String,
    pub game: Option<String>,
    pub duration_secs: f64,
    pub created_at: String,
    pub file_path: String,
    pub input_event_count: u64,
    pub has_audio: bool,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub video_encoded: bool,
}

/// List all saved clips from the save directory.
#[tauri::command]
pub fn list_clips(state: State<'_, EngineState>) -> Result<Vec<ClipSummary>, String> {
    let settings = state.settings.lock().map_err(|e| e.to_string())?;
    let save_dir = PathBuf::from(&settings.save_directory);

    if !save_dir.exists() {
        return Ok(vec![]);
    }

    let mut clips = Vec::new();
    let entries = fs::read_dir(&save_dir).map_err(|e| e.to_string())?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("gameclip") {
            match read_clip(&path) {
                Ok(contents) => {
                    clips.push(ClipSummary {
                        id: contents.metadata.id.clone(),
                        name: contents.metadata.name.clone(),
                        game: contents.metadata.game.clone(),
                        duration_secs: contents.metadata.duration_secs,
                        created_at: contents.metadata.created_at.to_rfc3339(),
                        file_path: path.to_string_lossy().to_string(),
                        input_event_count: contents.metadata.input_event_count,
                        has_audio: contents.metadata.has_audio,
                        width: contents.metadata.width,
                        height: contents.metadata.height,
                        fps: contents.metadata.fps,
                        video_encoded: contents.metadata.video_encoded,
                    });
                }
                Err(e) => {
                    warn!("Failed to read clip {}: {e}", path.display());
                }
            }
        }
    }

    clips.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(clips)
}

/// Get full metadata for a specific clip.
#[tauri::command]
pub fn get_clip_metadata(file_path: String) -> Result<ClipMetadata, String> {
    let path = PathBuf::from(&file_path);
    let contents = read_clip(&path).map_err(|e| e.to_string())?;
    Ok(contents.metadata)
}

/// Delete a clip file.
#[tauri::command]
pub fn delete_clip(file_path: String) -> Result<(), String> {
    let path = PathBuf::from(&file_path);
    if path.exists() {
        fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Trigger a clip save from the frontend.
///
/// Runs the heavy encoding/packaging work on a blocking thread so the
/// Tauri main thread (and therefore the UI) stays responsive.
#[tauri::command]
pub async fn save_clip(state: State<'_, EngineState>) -> Result<String, String> {
    let saver = Arc::clone(&state.saver);
    tauri::async_runtime::spawn_blocking(move || {
        crate::engine::save_clip(&saver)
            .map(|p| p.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Get current app settings.
#[tauri::command]
pub fn get_settings(state: State<'_, EngineState>) -> Result<AppSettings, String> {
    let settings = state.settings.lock().map_err(|e| e.to_string())?;
    Ok(settings.clone())
}

/// Update app settings.
#[tauri::command]
pub fn update_settings(
    state: State<'_, EngineState>,
    new_settings: AppSettings,
) -> Result<(), String> {
    let mut settings = state.settings.lock().map_err(|e| e.to_string())?;
    *settings = new_settings;
    Ok(())
}

/// Extract video data from a .gameclip file and write to a temp MP4 file.
/// Returns the temp file path for use with convertFileSrc().
///
/// If the clip's video is stored as raw RGBA (encoding failed at save time),
/// this command re-encodes it to MP4 using FFmpeg before returning.
#[tauri::command]
pub async fn extract_clip_video(file_path: String) -> Result<String, String> {
    debug!("Extracting video from clip: {file_path}");
    let path = PathBuf::from(&file_path);
    let contents = read_clip(&path).map_err(|e| e.to_string())?;

    let playback_dir = std::env::temp_dir().join("gameclip_playback");
    fs::create_dir_all(&playback_dir).map_err(|e| e.to_string())?;

    let out_path = playback_dir.join(format!("{}.mp4", contents.metadata.id));

    // Detect if video is actually encoded MP4 by checking for ftyp box
    // (older clips don't have video_encoded field, defaults to true incorrectly)
    let is_mp4 = contents.video_data.len() >= 8
        && &contents.video_data[4..8] == b"ftyp";
    let is_encoded = contents.metadata.video_encoded && is_mp4;

    if is_encoded {
        debug!("Video is already encoded MP4, writing {} bytes", contents.video_data.len());
        fs::write(&out_path, &contents.video_data).map_err(|e| e.to_string())?;
        return Ok(out_path.to_string_lossy().to_string());
    }

    // Raw RGBA data — re-encode to MP4 on a blocking thread
    info!(
        "Re-encoding raw RGBA video for clip {} ({}x{} @ {}fps)",
        contents.metadata.id, contents.metadata.width, contents.metadata.height, contents.metadata.fps
    );

    let meta = contents.metadata.clone();
    let video_data = contents.video_data;
    let out = out_path.clone();

    let mp4_data = tauri::async_runtime::spawn_blocking(move || {
        crate::clip::encoder::reencode_raw_to_mp4(
            &video_data,
            meta.width,
            meta.height,
            meta.fps,
        )
    })
    .await
    .map_err(|e| format!("spawn error: {e}"))?
    .map_err(|e| format!("re-encode failed: {e}"))?;

    fs::write(&out, &mp4_data).map_err(|e| e.to_string())?;
    info!("Re-encoded clip written to {}", out.display());
    Ok(out_path.to_string_lossy().to_string())
}

/// Get thumbnail from a .gameclip file as a base64 data URL.
/// Returns None if the clip has no thumbnail.
#[tauri::command]
pub fn get_clip_thumbnail(file_path: String) -> Result<Option<String>, String> {
    debug!("Loading thumbnail for clip: {file_path}");
    let path = PathBuf::from(&file_path);
    let contents = read_clip(&path).map_err(|e| e.to_string())?;

    if contents.thumbnail.is_empty() {
        return Ok(None);
    }

    let b64 = base64::engine::general_purpose::STANDARD.encode(&contents.thumbnail);
    Ok(Some(format!("data:image/jpeg;base64,{b64}")))
}

/// Get input events from a .gameclip file.
#[tauri::command]
pub fn get_clip_input_events(file_path: String) -> Result<Vec<InputEvent>, String> {
    let path = PathBuf::from(&file_path);
    let contents = read_clip(&path).map_err(|e| e.to_string())?;
    Ok(contents.input_events)
}
