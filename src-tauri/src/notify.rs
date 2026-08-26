use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

/// The one real delivery channel for the Follow-up Engine that needs no
/// external account: a native OS notification. Never panics on failure —
/// a missing notification permission or an unsupported environment (no
/// notification daemon, as in a headless Linux sandbox) should degrade to
/// "nothing shown", not crash Amin.
pub fn send(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}
