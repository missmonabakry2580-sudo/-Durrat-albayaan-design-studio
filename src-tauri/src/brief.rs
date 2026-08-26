use chrono::{Duration, Utc};
use rusqlite::{params, Connection};
use serde::Serialize;

/// Phase 3's first slice, built without Gmail/Calendar: a "what changed"
/// summary of Amin's own local activity — tasks, follow-ups, the audit
/// log. Real Gmail/Calendar data joins this once Mona has created a
/// Google OAuth client for those connectors to authenticate against;
/// nothing here waits on that, since none of it needs an external
/// account.
#[derive(Serialize)]
pub struct DeltaBrief {
    pub open_tasks: i64,
    pub tasks_created_last_24h: i64,
    pub tasks_completed_last_24h: i64,
    pub due_follow_ups: i64,
    pub recent_audit_events: Vec<String>,
}

pub fn generate(conn: &Connection) -> Result<DeltaBrief, String> {
    let since = (Utc::now() - Duration::hours(24)).to_rfc3339();

    let open_tasks: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE status IN ('open', 'in_progress')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;

    let tasks_created_last_24h: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE created_at >= ?1",
            params![since],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;

    let tasks_completed_last_24h: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE status = 'done' AND updated_at >= ?1",
            params![since],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;

    let due_follow_ups = crate::followups::list_due(conn, Utc::now())?.len() as i64;

    let mut stmt = conn
        .prepare("SELECT ts, actor, action, decision FROM audit_log ORDER BY ts DESC LIMIT 5")
        .map_err(|e| e.to_string())?;
    let recent_audit_events = stmt
        .query_map([], |row| {
            let ts: String = row.get(0)?;
            let actor: String = row.get(1)?;
            let action: String = row.get(2)?;
            let decision: String = row.get(3)?;
            Ok(format!("{ts} — {actor} {action} ({decision})"))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(DeltaBrief {
        open_tasks,
        tasks_created_last_24h,
        tasks_completed_last_24h,
        due_follow_ups,
        recent_audit_events,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{audit, followups, policy::RiskTier, tasks};

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../schema.sql")).unwrap();
        conn
    }

    #[test]
    fn counts_open_and_recently_created_tasks() {
        let conn = test_db();
        let a = tasks::create(&conn, "a", "manual").unwrap();
        tasks::create(&conn, "b", "manual").unwrap();
        tasks::set_status(&conn, &a.id, "done").unwrap();

        let brief = generate(&conn).unwrap();
        assert_eq!(brief.open_tasks, 1);
        assert_eq!(brief.tasks_created_last_24h, 2);
        assert_eq!(brief.tasks_completed_last_24h, 1);
    }

    #[test]
    fn counts_due_follow_ups() {
        let conn = test_db();
        let task = tasks::create(&conn, "x", "manual").unwrap();
        followups::create(&conn, &task.id, "2020-01-01T00:00:00Z").unwrap();

        let brief = generate(&conn).unwrap();
        assert_eq!(brief.due_follow_ups, 1);
    }

    #[test]
    fn includes_recent_audit_events() {
        let conn = test_db();
        audit::record(
            &conn,
            "user",
            "did_a_thing",
            RiskTier::Auto,
            audit::Decision::Executed,
            None,
            None,
        )
        .unwrap();

        let brief = generate(&conn).unwrap();
        assert_eq!(brief.recent_audit_events.len(), 1);
        assert!(brief.recent_audit_events[0].contains("did_a_thing"));
    }
}
