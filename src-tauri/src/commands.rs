use tauri::{AppHandle, State};

use crate::brief::DeltaBrief;
use crate::confirmation::{self, PendingAction, PendingConfirmation};
use crate::db::Db;
use crate::files::WorkspaceEntry;
use crate::followups::FollowUp;
use crate::policy::{self, AutonomyLevel, RiskTier};
use crate::tasks::Task;
use crate::voice::VoiceSession;
use crate::{agent, audit, brief, browser, files, followups, notify, tasks, tools, voice};

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

// The Anthropic key was originally stored in the OS Keychain via
// secrets.rs (see that module for the generic wrapper, still used
// elsewhere). On at least one real Mac, saving it there reported success
// every time yet reading it back — moments later, same running session —
// reliably came back "No matching entry found in secure storage". That's
// not a permissions prompt or an ambiguous-item issue (checked the
// keyring crate's own macOS backend source for both); it looks like a
// genuine environment-specific Keychain fault this app can't work around,
// and there's no second Mac available to root-cause it further. Mona
// chose, knowingly, to store it in the local settings table instead (the
// same table already reliably holding autonomy_level/kill_switch) so Amin
// is actually usable now, rather than staying blocked on an unresolved
// OS-level issue. Trade-off, stated plainly: this is local-disk storage
// with the same protection as any other file only her logged-in macOS
// account can read — not Keychain's at-rest encryption. See
// docs/SECURITY.md.
#[tauri::command]
pub fn has_api_key(db: State<Db>) -> Result<bool, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(get_setting(&conn, ANTHROPIC_KEY_NAME)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false))
}

