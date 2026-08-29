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
    /// Mona's explicit task shape — see NewTaskDetails' doc comment.
    /// Claude's own judgment call, not validated against a fixed set here.
    pub priority: Option<String>,
    pub deadline: Option<String>,
    pub project: Option<String>,
    pub next_action: Option<String>,
    pub approval_required: bool,
    pub dependencies: Vec<String>,
}

/// The richer fields Mona spelled out explicitly in her rearchitecture
/// request (she gave the literal JSON shape). All optional — `create()`
/// below is `create_with_details()` with all of these left at their
/// defaults, so the common "دوّني كذا" quick task still works with none of
/// this filled in.
#[derive(Default)]
pub struct NewTaskDetails {
    pub priority: Option<String>,
    pub deadline: Option<String>,
    pub project: Option<String>,
    pub next_action: Option<String>,
    pub approval_required: bool,
    pub dependencies: Vec<String>,
}

const VALID_STATUSES: &[&str] = &["open", "in_progress", "done", "cancelled"];

pub fn create(conn: &Connection, title: &str, source: &str) -> Result<Task, String> {
    create_with_details(conn, title, source, NewTaskDetails::default())
}

pub fn create_with_details(
    conn: &Connection,
    title: &str,
    source: &str,
    details: NewTaskDetails,
) -> Result<Task, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("a task needs a title".to_string());
    }

    let now = Utc::now().to_rfc3339();
    let dependencies_json = serde_json::to_string(&details.dependencies).unwrap_or_else(|_| "[]".to_string());
    let task = Task {
        id: Uuid::new_v4().to_string(),
        title: title.to_string(),
        status: "open".to_string(),
        source: Some(source.to_string()),
        created_at: now.clone(),
        updated_at: now,
        metadata: None,
        priority: details.priority,
        deadline: details.deadline,
        project: details.project,
        next_action: details.next_action,
        approval_required: details.approval_required,
        dependencies: details.dependencies,
    };

    conn.execute(
        "INSERT INTO tasks (
            id, title, status, source, created_at, updated_at, metadata,
            priority, deadline, project, next_action, approval_required, dependencies
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            task.id,
            task.title,
            task.status,
            task.source,
            task.created_at,
            task.updated_at,
            task.metadata,
            task.priority,
            task.deadline,
            task.project,
            task.next_action,
            task.approval_required as i64,
            dependencies_json,
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(task)
}

const SELECT_COLUMNS: &str = "id, title, status, source, created_at, updated_at, metadata,
     priority, deadline, project, next_action, approval_required, dependencies";

fn row_to_task(row: &rusqlite::Row) -> rusqlite::Result<Task> {
    let approval_required: Option<i64> = row.get(11)?;
    let dependencies_json: Option<String> = row.get(12)?;
    let dependencies = dependencies_json
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default();
    Ok(Task {
        id: row.get(0)?,
        title: row.get(1)?,
        status: row.get(2)?,
        source: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        metadata: row.get(6)?,
        priority: row.get(7)?,
        deadline: row.get(8)?,
        project: row.get(9)?,
        next_action: row.get(10)?,
        approval_required: approval_required.unwrap_or(0) != 0,
        dependencies,
    })
}

pub fn list(conn: &Connection, status_filter: Option<&str>) -> Result<Vec<Task>, String> {
    let sql = match status_filter {
        Some(_) => format!("SELECT {SELECT_COLUMNS} FROM tasks WHERE status = ?1 ORDER BY created_at DESC"),
        None => format!("SELECT {SELECT_COLUMNS} FROM tasks ORDER BY created_at DESC"),
    };

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = match status_filter {
        Some(status) => stmt.query_map(params![status], row_to_task),
        None => stmt.query_map([], row_to_task),
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
        assert_eq!(task.approval_required, false);
        assert!(task.dependencies.is_empty());

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

    #[test]
    fn stores_and_round_trips_the_richer_task_fields() {
        let conn = test_db();
        let details = NewTaskDetails {
            priority: Some("high".to_string()),
            deadline: Some("2026-09-01T09:00:00Z".to_string()),
            project: Some("تسجيل أحمد".to_string()),
            next_action: Some("اتصلي بأبو أحمد".to_string()),
            approval_required: true,
            dependencies: vec!["task-1".to_string(), "task-2".to_string()],
        };
        let created = create_with_details(&conn, "متابعة تسجيل أحمد", "amin", details).unwrap();
        assert_eq!(created.priority.as_deref(), Some("high"));
        assert!(created.approval_required);
        assert_eq!(created.dependencies, vec!["task-1", "task-2"]);

        let all = list(&conn, None).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].project.as_deref(), Some("تسجيل أحمد"));
        assert_eq!(all[0].next_action.as_deref(), Some("اتصلي بأبو أحمد"));
        assert_eq!(all[0].dependencies, vec!["task-1", "task-2"]);
    }

    #[test]
    fn a_plain_quick_task_leaves_the_richer_fields_empty() {
        let conn = test_db();
        let task = create(&conn, "دوّني كذا", "quick_capture").unwrap();
        assert!(task.priority.is_none());
        assert!(task.deadline.is_none());
        assert!(task.project.is_none());
        assert!(!task.approval_required);
    }
}
