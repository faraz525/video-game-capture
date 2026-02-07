use crate::clip::format::read_clip;
use crate::clip::metadata::ClipMetadata;
use crate::engine::{AppSettings, EngineState};
use std::fs;
use std::path::PathBuf;
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
                    });
                }
                Err(e) => {
                    eprintln!("[GameClip] Failed to read clip {}: {e}", path.display());
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
#[tauri::command]
pub fn save_clip(state: State<'_, EngineState>) -> Result<String, String> {
    let path = crate::engine::save_clip(&state)?;
    Ok(path.to_string_lossy().to_string())
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
