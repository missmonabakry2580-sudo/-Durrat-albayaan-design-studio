mod audit;
mod commands;
mod db;
mod policy;
mod secrets;

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
            Ok(())
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running Amin");
}
