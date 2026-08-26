use rusqlite::Connection;
use serde_json::{json, Value};
use tauri::{AppHandle, Runtime};

use crate::commands::task_title;
use crate::policy::RiskTier;
use crate::{browser, files, followups, memory, notify, tasks};

/// Amin's real tool registry for the Anthropic API's tool-use feature.
/// Three things live here, deliberately kept together rather than spread
/// across the modules that back each tool: the JSON schema Claude sees
/// (`tool_definitions`), the risk tier that decides whether it runs
/// immediately or waits for Mona's confirmation (`risk_for`), and the
/// dispatcher that actually calls into `tasks`/`files`/`browser`/
/// `followups` (`execute`). Keeping the risk decision here — rather than
/// relying on `policy::classify`'s generic keyword match — means every new
/// tool gets an explicit, reviewed risk tier instead of an inferred one.
///
/// Only the first tool call per turn is handled (see agent.rs's
/// `first_tool_use`) — parallel tool use is out of scope for this pass.
pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "create_task",
            "description": "Create a new task in Amin's local task list. Only title is required — fill in the rest when Mona's own words make it clear (a deadline she mentioned, what the concrete next step is, whether another task blocks this one), not by guessing for the sake of completeness.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "priority": { "type": "string", "enum": ["low", "medium", "high"] },
                    "deadline": { "type": "string", "description": "RFC3339 timestamp, if Mona gave or implied one" },
                    "project": { "type": "string", "description": "free-form grouping label, e.g. \"تسجيل أحمد\"" },
                    "next_action": { "type": "string", "description": "the concrete next step, not just a restatement of the title" },
                    "approval_required": { "type": "boolean", "description": "whether finishing this task itself needs Mona's approval before it counts as done" },
                    "dependencies": { "type": "array", "items": { "type": "string" }, "description": "ids of other tasks this one is blocked on" }
                },
                "required": ["title"]
            }
        }),
        json!({
            "name": "quick_capture",
            "description": "Jot down a quick note as a task, without deciding its details yet.",
            "input_schema": {
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"]
            }
        }),
        json!({
            "name": "list_tasks",
            "description": "List Mona's tasks, optionally filtered by status.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "status": { "type": "string", "enum": ["open", "in_progress", "done", "cancelled"] }
                }
            }
        }),
        json!({
            "name": "set_task_status",
            "description": "Change a task's status.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "status": { "type": "string", "enum": ["open", "in_progress", "done", "cancelled"] }
                },
                "required": ["id", "status"]
            }
        }),
        json!({
            "name": "list_workspace_files",
            "description": "List files in Amin's dedicated workspace folder.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "read_workspace_file",
            "description": "Read a file from Amin's workspace folder.",
            "input_schema": {
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }
        }),
        json!({
            "name": "write_workspace_file",
            "description": "Create or overwrite a file in Amin's workspace folder. Requires Mona's explicit confirmation before it runs.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "contents": { "type": "string" }
                },
                "required": ["path", "contents"]
            }
        }),
        json!({
            "name": "delete_workspace_file",
            "description": "Delete a file from Amin's workspace folder. Requires Mona's explicit confirmation before it runs.",
            "input_schema": {
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }
        }),
        json!({
            "name": "open_browser_url",
            "description": "Open a URL in Amin's own isolated browser window (never Mona's personal browser/profile). Requires Mona's explicit confirmation before it runs.",
            "input_schema": {
                "type": "object",
                "properties": { "url": { "type": "string" } },
                "required": ["url"]
            }
        }),
        json!({
            "name": "create_follow_up",
            "description": "Schedule a follow-up reminder for a task, due at a given RFC3339 timestamp.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string" },
                    "due_at": { "type": "string", "description": "RFC3339 timestamp, e.g. 2026-01-01T09:00:00Z" }
                },
                "required": ["task_id", "due_at"]
            }
        }),
        json!({
            "name": "list_follow_ups",
            "description": "List follow-up reminders, optionally for one task.",
            "input_schema": {
                "type": "object",
                "properties": { "task_id": { "type": "string" } }
            }
        }),
        json!({
            "name": "list_due_follow_ups",
            "description": "List follow-up reminders that are pending and already due.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "escalate_follow_up",
            "description": "Advance a follow-up to its next escalation stage and notify Mona.",
            "input_schema": {
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }
        }),
        json!({
            "name": "set_follow_up_status",
            "description": "Change a follow-up's status.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "status": { "type": "string", "enum": ["pending", "sent", "resolved", "cancelled"] }
                },
                "required": ["id", "status"]
            }
        }),
        json!({
            "name": "remember_fact",
            "description": "Remember a fact about Mona, her people, projects, routines, or a decision made — for recall in future conversations, not just this one. Remembering the same category+key again updates it (corrects the fact) instead of creating a duplicate. Use when Mona says something worth carrying forward, e.g. \"افتكر إن...\" — not for every sentence she says.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "category": { "type": "string", "description": "e.g. preference, person, project, routine, decision" },
                    "key": { "type": "string", "description": "short label for this fact, e.g. \"اسم ابن منى\"" },
                    "value": { "type": "string" }
                },
                "required": ["category", "key", "value"]
            }
        }),
        json!({
            "name": "search_memory",
            "description": "Search remembered facts by keyword, across both their label and their value. Use before answering anything that depends on something Mona told Amin to remember previously.",
            "input_schema": {
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"]
            }
        }),
        json!({
            "name": "forget_fact",
            "description": "Permanently forget a remembered fact by its id (get the id from search_memory first). Use when Mona explicitly says to forget something, e.g. \"انسَ المعلومة دي\".",
            "input_schema": {
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }
        }),
    ]
}

