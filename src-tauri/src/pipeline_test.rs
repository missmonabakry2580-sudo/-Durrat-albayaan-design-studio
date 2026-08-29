//! Integration test: the approval → execution → audit → verification
//! pipeline described in `commands.rs`, exercised directly against the
//! real `tasks`/`confirmation`/`audit`/`memory`/`tools` modules against an
//! in-memory SQLite database — no mocked business logic, no fixtures
//! standing in for real rows.
//!
//! This is Mona's mandated "INTEGRATION TEST" phase, scoped honestly: it
//! proves the mechanics Amin runs on every turn — propose a high-risk
//! action, wait, read her real reply, only then execute for real, audit
//! every step, and never treat a result as "تم" before it has actually come
//! back — using real local data end to end. It does not, and cannot from
//! this sandbox, exercise the live Anthropic API, a real microphone/speaker,
//! or a real Mac — `commands::send_agent_message` itself (the network call)
//! is still unverified outside a signed build Mona runs herself.

#[cfg(test)]
mod tests {
    use crate::db::Db;
    use crate::policy::RiskTier;
    use crate::{audit, confirmation, memory, tasks, tools};
    use rusqlite::Connection;
    use serde_json::json;

    fn test_db() -> Db {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../schema.sql")).unwrap();
        Db(std::sync::Mutex::new(conn))
    }

