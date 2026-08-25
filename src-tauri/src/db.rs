use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

/// Local SQLite connection, managed as Tauri state. The frontend never gets
/// raw SQL access — only the typed commands in `commands.rs` — so invariants
/// like "audit log is append-only" are enforced here, not trusted to the
/// webview.
pub struct Db(pub Mutex<Connection>);

const SCHEMA: &str = include_str!("../schema.sql");

pub fn init(path: impl AsRef<Path>) -> Result<Db, String> {
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    Ok(Db(Mutex::new(conn)))
}