#[tauri::command]
pub fn save_api_key(key: String, db: State<Db>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    set_setting(&conn, ANTHROPIC_KEY_NAME, key.trim())?;
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
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM settings WHERE key = ?1", [ANTHROPIC_KEY_NAME])
        .map_err(|e| e.to_string())?;
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

/// Send one turn to Amin's Agent Core. This is the one place Mona's
/// non-negotiable instruction — "any step Amin wants to take waits for my
/// explicit word before it runs" — is actually enforced at runtime, not
/// just described in the system prompt:
///
/// 1. If a ConfirmHighRisk tool call is already pending from the previous
///    turn, this message IS Mona's answer to it — `resolve_pending_action`
///    reads it as approve/deny/unclear and never starts a new turn until
///    that's settled.
/// 2. Otherwise, Claude gets the real tool registry (`tools.rs`). If it
///    calls a tool classified Auto/TrustedDelegation, that tool runs right
///    away (logged either way), then Claude gets one follow-up call to
///    narrate the result in plain language. If it calls a ConfirmHighRisk
///    tool, nothing runs yet — the call is stored as pending and this
///    returns a confirmation request instead.
///
/// Every branch audits its outcome, including "proposed and waiting" —
/// see `audit::Decision::Proposed`.
#[tauri::command]
pub async fn send_agent_message(
    message: String,
    app: AppHandle,
    db: State<'_, Db>,
    conversation: State<'_, agent::Conversation>,
    pending: State<'_, PendingConfirmation>,
) -> Result<String, String> {
    let halted = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        get_setting(&conn, "kill_switch").as_deref() == Some("on")
    };
    if halted {
        return Err("Amin is halted — turn the kill switch off to resume.".to_string());
    }

    // Read from the local settings table, not the OS Keychain — see
    // has_api_key's doc comment for why (a reproducible Keychain read
    // failure on Mona's real Mac that a real Keychain error message
    // confirmed was "No matching entry found in secure storage" despite
    // save_api_key reporting success every time).
    let api_key = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        get_setting(&conn, ANTHROPIC_KEY_NAME).filter(|v| !v.trim().is_empty())
    }
    .ok_or_else(|| "لسه محتاجة تحطي مفتاح الاتصال بأنثروبيك — من قسم الأمان والاستقلالية.".to_string())?;

    let existing_pending = {
        let guard = pending.0.lock().map_err(|e| e.to_string())?;
        guard.clone()
    };

    if let Some(action) = existing_pending {
        return resolve_pending_action(&app, &db, &conversation, &pending, &api_key, action, &message).await;
    }

    let tool_defs = tools::tool_definitions();

    let history = {
        let mut turns = conversation.0.lock().map_err(|e| e.to_string())?;
        turns.push(agent::ChatMessage::user_text(&message));
        agent::trim_history(&mut turns);
        turns.clone()
    };

    let response = match agent::send_message(&api_key, &history, &tool_defs).await {
        Ok(r) => r,
        Err(e) => {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let _ = audit::record(
                &conn,
                "amin",
                "agent_message",
                RiskTier::Auto,
                audit::Decision::Blocked,
                Some(&e),
                None,
            );
            return Err(e);
        }
    };

    if let Some(refusal) = response.refusal_error() {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let _ = audit::record(
            &conn,
            "amin",
            "agent_message",
            RiskTier::Auto,
            audit::Decision::Blocked,
            Some(&refusal),
            None,
        );
        return Err(refusal);
    }

    {
        let mut turns = conversation.0.lock().map_err(|e| e.to_string())?;
        turns.push(agent::ChatMessage::assistant_content(response.as_assistant_content()));
        agent::trim_history(&mut turns);
    }

    let claude_text = response.text();

    let Some((tool_id, tool_name, tool_input)) = response.first_tool_use() else {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let _ = audit::record(
            &conn,
            "amin",
            "agent_message",
            RiskTier::Auto,
            audit::Decision::Executed,
            None,
            None,
        );
        return if claude_text.is_empty() {
            Err("Amin returned an empty response".to_string())
        } else {
            Ok(claude_text)
        };
    };

    let tool_id = tool_id.to_string();
    let tool_name = tool_name.to_string();
    let tool_input = tool_input.clone();
    let risk = tools::risk_for(&tool_name);
    let description = tools::describe(&tool_name, &tool_input);

    if risk == RiskTier::ConfirmHighRisk {
        {
            let mut guard = pending.0.lock().map_err(|e| e.to_string())?;
            *guard = Some(PendingAction {
                tool_use_id: tool_id,
                name: tool_name.clone(),
                input: tool_input,
            });
        }
        {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let _ = audit::record(
                &conn,
                "amin",
                &tool_name,
                risk,
                audit::Decision::Proposed,
                Some(&description),
                None,
            );
        }

        let prefix = if claude_text.is_empty() {
            String::new()
        } else {
            format!("{claude_text}\n\n")
        };
        return Ok(format!(
            "{prefix}⏸️ {description}\n\nمستني كلمتك يا مُنى — قولي \"موافقة\" أو \"نفذ\" عشان أكمل، أو \"إلغاء\" لو غيرتِ رأيك."
        ));
    }

    // Auto / TrustedDelegation: safe to run immediately — still logged,
    // and still narrated back rather than executed silently.
    let exec_result = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let result = tools::execute(&app, &conn, &tool_name, &tool_input);
        let (details, decision) = match &result {
            Ok(_) => (description.clone(), audit::Decision::Executed),
            Err(e) => (format!("{description} — خطأ: {e}"), audit::Decision::Blocked),
        };
        let _ = audit::record(&conn, "amin", &tool_name, risk, decision, Some(&details), None);
        result
    };

    let result_text = match &exec_result {
        Ok(v) => v.to_string(),
        Err(e) => format!("Error: {e}"),
    };

    {
        let mut turns = conversation.0.lock().map_err(|e| e.to_string())?;
        turns.push(agent::ChatMessage::tool_result(&tool_id, &result_text, None));
        agent::trim_history(&mut turns);
    }

    let fallback = if exec_result.is_ok() {
        format!("تم: {description}")
    } else {
        format!("حصل خطأ أثناء: {description}")
    };
    narrate(&api_key, &conversation, &tool_defs, fallback).await
}