/// The risk tier that decides whether a tool call runs immediately (Auto,
/// TrustedDelegation) or must wait for Mona's explicit confirmation word
/// (ConfirmHighRisk) — see confirmation.rs.
///
/// File tools are *all* ConfirmHighRisk, including plain reads and listing
/// — not just writes/deletes. This is deliberate: since files.rs's scope
/// was broadened to Mona's whole home directory (her own explicit "كل
/// الملفات" request), a read is no longer confined to a small dedicated
/// folder she put things in for Amin — it can reach anything on her
/// machine, and its content then leaves her machine in a tool_result sent
/// to the Anthropic API. That is exactly the kind of "step" her own
/// instruction says must wait for her word, not just the destructive ones.
///
/// Task/follow-up tools stay local-only bookkeeping in Amin's own database
/// — nothing leaves the machine and nothing outside that database is
/// touched — so Auto/TrustedDelegation still fits them.
///
/// Per Mona's own instruction to treat cybersecurity as the top priority,
/// an unrecognized tool name defaults to ConfirmHighRisk rather than Auto:
/// a tool Claude asks for that isn't in this registry should never run
/// silently.
pub fn risk_for(name: &str) -> RiskTier {
    match name {
        "create_task" | "quick_capture" | "list_tasks" | "set_task_status"
        | "list_follow_ups" | "list_due_follow_ups" | "set_follow_up_status"
        | "create_follow_up" | "remember_fact" | "search_memory" | "forget_fact" => RiskTier::Auto,
        "escalate_follow_up" => RiskTier::TrustedDelegation,
        "list_workspace_files"
        | "read_workspace_file"
        | "write_workspace_file"
        | "delete_workspace_file"
        | "open_browser_url" => RiskTier::ConfirmHighRisk,
        _ => RiskTier::ConfirmHighRisk,
    }
}

/// A short, human-readable Arabic description of a tool call, used to build
/// the confirmation prompt Mona actually reads before she says "موافقة" /
/// "نفذ". Never used to decide whether to execute — only to describe.
pub fn describe(name: &str, input: &Value) -> String {
    let s = |k: &str| input.get(k).and_then(|v| v.as_str()).unwrap_or("");
    match name {
        "create_task" => format!("إضافة مهمة: \"{}\"", s("title")),
        "quick_capture" => format!("تدوين سريع: \"{}\"", s("text")),
        "list_tasks" => "عرض قائمة المهام".to_string(),
        "set_task_status" => format!("تغيير حالة المهمة {} إلى {}", s("id"), s("status")),
        "list_workspace_files" => "عرض ملفات مساحة أمين".to_string(),
        "read_workspace_file" => format!("قراءة الملف: {}", s("path")),
        "write_workspace_file" => format!("كتابة/تعديل الملف: {}", s("path")),
        "delete_workspace_file" => format!("حذف الملف: {}", s("path")),
        "open_browser_url" => format!("فتح هذا الرابط في متصفح أمين المعزول: {}", s("url")),
        "create_follow_up" => format!("جدولة متابعة للمهمة {} في {}", s("task_id"), s("due_at")),
        "list_follow_ups" => "عرض قائمة المتابعات".to_string(),
        "list_due_follow_ups" => "عرض المتابعات المستحقة".to_string(),
        "escalate_follow_up" => format!("تصعيد المتابعة رقم {}", s("id")),
        "set_follow_up_status" => format!("تغيير حالة المتابعة {} إلى {}", s("id"), s("status")),
        "remember_fact" => format!("تذكّر: {} = {}", s("key"), s("value")),
        "search_memory" => format!("البحث في الذاكرة عن: {}", s("query")),
        "forget_fact" => format!("نسيان المعلومة رقم {}", s("id")),
        other => format!("تنفيذ إجراء غير معروف: {other} — يُنصح بعدم الموافقة"),
    }
}

