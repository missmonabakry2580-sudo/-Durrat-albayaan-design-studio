use tauri::{AppHandle, State};

use crate::db::Db;
use crate::files::WorkspaceEntry;
use crate::policy::{self, AutonomyLevel, RiskTier};
use crate::tasks::Task;
use crate::voice::VoiceSession;
use crate::{agent, audit, files, secrets, tasks, voice};

const ANTHROPIC_KEY_NAME: &str = "anthropic_api_key";

fn set_setting(conn: &rusqlite::Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        rusqlite::params![key, value, chrono::Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn get_setting(conn: &rusqlite::Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        [key],
        |row| row.get(0),
    )
    .ok()
}

#[derive(serde::Serialize)]
pub struct AppInfo {
    name: &'static str,
    version: &'static str,
}

#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        name: "Amin",
        version: env!("CARGO_PKG_VERSION"),
    }
}

#[tauri::command]
pub fn has_api_key() -> bool {
    secrets::has_secret(ANTHROPIC_KEY_NAME)
}

#[tauri::command]
pub fn save_api_key(key: String, db: State<Db>) -> Result<(), String> {
    secrets::set_secret(ANTHROPIC_KEY_NAME, &key)?;
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    audit::record(
        &conn,
        "user",
        "save_api_key",
        RiskTier::TrustedDelegation,
        audit::Decision::Confirmed,
        None,
        None,
    )
}

#[tauri::command]
pub fn clear_api_key(db: State<Db>) -> Result<(), String> {
    secrets::clear_secret(ANTHROPIC_KEY_NAME)?;
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    audit::record(
        &conn,
        "user",
        "clear_api_key",
        RiskTier::TrustedDelegation,
        audit::Decision::Confirmed,
        None,
        None,
    )
}

#[tauri::command]
pub fn get_autonomy_level(db: State<Db>) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let level = get_setting(&conn, "autonomy_level").unwrap_or_else(|| {
        AutonomyLevel::default().as_str().to_string()
    });
    Ok(level)
}

#[tauri::command]
pub fn set_autonomy_level(level: String, db: State<Db>) -> Result<(), String> {
    let parsed = AutonomyLevel::parse(&level)?;
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    set_setting(&conn, "autonomy_level", parsed.as_str())?;
    audit::record(
        &conn,
        "user",
        "set_autonomy_level",
        RiskTier::TrustedDelegation,
        audit::Decision::Confirmed,
        Some(parsed.as_str()),
        None,
    )
}

#[tauri::command]
pub fn is_halted(db: State<Db>) -> Result<bool, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(get_setting(&conn, "kill_switch").as_deref() == Some("on"))
}

#[tauri::command]
pub fn set_kill_switch(active: bool, db: State<Db>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    set_setting(&conn, "kill_switch", if active { "on" } else { "off" })?;
    audit::record(
        &conn,
        "user",
        if active { "kill_switch_on" } else { "kill_switch_off" },
        RiskTier::ConfirmHighRisk,
        audit::Decision::Executed,
        None,
        None,
    )
}

#[tauri::command]
pub fn classify_action(domain: String) -> String {
    policy::classify(&domain).as_str().to_string()
}

/// Send one turn to Amin's Agent Core. Checks the kill switch first, appends
/// the user turn to the session's short-term conversation memory, calls the
/// Anthropic API with the Keychain-stored key, appends the reply back into
/// memory on success, and always audits the outcome — success or failure —
/// so the log reflects every real call, not just the ones that went well.
#[tauri::command]
pub async fn send_agent_message(
    message: String,
    db: State<'_, Db>,
    conversation: State<'_, agent::Conversation>,
) -> Result<String, String> {
    let halted = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        get_setting(&conn, "kill_switch").as_deref() == Some("on")
    };
    if halted {
        return Err("Amin is halted — turn the kill switch off to resume.".to_string());
    }

    let api_key = secrets::get_secret(ANTHROPIC_KEY_NAME)
        .map_err(|_| "No Anthropic API key configured yet — add one above first.".to_string())?;

    let history = {
        let mut turns = conversation.0.lock().map_err(|e| e.to_string())?;
        turns.push(agent::ChatMessage {
            role: "user".to_string(),
            content: message,
        });
        agent::trim_history(&mut turns);
        turns.clone()
    };

    let result = agent::send_message(&api_key, &history).await;

    if let Ok(reply) = &result {
        let mut turns = conversation.0.lock().map_err(|e| e.to_string())?;
        turns.push(agent::ChatMessage {
            role: "assistant".to_string(),
            content: reply.clone(),
        });
        agent::trim_history(&mut turns);
    }

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    match &result {
        Ok(_) => {
            let _ = audit::record(
                &conn,
                "amin",
                "agent_message",
                RiskTier::Auto,
                audit::Decision::Executed,
                None,
                None,
            );
        }
        Err(e) => {
            let _ = audit::record(
                &conn,
                "amin",
                "agent_message",
                RiskTier::Auto,
                audit::Decision::Blocked,
                Some(e),
                None,
            );
        }
    }

    result
}