/// Mona's reply to an already-pending ConfirmHighRisk tool call. Reads her
/// message with `confirmation::interpret` rather than assuming any
/// non-empty reply means yes — an unclear message re-states what's pending
/// and waits again instead of guessing.
async fn resolve_pending_action(
    app: &AppHandle,
    db: &State<'_, Db>,
    conversation: &State<'_, agent::Conversation>,
    pending: &State<'_, PendingConfirmation>,
    api_key: &str,
    action: PendingAction,
    message: &str,
) -> Result<String, String> {
    let tool_defs = tools::tool_definitions();
    let description = tools::describe(&action.name, &action.input);
    let risk = tools::risk_for(&action.name);

    match confirmation::interpret(message) {
        confirmation::Reply::Unclear => Ok(format!(
            "لسه مستني تأكيدك على:\n⏸️ {description}\n\nقولي \"موافقة\" أو \"نفذ\" عشان أكمل، أو \"إلغاء\" لو غيرتِ رأيك."
        )),
        confirmation::Reply::Deny => {
            {
                let mut guard = pending.0.lock().map_err(|e| e.to_string())?;
                *guard = None;
            }
            {
                let conn = db.0.lock().map_err(|e| e.to_string())?;
                let _ = audit::record(
                    &conn,
                    "user",
                    &action.name,
                    risk,
                    audit::Decision::Declined,
                    Some(&description),
                    None,
                );
            }
            {
                let mut turns = conversation.0.lock().map_err(|e| e.to_string())?;
                turns.push(agent::ChatMessage::tool_result(
                    &action.tool_use_id,
                    "Mona declined this action. Do not perform it.",
                    Some(message),
                ));
                agent::trim_history(&mut turns);
            }
            narrate(api_key, conversation, &tool_defs, format!("تمام، اتلغى: {description}")).await
        }
        confirmation::Reply::Approve => {
            {
                let mut guard = pending.0.lock().map_err(|e| e.to_string())?;
                *guard = None;
            }
            let exec_result = {
                let conn = db.0.lock().map_err(|e| e.to_string())?;
                let result = tools::execute(app, &conn, &action.name, &action.input);
                let (details, decision) = match &result {
                    Ok(_) => (description.clone(), audit::Decision::Executed),
                    Err(e) => (format!("{description} — خطأ: {e}"), audit::Decision::Blocked),
                };
                let _ = audit::record(&conn, "user", &action.name, risk, decision, Some(&details), None);
                result
            };
            let result_text = match &exec_result {
                Ok(v) => v.to_string(),
                Err(e) => format!("Error: {e}"),
            };
            {
                let mut turns = conversation.0.lock().map_err(|e| e.to_string())?;
                turns.push(agent::ChatMessage::tool_result(
                    &action.tool_use_id,
                    &result_text,
                    Some(message),
                ));
                agent::trim_history(&mut turns);
            }
            let fallback = if exec_result.is_ok() {
                format!("تم: {description}")
            } else {
                format!("حصل خطأ أثناء: {description}")
            };
            narrate(api_key, conversation, &tool_defs, fallback).await
        }
    }
}

