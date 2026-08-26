mod agent;
mod audit;
mod brief;
mod browser;
mod commands;
mod db;
mod files;
mod followups;
mod notify;
mod policy;
mod secrets;
mod tasks;
mod tray;
mod voice;

use tauri::{Emitter, Manager};

// Amin is a macOS desktop app by design — no mobile entry point, no
// Android/iOS targets. See docs/ARCHITECTURE.md.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let database = db::init(dir.join("amin.db"))?;
            app.manage(database);
            app.manage(agent::Conversation::new());
            app.manage(voice::VoiceSession::new());

            tray::setup(app.handle())?;

            // Push-to-talk: hold the shortcut to listen, release to stop.
            // Placeholder key combo — alt+A is unlikely to clash with
            // common macOS/app shortcuts but hasn't been tried on a real
            // Mac yet; change it here (and tell Mona) if it turns out to
            // collide with something. See docs/ARCHITECTURE.md "Voice
            // pipeline" for why the actual transcription helper isn't
            // wired up yet — this registers the key even so, so wiring it
            // in later is a one-file change.
            #[cfg(desktop)]
            {
                use tauri_plugin_global_shortcut::{Code, Modifiers, ShortcutState};

                app.handle().plugin(
                    tauri_plugin_global_shortcut::Builder::new()
                        .with_shortcuts(["alt+a"])?
                        .with_handler(|app, shortcut, event| {
                            if !shortcut.matches(Modifiers::ALT, Code::KeyA) {
                                return;
                            }
                            let session = app.state::<voice::VoiceSession>();
                            match event.state {
                                ShortcutState::Pressed => {
                                    if let Err(e) = voice::start_listening(app.clone(), session) {
                                        let _ = app.emit("voice://error", e);
                                    } else {
                                        let _ = app.emit("voice://state", "listening");
                                    }
                                }
                                ShortcutState::Released => {
                                    let _ = voice::stop_listening(session);
                                    let _ = app.emit("voice://state", "idle");
                                }
                            }
                        })
                        .build(),
                )?;
            }

            // Menu-bar presence, not a Dock app: no icon bouncing in the
            // Dock or app-switcher entry. See docs/ARCHITECTURE.md.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window hides Amin; it keeps running in the menu
            // bar. Only the tray's "Quit Amin" item actually exits.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::has_api_key,
            commands::save_api_key,
            commands::clear_api_key,
            commands::get_autonomy_level,
            commands::set_autonomy_level,
            commands::is_halted,
            commands::set_kill_switch,
            commands::classify_action,
            commands::list_audit_log,
            commands::send_agent_message,
            commands::clear_agent_conversation,
            commands::start_voice_capture,
            commands::stop_voice_capture,
            commands::create_task,
            commands::quick_capture,
            commands::list_tasks,
            commands::set_task_status,
            commands::list_workspace_files,
            commands::read_workspace_file,
            commands::write_workspace_file,
            commands::delete_workspace_file,
            commands::open_browser_url,
            commands::create_follow_up,
            commands::list_follow_ups,
            commands::list_due_follow_ups,
            commands::escalate_follow_up,
            commands::set_follow_up_status,
            commands::generate_delta_brief,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Amin");
}
