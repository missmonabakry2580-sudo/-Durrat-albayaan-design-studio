use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

/// Local SQLite connection, managed as Tauri state. The frontend never gets
/// raw SQL access — only the typed commands in `commands.rs` — so invariants
/// like "audit log is append-only" are enforced here, not trusted to the
/// webview.
pub struct Db(pub Mutex<Connection>);

const SCHEMA: &str = include_str!("../schema.sql");

/// Columns added to `tasks` after it first shipped — `CREATE TABLE IF NOT
/// EXISTS` in schema.sql only ever runs for a brand-new database, so an
/// existing installation (Mona already has tasks in her real `amin.db`)
/// needs these added in place instead, or the app would break against her
/// actual data the moment tasks.rs starts reading/writing them. SQLite has
/// no `ADD COLUMN IF NOT EXISTS`, so each statement runs and a "duplicate
/// column name" error (already-migrated database, or the column came from
/// schema.sql's own CREATE TABLE on a fresh install) is swallowed —
/// anything else is a real failure and still propagates.
const TASK_COLUMN_MIGRATIONS: &[&str] = &[
    "ALTER TABLE tasks ADD COLUMN priority TEXT",
    "ALTER TABLE tasks ADD COLUMN deadline TEXT",
    "ALTER TABLE tasks ADD COLUMN project TEXT",
    "ALTER TABLE tasks ADD COLUMN next_action TEXT",
    "ALTER TABLE tasks ADD COLUMN approval_required INTEGER",
    "ALTER TABLE tasks ADD COLUMN dependencies TEXT",
];

fn migrate(conn: &Connection) -> Result<(), String> {
    for statement in TASK_COLUMN_MIGRATIONS {
        if let Err(e) = conn.execute(statement, []) {
            if !e.to_string().contains("duplicate column name") {
                return Err(e.to_string());
            }
        }
    }
    Ok(())
}

pub fn init(path: impl AsRef<Path>) -> Result<Db, String> {
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    migrate(&conn)?;
    Ok(Db(Mutex::new(conn)))
}