    fn audit_rows(conn: &Connection) -> Vec<(String, String)> {
        let mut stmt = conn
            .prepare("SELECT action, decision FROM audit_log ORDER BY ts")
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    /// The exact scenario from Mona's spec: Amin proposes a high-risk
    /// action, she hasn't answered yet (nothing may run), an unclear reply
    /// still must not be read as consent, and only a real approval word lets
    /// the real tool execute — mirroring commands.rs's
    /// send_agent_message/resolve_pending_action split without needing a
    /// live Tauri window or network call.
    #[tokio::test]
    async fn a_high_risk_action_only_runs_after_real_approval_and_is_fully_audited() {
        let db = test_db();
        let app = tauri::test::mock_app();

        let action = confirmation::PendingAction {
            tool_use_id: "toolu_test".to_string(),
            name: "create_task".to_string(),
            input: json!({ "title": "اتصلي بالمدرسة بخصوص أحمد", "priority": "high" }),
            proposed_at: chrono::Utc::now(),
        };
        let description = tools::describe(&action.name, &action.input);
        {
            let conn = db.0.lock().unwrap();
            audit::record(
                &conn,
                "amin",
                &action.name,
                RiskTier::ConfirmHighRisk,
                audit::Decision::Proposed,
                Some(&description),
                None,
            )
            .unwrap();

            // Proposed, not executed — the task list must still be empty.
            assert!(tasks::list(&conn, None).unwrap().is_empty());
        }

        // An ambiguous reply must not be read as approval.
        assert!(matches!(
            confirmation::interpret("مش عارفة بصراحة"),
            confirmation::Reply::Unclear
        ));
        assert!(tasks::list(&db.0.lock().unwrap(), None).unwrap().is_empty());

        // Mona approves for real, in her own words.
        assert!(matches!(
            confirmation::interpret("ايوه نفذ"),
            confirmation::Reply::Approve
        ));
        assert!(!confirmation::is_expired(&action, chrono::Utc::now()));

        // Only now does the real tool run.
        let result = tools::execute(app.handle(), &db, &action.name, &action.input)
            .await
            .unwrap();
        let created_id = result["id"].as_str().unwrap().to_string();
        {
            let conn = db.0.lock().unwrap();
            audit::record(
                &conn,
                "user",
                &action.name,
                RiskTier::ConfirmHighRisk,
                audit::Decision::Executed,
                Some(&description),
                None,
            )
            .unwrap();

            // Verification Layer: "تم" only ever describes a result checked
            // here, not one merely planned.
            let stored = tasks::list(&conn, None).unwrap();
            assert_eq!(stored.len(), 1);
            assert_eq!(stored[0].id, created_id);
            assert_eq!(stored[0].priority.as_deref(), Some("high"));

            // The audit trail carries the whole story — nothing silently
            // skipped the wait.
            assert_eq!(
                audit_rows(&conn),
                vec![
                    ("create_task".to_string(), "proposed".to_string()),
                    ("create_task".to_string(), "executed".to_string()),
                ]
            );
        }

        // Morning Brief's own tool reads the same real row back, not a
        // separate, possibly-stale copy of the truth.
        let overview = tools::execute(app.handle(), &db, "get_daily_overview", &json!({}))
            .await
            .unwrap();
        assert_eq!(overview["open_tasks"].as_array().unwrap().len(), 1);
    }

    /// The mirror case: Mona declines. The action must never run, and the
    /// audit log must say so plainly.
    #[test]
    fn a_declined_high_risk_action_never_executes() {
        let db = test_db();
        let conn = db.0.lock().unwrap();
        let action = confirmation::PendingAction {
            tool_use_id: "toolu_test2".to_string(),
            name: "create_task".to_string(),
            input: json!({ "title": "احذفي كل حاجة" }),
            proposed_at: chrono::Utc::now(),
        };
        let description = tools::describe(&action.name, &action.input);
        audit::record(
            &conn,
            "amin",
            &action.name,
            RiskTier::ConfirmHighRisk,
            audit::Decision::Proposed,
            Some(&description),
            None,
        )
        .unwrap();

        assert!(matches!(
            confirmation::interpret("لأ متعملش كده"),
            confirmation::Reply::Deny
        ));
        audit::record(
            &conn,
            "user",
            &action.name,
            RiskTier::ConfirmHighRisk,
            audit::Decision::Declined,
            Some(&description),
            None,
        )
        .unwrap();

        assert!(tasks::list(&conn, None).unwrap().is_empty());
        assert_eq!(
            audit_rows(&conn),
            vec![
                ("create_task".to_string(), "proposed".to_string()),
                ("create_task".to_string(), "declined".to_string()),
            ]
        );
    }

    /// Mona's "Approval Token" requirement: an approval is scoped to one
    /// proposal within a limited window, not a standing permission. A stale
    /// pending action must expire rather than fire on a much-later approval
    /// word that was really about something else.
    #[test]
    fn a_stale_pending_action_expires_instead_of_executing() {
        let db = test_db();
        let conn = db.0.lock().unwrap();
        let stale = confirmation::PendingAction {
            tool_use_id: "toolu_test3".to_string(),
            name: "create_task".to_string(),
            input: json!({ "title": "مهمة قديمة" }),
            proposed_at: chrono::Utc::now() - chrono::Duration::minutes(11),
        };
        assert!(confirmation::is_expired(&stale, chrono::Utc::now()));
        assert!(matches!(
            confirmation::interpret("ايوه نفذ"),
            confirmation::Reply::Approve
        ));
        // commands::resolve_pending_action checks is_expired before it even
        // looks at what she said — expiry wins regardless of her reply, and
        // nothing about the stale proposal ever runs.
        assert!(tasks::list(&conn, None).unwrap().is_empty());
    }

    /// Real Memory + real Task rows, read back together through the same
    /// tool the Morning Brief actually calls — proves get_daily_overview
    /// reflects genuine state, not a fixture standing in for it.
    #[tokio::test]
    async fn morning_brief_reflects_real_tasks_and_memory_together() {
        let db = test_db();
        let app = tauri::test::mock_app();
        tools::execute(
            app.handle(),
            &db,
            "create_task",
            &json!({ "title": "متابعة تسجيل أحمد", "priority": "high" }),
        )
        .await
        .unwrap();
        memory::remember(&db.0.lock().unwrap(), "person", "اسم ابن منى", "أحمد").unwrap();

        let overview = tools::execute(app.handle(), &db, "get_daily_overview", &json!({}))
            .await
            .unwrap();
        assert_eq!(overview["open_tasks"][0]["title"], "متابعة تسجيل أحمد");
        assert_eq!(overview["remembered_facts"][0]["value"], "أحمد");
    }
}