/// One follow-up call to Claude after a tool has run (or been declined), so
/// Mona gets a natural sentence about the outcome instead of raw tool JSON.
/// `fallback_text` covers the (rare) case this follow-up call itself fails
/// or comes back empty — the action already ran and is already audited by
/// this point, so a narration hiccup is reported gently, not as a failure
/// of the whole turn.
async fn narrate(
    api_key: &str,
    conversation: &State<'_, agent::Conversation>,
    tool_defs: &[serde_json::Value],
    fallback_text: String,
) -> Result<String, String> {
    let history = {
        let turns = conversation.0.lock().map_err(|e| e.to_string())?;
        turns.clone()
    };

    let response = match agent::send_message(api_key, &history, tool_defs).await {
        Ok(r) => r,
        Err(_) => return Ok(fallback_text),
    };

    {
        let mut turns = conversation.0.lock().map_err(|e| e.to_string())?;
        turns.push(agent::ChatMessage::assistant_content(response.as_assistant_content()));
        agent::trim_history(&mut turns);
    }

    let text = response.text();
    Ok(if text.is_empty() { fallback_text } else { text })
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

/// Opens a URL in Amin's own isolated browser window — never Mona's
/// personal browser/profile. See browser.rs and docs/SECURITY.md §4. This
/// is TrustedDelegation, not Auto: showing a page is low-risk, but it's
/// still Amin reaching an external destination, worth a line in the audit
/// log.
#[tauri::command]
pub fn open_browser_url(app: AppHandle, url: String, db: State<Db>) -> Result<(), String> {
    let result = browser::open_url(&app, &url);
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let _ = audit::record(
        &conn,
        "user",
        "open_browser_url",
        RiskTier::TrustedDelegation,
        if result.is_ok() {
            audit::Decision::Executed
        } else {
            audit::Decision::Blocked
        },
        Some(&url),
        None,
    );
    result
}

pub(crate) fn task_title(conn: &rusqlite::Connection, task_id: &str) -> String {
    conn.query_row(
        "SELECT title FROM tasks WHERE id = ?1",
        rusqlite::params![task_id],
        |row| row.get(0),
    )
    .unwrap_or_else(|_| task_id.to_string())
}

/// Follow-up Engine (local only for now — see followups.rs's module doc
/// for why "sent" doesn't mean an email went out yet — except for the one
/// real local channel, a native OS notification, which this and
/// escalate_follow_up below both use).
#[tauri::command]
pub fn create_follow_up(app: AppHandle, task_id: String, due_at: String, db: State<Db>) -> Result<FollowUp, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let follow_up = followups::create(&conn, &task_id, &due_at)?;
    let _ = audit::record(
        &conn,
        "user",
        "create_follow_up",
        RiskTier::Auto,
        audit::Decision::Executed,
        Some(&format!("task {task_id} due {due_at}")),
        None,
    );
    // Parsed comparison, not a raw string one — chrono's own to_rfc3339()
    // uses a "+00:00" suffix while due_at may arrive as "...Z" (e.g. from
    // JS's Date.toISOString()); those represent the same instants but
    // don't compare correctly as strings.
    let already_due = chrono::DateTime::parse_from_rfc3339(&follow_up.due_at)
        .map(|due| due <= chrono::Utc::now())
        .unwrap_or(false);
    if already_due {
        notify::send(&app, "أمين — متابعة", &task_title(&conn, &task_id));
    }
    Ok(follow_up)
}

#[tauri::command]
pub fn list_follow_ups(task_id: Option<String>, db: State<Db>) -> Result<Vec<FollowUp>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    followups::list(&conn, task_id.as_deref())
}

#[tauri::command]
pub fn list_due_follow_ups(db: State<Db>) -> Result<Vec<FollowUp>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    followups::list_due(&conn, chrono::Utc::now())
}

#[tauri::command]
pub fn escalate_follow_up(app: AppHandle, id: String, db: State<Db>) -> Result<FollowUp, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let follow_up = followups::escalate(&conn, &id)?;
    let _ = audit::record(
        &conn,
        "amin",
        "escalate_follow_up",
        RiskTier::TrustedDelegation,
        audit::Decision::Executed,
        Some(&format!("{id} -> {}", follow_up.escalation_stage)),
        None,
    );
    let title = task_title(&conn, &follow_up.task_id);
    let stage_label = match follow_up.escalation_stage.as_str() {
        "firm" => "تذكير",
        "escalate_to_user" => "محتاجة انتباهك",
        _ => "متابعة",
    };
    notify::send(&app, &format!("أمين — {stage_label}"), &title);
    Ok(follow_up)
}

#[tauri::command]
pub fn set_follow_up_status(id: String, status: String, db: State<Db>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    followups::set_status(&conn, &id, &status)?;
    let _ = audit::record(
        &conn,
        "user",
        "set_follow_up_status",
        RiskTier::Auto,
        audit::Decision::Executed,
        Some(&format!("{id} -> {status}")),
        None,
    );
    Ok(())
}

/// Local Delta Brief (Phase 3 slice that needs no Gmail/Calendar) — a
/// "what changed" summary of Amin's own local activity. See brief.rs.
#[tauri::command]
pub fn generate_delta_brief(db: State<Db>) -> Result<DeltaBrief, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    brief::generate(&conn)
}
