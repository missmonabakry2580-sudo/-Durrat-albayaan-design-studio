mod agent;
mod audit;
mod commands;
mod db;
mod policy;
mod secrets;
mod tray;

use tauri::Manager;

// Amin is a macOS desktop app by design — no mobile entry point, no
// Android/iOS targets. See docs/ARCHITECTURE.md.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let database = db::init(dir.join("amin.db"))?;
            app.manage(database);
            app.manage(agent::Conversation::new());

            tray::setup(app.handle())?;

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
        ])
        .run(tauri::generate_context!())
        .expect("error while running Amin");
}