/// Reset the session's short-term conversation memory (a "New conversation"
/// action). Does not touch the audit log itself — nothing risky happened.
#[tauri::command]
pub fn clear_agent_conversation(conversation: State<'_, agent::Conversation>) -> Result<(), String> {
    let mut turns = conversation.0.lock().map_err(|e| e.to_string())?;
    turns.clear();
    Ok(())
}

#[derive(serde::Serialize)]
pub struct AuditEntry {
    pub id: String,
    pub ts: String,
    pub actor: String,
    pub action: String,
    pub risk_tier: String,
    pub decision: String,
    pub details: Option<String>,
    pub evidence: Option<String>,
}

#[tauri::command]
pub fn list_audit_log(limit: i64, db: State<Db>) -> Result<Vec<AuditEntry>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, ts, actor, action, risk_tier, decision, details, evidence
             FROM audit_log ORDER BY ts DESC LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([limit], |row| {
            Ok(AuditEntry {
                id: row.get(0)?,
                ts: row.get(1)?,
                actor: row.get(2)?,
                action: row.get(3)?,
                risk_tier: row.get(4)?,
                decision: row.get(5)?,
                details: row.get(6)?,
                evidence: row.get(7)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Start push-to-talk listening from the UI (a mic button), independent of
/// the global keyboard shortcut in lib.rs — both call the same
/// `voice::start_listening`.
#[tauri::command]
pub fn start_voice_capture(app: AppHandle, session: State<VoiceSession>) -> Result<(), String> {
    voice::start_listening(app, session)
}

#[tauri::command]
pub fn stop_voice_capture(session: State<VoiceSession>) -> Result<(), String> {
    voice::stop_listening(session)
}

/// Create a task manually (from the Tasks panel's own form).
#[tauri::command]
pub fn create_task(title: String, db: State<Db>) -> Result<Task, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let task = tasks::create(&conn, &title, "manual")?;
    let _ = audit::record(
        &conn,
        "user",
        "create_task",
        policy::classify("create_task"),
        audit::Decision::Executed,
        Some(&task.title),
        None,
    );
    Ok(task)
}

/// Quick Capture: one text box, no status/source picking — for jotting
/// something down fast (typed or, once voice is wired up, spoken) without
/// interrupting whatever else Mona is doing.
#[tauri::command]
pub fn quick_capture(text: String, db: State<Db>) -> Result<Task, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let task = tasks::create(&conn, &text, "quick_capture")?;
    let _ = audit::record(
        &conn,
        "user",
        "quick_capture",
        policy::classify("quick_capture"),
        audit::Decision::Executed,
        Some(&task.title),
        None,
    );
    Ok(task)
}

#[tauri::command]
pub fn list_tasks(status: Option<String>, db: State<Db>) -> Result<Vec<Task>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    tasks::list(&conn, status.as_deref())
}

#[tauri::command]
pub fn set_task_status(id: String, status: String, db: State<Db>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    tasks::set_status(&conn, &id, &status)?;
    let _ = audit::record(
        &conn,
        "user",
        "set_task_status",
        policy::classify("set_task_status"),
        audit::Decision::Executed,
        Some(&format!("{id} -> {status}")),
        None,
    );
    Ok(())
}

/// File access is confined to one dedicated folder (`~/Documents/Amin`) —
/// see files.rs for the containment check every one of these goes through.
/// Listing/reading is Auto; writing is TrustedDelegation; deleting matches
/// policy::classify's "delete" keyword and comes back ConfirmHighRisk.
#[tauri::command]
pub fn list_workspace_files(app: AppHandle) -> Result<Vec<WorkspaceEntry>, String> {
    files::list(&app)
}

#[tauri::command]
pub fn read_workspace_file(app: AppHandle, path: String) -> Result<String, String> {
    files::read(&app, &path)
}

#[tauri::command]
pub fn write_workspace_file(app: AppHandle, path: String, contents: String, db: State<Db>) -> Result<(), String> {
    let result = files::write(&app, &path, &contents);
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let _ = audit::record(
        &conn,
        "user",
        "write_workspace_file",
        RiskTier::TrustedDelegation,
        if result.is_ok() {
            audit::Decision::Executed
        } else {
            audit::Decision::Blocked
        },
        Some(&path),
        None,
    );
    result
}

#[tauri::command]
pub fn delete_workspace_file(app: AppHandle, path: String, db: State<Db>) -> Result<(), String> {
    let result = files::delete(&app, &path);
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let _ = audit::record(
        &conn,
        "user",
        "delete_workspace_file",
        policy::classify("delete_workspace_file"),
        if result.is_ok() {
            audit::Decision::Executed
        } else {
            audit::Decision::Blocked
        },
        Some(&path),
        None,
    );
    result
}
