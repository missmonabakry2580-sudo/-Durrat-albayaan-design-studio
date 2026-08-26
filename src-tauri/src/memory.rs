use chrono::Utc;
use rusqlite::{params, Connection};
use serde::Serialize;
use uuid::Uuid;

/// Structured long-term memory — Mona's explicit request: not one growing
/// transcript replayed every turn (that's `agent::Conversation`/
/// `conversation_history`, which is short-term working memory, not this),
/// but categorized facts Amin can recall, update, and forget on command.
/// Each fact is `(category, key) -> value`; remembering the same
/// `(category, key)` again updates it in place rather than duplicating —
/// "افتكري اسم المدرسة" said twice should correct the fact, not leave two
/// contradictory rows. See tools.rs's `remember_fact`/`forget_fact`/
/// `search_memory` for how Claude actually reaches this, and
/// `agent::memory_prompt_block` for how it's surfaced back to Claude.
#[derive(Serialize, Clone)]
pub struct MemoryFact {
    pub id: String,
    pub category: String,
    pub key: String,
    pub value: String,
    pub created_at: String,
    pub updated_at: String,
}

// `category` is a plain string column, not a Rust enum, deliberately —
// unlike task/follow-up status, a fixed list would mean a code change
// every time a new kind of thing needs remembering. tools.rs's
// `remember_fact` tool description suggests preference/person/project/
// routine/decision to Claude, but nothing here enforces that list.

pub fn remember(conn: &Connection, category: &str, key: &str, value: &str) -> Result<MemoryFact, String> {
    let category = category.trim();
    let key = key.trim();
    let value = value.trim();
    if category.is_empty() || key.is_empty() || value.is_empty() {
        return Err("category, key, and value must all be non-empty".to_string());
    }

    let now = Utc::now().to_rfc3339();
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM memory_facts WHERE category = ?1 AND key = ?2",
            params![category, key],
            |row| row.get(0),
        )
        .ok();

    let id = existing.unwrap_or_else(|| Uuid::new_v4().to_string());
    conn.execute(
        "INSERT INTO memory_facts (id, category, key, value, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(id) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![id, category, key, value, now],
    )
    .map_err(|e| e.to_string())?;

    Ok(MemoryFact {
        id,
        category: category.to_string(),
        key: key.to_string(),
        value: value.to_string(),
        created_at: now.clone(),
        updated_at: now,
    })
}

pub fn forget(conn: &Connection, id: &str) -> Result<(), String> {
    let deleted = conn
        .execute("DELETE FROM memory_facts WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    if deleted == 0 {
        return Err(format!("no memory fact found with id {id}"));
    }
    Ok(())
}

fn row_to_fact(row: &rusqlite::Row) -> rusqlite::Result<MemoryFact> {
    Ok(MemoryFact {
        id: row.get(0)?,
        category: row.get(1)?,
        key: row.get(2)?,
        value: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

const SELECT_COLUMNS: &str = "id, category, key, value, created_at, updated_at";

/// All remembered facts, optionally filtered to one category. Used both by
/// the `search_memory` tool and — unfiltered — to build the compact
/// memory block injected into every turn's system prompt (see
/// `agent::memory_prompt_block`), so this stays a plain full list rather
/// than a paginated query: the whole point is Amin holding all of it in
/// mind at once, and the set is expected to stay small (facts, not a
/// transcript).
pub fn list(conn: &Connection, category: Option<&str>) -> Result<Vec<MemoryFact>, String> {
    let sql = match category {
        Some(_) => format!("SELECT {SELECT_COLUMNS} FROM memory_facts WHERE category = ?1 ORDER BY updated_at DESC"),
        None => format!("SELECT {SELECT_COLUMNS} FROM memory_facts ORDER BY updated_at DESC"),
    };
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = match category {
        Some(c) => stmt.query_map(params![c], row_to_fact),
        None => stmt.query_map([], row_to_fact),
    }
    .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// A simple substring search over both `key` and `value` — no fuzzy
/// matching or embeddings; the fact set is expected to be small enough
/// that a plain `LIKE` is enough, and predictable substring matching is
/// easier to reason about than a similarity score when Claude is deciding
/// what to forget.
pub fn search(conn: &Connection, query: &str) -> Result<Vec<MemoryFact>, String> {
    let pattern = format!("%{}%", query.trim());
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {SELECT_COLUMNS} FROM memory_facts
             WHERE key LIKE ?1 OR value LIKE ?1
             ORDER BY updated_at DESC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map(params![pattern], row_to_fact).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
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
    fn remembers_and_lists_a_fact() {
        let conn = test_db();
        let fact = remember(&conn, "person", "اسم ابن منى", "أحمد").unwrap();
        assert_eq!(fact.value, "أحمد");

        let all = list(&conn, None).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].key, "اسم ابن منى");
    }

    #[test]
    fn remembering_the_same_key_again_updates_instead_of_duplicating() {
        let conn = test_db();
        let first = remember(&conn, "preference", "اسم المدرسة", "قديم").unwrap();
        let second = remember(&conn, "preference", "اسم المدرسة", "درة البيان").unwrap();

        assert_eq!(first.id, second.id, "same (category, key) must reuse the same row");
        let all = list(&conn, None).unwrap();
        assert_eq!(all.len(), 1, "must not duplicate");
        assert_eq!(all[0].value, "درة البيان");
    }

    #[test]
    fn rejects_empty_fields() {
        let conn = test_db();
        assert!(remember(&conn, "", "k", "v").is_err());
        assert!(remember(&conn, "c", "", "v").is_err());
        assert!(remember(&conn, "c", "k", "  ").is_err());
    }

    #[test]
    fn forgets_a_fact_by_id() {
        let conn = test_db();
        let fact = remember(&conn, "decision", "قرار الاجتماع", "تأجيل لبكرة").unwrap();
        forget(&conn, &fact.id).unwrap();
        assert!(list(&conn, None).unwrap().is_empty());
    }

    #[test]
    fn forgetting_an_unknown_id_is_an_error() {
        let conn = test_db();
        assert!(forget(&conn, "does-not-exist").is_err());
    }

    #[test]
    fn filters_by_category() {
        let conn = test_db();
        remember(&conn, "person", "المعلمة", "أ. سارة").unwrap();
        remember(&conn, "routine", "موعد الاجتماع الأسبوعي", "الأحد الساعة 9").unwrap();

        let people = list(&conn, Some("person")).unwrap();
        assert_eq!(people.len(), 1);
        assert_eq!(people[0].key, "المعلمة");
    }

    #[test]
    fn searches_across_key_and_value() {
        let conn = test_db();
        remember(&conn, "project", "تسجيل أحمد", "متابعة أوراق الروضة").unwrap();
        remember(&conn, "person", "ولي أمر أحمد", "0500000000").unwrap();
        remember(&conn, "person", "معلمة الصف", "أ. منى").unwrap();

        let by_value = search(&conn, "الروضة").unwrap();
        assert_eq!(by_value.len(), 1);
        assert_eq!(by_value[0].key, "تسجيل أحمد");

        let by_key = search(&conn, "أحمد").unwrap();
        assert_eq!(by_key.len(), 2);
    }
}
