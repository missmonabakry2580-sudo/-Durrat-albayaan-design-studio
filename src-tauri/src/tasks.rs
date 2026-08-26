use chrono::Utc;
use rusqlite::{params, Connection};
use serde::Serialize;
use uuid::Uuid;

/// Phase 2's first real capability: local task management + Quick Capture,
/// both backed by the `tasks` table that's existed since Phase 0's schema.
/// No tools here touch the network or the filesystem outside this
/// database — see docs/ARCHITECTURE.md "Phase 2" for what's still ahead
/// (file access, browser control) and why those aren't in this file.

#[derive(Serialize, Clone)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub status: String,
    pub source: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub metadata: Option<String>,
}

const VALID_STATUSES: &[&str] = &["open", "in_progress", "done", "cancelled"];

pub fn create(conn: &Connection, title: &str, source: &str) -> Result<Task, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("a task needs a title".to_string());
    }

    let task = Task {
        id: Uuid::new_v4().to_string(),
        title: title.to_string(),
        status: "open".to_string(),
        source: Some(source.to_string()),
        created_at: Utc::now().to_rfc3339(),
        updated_at: Utc::now().to_rfc3339(),
        metadata: None,
    };

    conn.execute(
        "INSERT INTO tasks (id, title, status, source, created_at, updated_at, metadata)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            task.id,
            task.title,
            task.status,
            task.source,
            task.created_at,
            task.updated_at,
            task.metadata,
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(task)
}

pub fn list(conn: &Connection, status_filter: Option<&str>) -> Result<Vec<Task>, String> {
    let sql = match status_filter {
        Some(_) => {
            "SELECT id, title, status, source, created_at, updated_at, metadata
             FROM tasks WHERE status = ?1 ORDER BY created_at DESC"
        }
        None => {
            "SELECT id, title, status, source, created_at, updated_at, metadata
             FROM tasks ORDER BY created_at DESC"
        }
    };

    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let row_mapper = |row: &rusqlite::Row| {
        Ok(Task {
            id: row.get(0)?,
            title: row.get(1)?,
            status: row.get(2)?,
            source: row.get(3)?,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
            metadata: row.get(6)?,
        })
    };

    let rows = match status_filter {
        Some(status) => stmt.query_map(params![status], row_mapper),
        None => stmt.query_map([], row_mapper),
    }
    .map_err(|e| e.to_string())?;

    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn set_status(conn: &Connection, id: &str, status: &str) -> Result<(), String> {
    if !VALID_STATUSES.contains(&status) {
        return Err(format!(
            "unknown task status: {status} (expected one of {VALID_STATUSES:?})"
        ));
    }

    let updated = conn
        .execute(
            "UPDATE tasks SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status, Utc::now().to_rfc3339(), id],
        )
        .map_err(|e| e.to_string())?;

    if updated == 0 {
        return Err(format!("no task found with id {id}"));
    }
    Ok(())
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
    fn creates_and_lists_a_task() {
        let conn = test_db();
        let task = create(&conn, "  اتصلي بالمدرسة  ", "manual").unwrap();
        assert_eq!(task.title, "اتصلي بالمدرسة");
        assert_eq!(task.status, "open");

        let all = list(&conn, None).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, task.id);
    }

    #[test]
    fn rejects_a_blank_title() {
        let conn = test_db();
        assert!(create(&conn, "   ", "manual").is_err());
    }

    #[test]
    fn filters_by_status() {
        let conn = test_db();
        let a = create(&conn, "task a", "manual").unwrap();
        let _b = create(&conn, "task b", "manual").unwrap();
        set_status(&conn, &a.id, "done").unwrap();

        let open_only = list(&conn, Some("open")).unwrap();
        assert_eq!(open_only.len(), 1);
        assert_eq!(open_only[0].title, "task b");

        let done_only = list(&conn, Some("done")).unwrap();
        assert_eq!(done_only.len(), 1);
        assert_eq!(done_only[0].id, a.id);
    }

    #[test]
    fn rejects_an_unknown_status() {
        let conn = test_db();
        let task = create(&conn, "x", "manual").unwrap();
        assert!(set_status(&conn, &task.id, "sleeping").is_err());
    }

    #[test]
    fn set_status_errors_on_an_unknown_id() {
        let conn = test_db();
        assert!(set_status(&conn, "does-not-exist", "done").is_err());
    }
}
