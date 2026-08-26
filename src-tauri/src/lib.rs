mod agent;
mod audit;
mod brief;
mod browser;
mod commands;
mod confirmation;
mod db;
mod elevenlabs;
mod files;
mod followups;
mod memory;
mod notify;
#[cfg(test)]
mod pipeline_test;
mod policy;
mod secrets;
mod tasks;
mod tools;
mod tray;
mod voice;

use tauri::{Emitter, Manager};

// Amin is a macOS desktop app by design — no mobile entry point, no
// Android/iOS targets. See docs/ARCHITECTURE.md.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        // Tauri's auto-generated macOS menu bar (File/Edit/View/Go/Window/
        // Help) is a generic document-app default — the "View"/"Go"/
        // "Window" items don't apply to Amin's single voice-first window,
        // and Mona flagged the whole bar directly as visual clutter that
        // doesn't belong in a presence-style app. Disabled here; a minimal
        // replacement (just enough for Quit and text-field copy/paste) is
        // installed explicitly in `setup` below instead of leaving the app
        // with no menu at all.
        .enable_macos_default_menu(false)
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                use tauri::menu::{Menu, PredefinedMenuItem, Submenu};
                let handle = app.handle();
                let app_menu = Submenu::with_items(
                    handle,
                    "أمين",
                    true,
                    &[&PredefinedMenuItem::quit(handle, None)?],
                )?;
                let edit_menu = Submenu::with_items(
                    handle,
                    "Edit",
                    true,
                    &[
                        &PredefinedMenuItem::cut(handle, None)?,
                        &PredefinedMenuItem::copy(handle, None)?,
                        &PredefinedMenuItem::paste(handle, None)?,
                        &PredefinedMenuItem::select_all(handle, None)?,
                    ],
                )?;
                app.set_menu(Menu::with_items(handle, &[&app_menu, &edit_menu])?)?;
            }

            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let database = db::init(dir.join("amin.db"))?;
            let history = {
                let conn = database.0.lock().map_err(|e| e.to_string())?;
                commands::load_conversation_history(&conn)
            };
            app.manage(database);
            // Seeded from disk, not empty — long-term memory across app
            // restarts, per Mona's request. See agent::Conversation and
            // commands::load_conversation_history/clear_agent_conversation.
            app.manage(agent::Conversation::with_history(history));
            app.manage(confirmation::PendingConfirmation::new());
            app.manage(voice::VoiceSession::new());
            app.manage(voice::HandsFreeSession::new());

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
                                    // See commands::start_voice_capture's own
                                    // guard — the two native voice engines
                                    // each open their own AVAudioEngine and
                                    // shouldn't run at once.
                                    if app.state::<voice::HandsFreeSession>().is_active() {
                                        let _ = app.emit(
                                            "voice://error",
                                            "الاستماع الحر شغّال دلوقتي — قفليه الأول لو عايزة تستخدمي alt+A",
                                        );
                                        return;
                                    }
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
            // Originally hid the window on close and relied on the tray
            // icon's "Show Amin" item to bring it back — a real menu-bar-
            // app pattern, but one that turned out to depend entirely on
            // that tray icon actually being visible. On at least one real
            // Mac it wasn't (unclear why — unverified without more Macs to
            // test on), which left the app running invisibly with no way
            // back in short of Activity Monitor. Closing the window now
            // just quits, like any ordinary app: predictable, and doesn't
            // depend on the tray icon rendering. Revisit menu-bar
            // residency once the tray icon's reliability is confirmed.
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                window.app_handle().exit(0);
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::has_api_key,
            commands::save_api_key,
            commands::clear_api_key,
            commands::has_elevenlabs_key,
            commands::save_elevenlabs_key,
            commands::clear_elevenlabs_key,
            commands::get_elevenlabs_voice_id,
            commands::save_elevenlabs_voice_id,
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
            commands::speak_text,
            commands::stop_speaking,
            commands::get_hands_free_settings,
            commands::save_hands_free_phrases,
            commands::set_hands_free_mode,
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
            commands::get_pending_action,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Amin");
}
