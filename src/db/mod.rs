pub mod queries;
pub mod schema;

use rusqlite::Connection;

pub fn init(path: &str) -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    schema::create_tables(&conn)?;
    Ok(conn)
}
