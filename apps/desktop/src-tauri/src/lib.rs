mod audio;
mod capture;
mod clip;
mod game;
mod input;
mod sync;

mod commands;
mod engine;

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
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
                                match engine::save_clip(&state) {
                                    Ok(path) => {
                                        println!(
                                            "[GameClip] Clip saved: {}",
                                            path.display()
                                        );
                                        if let Some(window) =
                                            app_handle.get_webview_window("main")
                                        {
                                            let _ = window.emit("clip-saved", path.to_string_lossy().to_string());
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("[GameClip] Failed to save clip: {e}");
                                    }
                                }
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
