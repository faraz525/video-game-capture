mod annotation;
mod audio;
mod capture;
mod clip;
mod game;
mod input;
mod sync;
mod upload;

mod commands;
mod engine;

use log::{error, info};
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .manage(engine::create_engine_state())
        .setup(|app| {
            let settings_item =
                MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&settings_item, &quit_item])?;

            let _tray = TrayIconBuilder::new()
                .menu(&menu)
                .show_menu_on_left_click(true)
                .tooltip("GameClip")
                .on_menu_event(|app_handle, event| match event.id.as_ref() {
                    "settings" => {
                        if let Some(window) = app_handle.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        app_handle.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            // Initialize file + terminal logging
            {
                use simplelog::{
                    CombinedLogger, Config, LevelFilter, TermLogger, TerminalMode,
                    WriteLogger, ColorChoice,
                };

                let home = std::env::var("HOME")
                    .or_else(|_| std::env::var("USERPROFILE"))
                    .unwrap_or_else(|_| ".".to_string());
                let log_dir = std::path::PathBuf::from(&home).join("GameClip");
                let _ = std::fs::create_dir_all(&log_dir);
                let log_path = log_dir.join("gameclip.log");

                let file_logger: Box<dyn simplelog::SharedLogger> = match std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log_path)
                {
                    Ok(file) => WriteLogger::new(LevelFilter::Debug, Config::default(), file),
                    Err(_) => WriteLogger::new(
                        LevelFilter::Debug,
                        Config::default(),
                        std::io::sink(),
                    ),
                };

                let _ = CombinedLogger::init(vec![
                    TermLogger::new(
                        LevelFilter::Debug,
                        Config::default(),
                        TerminalMode::Mixed,
                        ColorChoice::Auto,
                    ),
                    file_logger,
                ]);

                info!(
                    "GameClip v{} starting — log file: {}",
                    env!("CARGO_PKG_VERSION"),
                    log_path.display()
                );
            }

            // Resolve FFmpeg sidecar path (if bundled with the app)
            {
                // The sidecar binary is placed next to the app executable
                // by Tauri's bundling process. We resolve it by looking
                // in the same directory as the current executable.
                if let Ok(exe_path) = std::env::current_exe() {
                    if let Some(exe_dir) = exe_path.parent() {
                        let triple = env!("TAURI_ENV_TARGET_TRIPLE");
                        let sidecar_name = format!("ffmpeg-{triple}");
                        let sidecar_path = exe_dir.join(&sidecar_name);
                        if sidecar_path.exists() {
                            let path_str = sidecar_path.to_string_lossy().to_string();
                            info!("FFmpeg sidecar resolved: {path_str}");
                            clip::encoder::set_sidecar_path(path_str);
                        } else {
                            info!("FFmpeg sidecar not found at {}, will use system FFmpeg", sidecar_path.display());
                        }
                    }
                }
            }

            // Start the capture engine
            {
                let state = app.state::<engine::EngineState>();
                engine::start_capture(&state)?;
            }

            #[cfg(desktop)]
            {
                use tauri_plugin_global_shortcut::{
                    Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState,
                };

                let record_shortcut =
                    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyR);

                let app_handle = app.handle().clone();
                app.handle().plugin(
                    tauri_plugin_global_shortcut::Builder::new()
                        .with_handler(move |_app, shortcut, event| {
                            if shortcut == &record_shortcut
                                && event.state() == ShortcutState::Pressed
                            {
                                let state = app_handle.state::<engine::EngineState>();
                                let saver = Arc::clone(&state.saver);
                                let handle = app_handle.clone();
                                std::thread::spawn(move || {
                                    match engine::save_clip(&saver) {
                                        Ok(path) => {
                                            info!(
                                                "Clip saved: {}",
                                                path.display()
                                            );
                                            if let Some(window) =
                                                handle.get_webview_window("main")
                                            {
                                                let _ = window.emit("clip-saved", path.to_string_lossy().to_string());
                                            }
                                        }
                                        Err(e) => {
                                            error!("Failed to save clip: {e}");
                                        }
                                    }
                                });
                            }
                        })
                        .build(),
                )?;

                app.global_shortcut().register(record_shortcut)?;
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_clips,
            commands::get_clip_metadata,
            commands::delete_clip,
            commands::save_clip,
            commands::get_settings,
            commands::update_settings,
            commands::extract_clip_video,
            commands::get_clip_thumbnail,
            commands::get_clip_input_events,
            commands::annotate_clip,
            commands::get_frame_actions,
            commands::get_quality_score,
            commands::export_clips,
            commands::upload_clips,
            commands::cancel_upload,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
