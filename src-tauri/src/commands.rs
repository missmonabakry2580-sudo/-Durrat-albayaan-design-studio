use tauri::State;

use crate::db::Db;
use crate::policy::{self, AutonomyLevel, RiskTier};
use crate::{agent, audit, secrets};

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

/// Send one turn to Amin's Agent Core. Checks the kill switch first, then
/// calls the Anthropic API with the Keychain-stored key, and always audits
/// the outcome — success or failure — so the log reflects every real call,
/// not just the ones that went well.
#[tauri::command]
pub async fn send_agent_message(message: String, db: State<'_, Db>) -> Result<String, String> {
    let halted = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        get_setting(&conn, "kill_switch").as_deref() == Some("on")
    };
    if halted {
        return Err("Amin is halted — turn the kill switch off to resume.".to_string());
    }

    let api_key = secrets::get_secret(ANTHROPIC_KEY_NAME)
        .map_err(|_| "No Anthropic API key configured yet — add one above first.".to_string())?;

    let result = agent::send_message(&api_key, &message).await;

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
