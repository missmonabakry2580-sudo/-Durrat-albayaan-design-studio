use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::Serialize;
use uuid::Uuid;

/// Phase 4's first slice: local follow-up tracking with escalation stages,
/// tied to the `follow_ups` table that's existed since Phase 0's schema.
/// This is deliberately *local only* for now — "sent" here means "Amin
/// surfaced it in the app", not an email or notification actually going
/// out. Real delivery channels (email via Phase 3's Gmail connector, OS
/// notifications) are follow-on work once those exist to deliver through;
/// wiring this to a channel that doesn't exist yet would be a stub
/// pretending to be a feature, not a real one.

#[derive(Serialize, Clone)]
pub struct FollowUp {
    pub id: String,
    pub task_id: String,
    pub due_at: String,
    pub escalation_stage: String,
    pub status: String,
    pub created_at: String,
}

const STAGES: &[&str] = &["friendly", "firm", "escalate_to_user"];
const STATUSES: &[&str] = &["pending", "sent", "resolved", "cancelled"];

pub fn create(conn: &Connection, task_id: &str, due_at: &str) -> Result<FollowUp, String> {
    // Fails loudly on a bad timestamp rather than silently storing
    // something `list_due` can never correctly compare against.
    DateTime::parse_from_rfc3339(due_at)
        .map_err(|e| format!("due_at must be an RFC3339 timestamp: {e}"))?;

    let task_exists: bool = conn
        .query_row(
            "SELECT 1 FROM tasks WHERE id = ?1",
            params![task_id],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !task_exists {
        return Err(format!("no task found with id {task_id}"));
    }

    let follow_up = FollowUp {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_string(),
        due_at: due_at.to_string(),
        escalation_stage: STAGES[0].to_string(),
        status: "pending".to_string(),
        created_at: Utc::now().to_rfc3339(),
    };

    conn.execute(
        "INSERT INTO follow_ups (id, task_id, due_at, escalation_stage, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            follow_up.id,
            follow_up.task_id,
            follow_up.due_at,
            follow_up.escalation_stage,
            follow_up.status,
            follow_up.created_at,
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(follow_up)
}

fn row_to_follow_up(row: &rusqlite::Row) -> rusqlite::Result<FollowUp> {
    Ok(FollowUp {
        id: row.get(0)?,
        task_id: row.get(1)?,
        due_at: row.get(2)?,
        escalation_stage: row.get(3)?,
        status: row.get(4)?,
        created_at: row.get(5)?,
    })
}

const SELECT_COLUMNS: &str = "id, task_id, due_at, escalation_stage, status, created_at";

pub fn list(conn: &Connection, task_id: Option<&str>) -> Result<Vec<FollowUp>, String> {
    let sql = match task_id {
        Some(_) => format!(
            "SELECT {SELECT_COLUMNS} FROM follow_ups WHERE task_id = ?1 ORDER BY due_at ASC"
        ),
        None => format!("SELECT {SELECT_COLUMNS} FROM follow_ups ORDER BY due_at ASC"),
    };
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = match task_id {
        Some(id) => stmt.query_map(params![id], row_to_follow_up),
        None => stmt.query_map([], row_to_follow_up),
    }
    .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Follow-ups whose due time has passed and are still pending — the ones
/// the UI should actually surface to Mona right now.
pub fn list_due(conn: &Connection, now: DateTime<Utc>) -> Result<Vec<FollowUp>, String> {
    let all = list(conn, None)?;
    Ok(all
        .into_iter()
        .filter(|f| {
            f.status == "pending"
                && DateTime::parse_from_rfc3339(&f.due_at)
                    .map(|due| due <= now)
                    .unwrap_or(false)
        })
        .collect())
}

/// Moves a follow-up to the next escalation stage (friendly -> firm ->
/// escalate_to_user) and marks it "sent" — this app surfaced it, whether
/// or not Mona has acted on it yet. Staying at escalate_to_user on repeat
/// calls rather than erroring: escalating an already-maximally-escalated
/// follow-up is a no-op, not a mistake worth failing on.
pub fn escalate(conn: &Connection, id: &str) -> Result<FollowUp, String> {
    let current = list(conn, None)?
        .into_iter()
        .find(|f| f.id == id)
        .ok_or_else(|| format!("no follow-up found with id {id}"))?;

    let current_index = STAGES
        .iter()
        .position(|s| *s == current.escalation_stage)
        .unwrap_or(0);
    let next_stage = STAGES[current_index.saturating_add(1).min(STAGES.len() - 1)];

    conn.execute(
        "UPDATE follow_ups SET escalation_stage = ?1, status = 'sent' WHERE id = ?2",
        params![next_stage, id],
    )
    .map_err(|e| e.to_string())?;

    let mut updated = current;
    updated.escalation_stage = next_stage.to_string();
    updated.status = "sent".to_string();
    Ok(updated)
}

pub fn set_status(conn: &Connection, id: &str, status: &str) -> Result<(), String> {
    if !STATUSES.contains(&status) {
        return Err(format!(
            "unknown follow-up status: {status} (expected one of {STATUSES:?})"
        ));
    }
    let updated = conn
        .execute(
            "UPDATE follow_ups SET status = ?1 WHERE id = ?2",
            params![status, id],
        )
        .map_err(|e| e.to_string())?;
    if updated == 0 {
        return Err(format!("no follow-up found with id {id}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../schema.sql")).unwrap();
        conn
    }

    #[test]
    fn creates_a_follow_up_for_an_existing_task() {
        let conn = test_db();
        let task = tasks::create(&conn, "call the school", "manual").unwrap();
        let f = create(&conn, &task.id, "2026-01-01T09:00:00Z").unwrap();
        assert_eq!(f.escalation_stage, "friendly");
        assert_eq!(f.status, "pending");
    }

    #[test]
    fn rejects_a_follow_up_for_a_missing_task() {
        let conn = test_db();
        assert!(create(&conn, "does-not-exist", "2026-01-01T09:00:00Z").is_err());
    }

    #[test]
    fn rejects_a_non_rfc3339_due_at() {
        let conn = test_db();
        let task = tasks::create(&conn, "x", "manual").unwrap();
        assert!(create(&conn, &task.id, "tomorrow morning").is_err());
    }

    #[test]
    fn list_due_only_returns_pending_past_due_items() {
        let conn = test_db();
        let task = tasks::create(&conn, "x", "manual").unwrap();
        let past = create(&conn, &task.id, "2020-01-01T00:00:00Z").unwrap();
        let future = create(&conn, &task.id, "2999-01-01T00:00:00Z").unwrap();

        let now: DateTime<Utc> = "2026-01-01T00:00:00Z".parse().unwrap();
        let due = list_due(&conn, now).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, past.id);
        let _ = future;
    }

    #[test]
    fn escalate_advances_through_stages_and_caps_at_the_last_one() {
        let conn = test_db();
        let task = tasks::create(&conn, "x", "manual").unwrap();
        let f = create(&conn, &task.id, "2020-01-01T00:00:00Z").unwrap();

        let f = escalate(&conn, &f.id).unwrap();
        assert_eq!(f.escalation_stage, "firm");
        let f = escalate(&conn, &f.id).unwrap();
        assert_eq!(f.escalation_stage, "escalate_to_user");
        let f = escalate(&conn, &f.id).unwrap();
        assert_eq!(f.escalation_stage, "escalate_to_user", "should cap, not error or wrap");
    }

    #[test]
    fn set_status_rejects_unknown_values() {
        let conn = test_db();
        let task = tasks::create(&conn, "x", "manual").unwrap();
        let f = create(&conn, &task.id, "2020-01-01T00:00:00Z").unwrap();
        assert!(set_status(&conn, &f.id, "snoozed").is_err());
        assert!(set_status(&conn, &f.id, "resolved").is_ok());
    }
}