fn required_str(input: &Value, key: &str, tool: &str) -> Result<String, String> {
    input
        .get(key)
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
        .ok_or_else(|| format!("tool '{tool}' is missing required field '{key}'"))
}

fn optional_str(input: &Value, key: &str) -> Option<String> {
    input.get(key).and_then(|v| v.as_str()).map(|v| v.to_string())
}

/// Actually run a tool call. Callers decide *when* this is allowed to run
/// (immediately for Auto/TrustedDelegation, only after Mona's confirmation
/// for ConfirmHighRisk) — this function has no opinion on risk, it just
/// executes.
pub fn execute<R: Runtime>(
    app: &AppHandle<R>,
    conn: &Connection,
    name: &str,
    input: &Value,
) -> Result<Value, String> {
    match name {
        "create_task" => {
            let details = tasks::NewTaskDetails {
                priority: optional_str(input, "priority"),
                deadline: optional_str(input, "deadline"),
                project: optional_str(input, "project"),
                next_action: optional_str(input, "next_action"),
                approval_required: input.get("approval_required").and_then(|v| v.as_bool()).unwrap_or(false),
                dependencies: input
                    .get("dependencies")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default(),
            };
            let task = tasks::create_with_details(conn, &required_str(input, "title", name)?, "amin", details)?;
            serde_json::to_value(task).map_err(|e| e.to_string())
        }
        "quick_capture" => {
            let task = tasks::create(conn, &required_str(input, "text", name)?, "amin_quick_capture")?;
            serde_json::to_value(task).map_err(|e| e.to_string())
        }
        "list_tasks" => {
            let list = tasks::list(conn, optional_str(input, "status").as_deref())?;
            serde_json::to_value(list).map_err(|e| e.to_string())
        }
        "set_task_status" => {
            tasks::set_status(
                conn,
                &required_str(input, "id", name)?,
                &required_str(input, "status", name)?,
            )?;
            Ok(json!({ "ok": true }))
        }
        "list_workspace_files" => {
            let entries = files::list(app)?;
            serde_json::to_value(entries).map_err(|e| e.to_string())
        }
        "read_workspace_file" => {
            let contents = files::read(app, &required_str(input, "path", name)?)?;
            Ok(json!({ "contents": contents }))
        }
        "write_workspace_file" => {
            files::write(
                app,
                &required_str(input, "path", name)?,
                &required_str(input, "contents", name)?,
            )?;
            Ok(json!({ "ok": true }))
        }
        "delete_workspace_file" => {
            files::delete(app, &required_str(input, "path", name)?)?;
            Ok(json!({ "ok": true }))
        }
        "open_browser_url" => {
            browser::open_url(app, &required_str(input, "url", name)?)?;
            Ok(json!({ "ok": true }))
        }
        "create_follow_up" => {
            let task_id = required_str(input, "task_id", name)?;
            let due_at = required_str(input, "due_at", name)?;
            let follow_up = followups::create(conn, &task_id, &due_at)?;
            let already_due = chrono::DateTime::parse_from_rfc3339(&follow_up.due_at)
                .map(|due| due <= chrono::Utc::now())
                .unwrap_or(false);
            if already_due {
                notify::send(app, "أمين — متابعة", &task_title(conn, &task_id));
            }
            serde_json::to_value(follow_up).map_err(|e| e.to_string())
        }
        "list_follow_ups" => {
            let list = followups::list(conn, optional_str(input, "task_id").as_deref())?;
            serde_json::to_value(list).map_err(|e| e.to_string())
        }
        "list_due_follow_ups" => {
            let list = followups::list_due(conn, chrono::Utc::now())?;
            serde_json::to_value(list).map_err(|e| e.to_string())
        }
        "escalate_follow_up" => {
            let follow_up = followups::escalate(conn, &required_str(input, "id", name)?)?;
            let title = task_title(conn, &follow_up.task_id);
            let stage_label = match follow_up.escalation_stage.as_str() {
                "firm" => "تذكير",
                "escalate_to_user" => "محتاجة انتباهك",
                _ => "متابعة",
            };
            notify::send(app, &format!("أمين — {stage_label}"), &title);
            serde_json::to_value(follow_up).map_err(|e| e.to_string())
        }
        "set_follow_up_status" => {
            followups::set_status(
                conn,
                &required_str(input, "id", name)?,
                &required_str(input, "status", name)?,
            )?;
            Ok(json!({ "ok": true }))
        }
        "remember_fact" => {
            let fact = memory::remember(
                conn,
                &required_str(input, "category", name)?,
                &required_str(input, "key", name)?,
                &required_str(input, "value", name)?,
            )?;
            serde_json::to_value(fact).map_err(|e| e.to_string())
        }
        "search_memory" => {
            let results = memory::search(conn, &required_str(input, "query", name)?)?;
            serde_json::to_value(results).map_err(|e| e.to_string())
        }
        "forget_fact" => {
            memory::forget(conn, &required_str(input, "id", name)?)?;
            Ok(json!({ "ok": true }))
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../schema.sql")).unwrap();
        conn
    }

    #[test]
    fn every_defined_tool_has_an_explicit_risk_tier_and_description() {
        for def in tool_definitions() {
            let name = def["name"].as_str().unwrap();
            // Just exercising both functions for every registered tool —
            // risk_for must not panic, and describe must produce non-empty
            // text even with an empty input object.
            let _ = risk_for(name);
            let text = describe(name, &json!({}));
            assert!(!text.is_empty(), "describe({name}) returned empty text");
        }
    }

    #[test]
    fn every_file_and_browser_tool_requires_confirmation() {
        // Not just writes/deletes — files.rs's scope is Mona's whole home
        // directory now, so even listing/reading must wait for her word.
        assert_eq!(risk_for("list_workspace_files"), RiskTier::ConfirmHighRisk);
        assert_eq!(risk_for("read_workspace_file"), RiskTier::ConfirmHighRisk);
        assert_eq!(risk_for("write_workspace_file"), RiskTier::ConfirmHighRisk);
        assert_eq!(risk_for("delete_workspace_file"), RiskTier::ConfirmHighRisk);
        assert_eq!(risk_for("open_browser_url"), RiskTier::ConfirmHighRisk);
    }

    #[test]
    fn local_bookkeeping_tools_run_without_confirmation() {
        assert_eq!(risk_for("list_tasks"), RiskTier::Auto);
        assert_eq!(risk_for("create_task"), RiskTier::Auto);
    }

    #[test]
    fn an_unrecognized_tool_name_defaults_to_confirm_high_risk() {
        assert_eq!(risk_for("delete_all_customer_records"), RiskTier::ConfirmHighRisk);
    }

    #[test]
    fn execute_creates_a_task_end_to_end() {
        let conn = test_db();
        let app = tauri::test::mock_app();
        let result = execute(app.handle(), &conn, "create_task", &json!({ "title": "اختبار" })).unwrap();
        assert_eq!(result["title"], "اختبار");
        assert_eq!(result["status"], "open");
    }

    #[test]
    fn execute_rejects_a_missing_required_field() {
        let conn = test_db();
        let app = tauri::test::mock_app();
        let err = execute(app.handle(), &conn, "create_task", &json!({})).unwrap_err();
        assert!(err.contains("title"));
    }

    #[test]
    fn execute_rejects_an_unknown_tool_name() {
        let conn = test_db();
        let app = tauri::test::mock_app();
        let err = execute(app.handle(), &conn, "wire_transfer_money", &json!({})).unwrap_err();
        assert!(err.contains("unknown tool"));
    }
}
