use crate::Result;
use rusqlite::{Connection, params};
use std::path::Path;

pub const CURRENT_SCHEMA_VERSION: i64 = 3;

pub fn open_database(local_cache_dir: &Path) -> Result<Connection> {
    std::fs::create_dir_all(local_cache_dir)?;
    let conn = Connection::open(local_cache_dir.join("kmo_index.db"))?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER NOT NULL
        );

        INSERT INTO schema_version(version)
        SELECT 0
        WHERE NOT EXISTS (SELECT 1 FROM schema_version);

        CREATE TABLE IF NOT EXISTS blob_index (
            book_hash TEXT PRIMARY KEY,
            last_remote_size INTEGER NOT NULL DEFAULT 0,
            last_remote_etag TEXT,
            last_sync_mtime INTEGER NOT NULL DEFAULT 0,
            local_file_path TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS meta_index (
            meta_id TEXT PRIMARY KEY,
            book_hash TEXT NOT NULL,
            last_meta_hash TEXT NOT NULL,
            last_sync_ts INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS conflict_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp INTEGER NOT NULL,
            object_type TEXT NOT NULL,
            object_id TEXT NOT NULL,
            description TEXT,
            local_json TEXT,
            remote_json TEXT,
            resolved_ts INTEGER
        );

        CREATE TABLE IF NOT EXISTS cas_chunk_index (
            blake3_hash TEXT PRIMARY KEY,
            size INTEGER NOT NULL,
            ref_count INTEGER NOT NULL DEFAULT 1,
            last_seen_ts INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS merkle_node_cache (
            book_hash TEXT NOT NULL,
            level INTEGER NOT NULL,
            node_index INTEGER NOT NULL,
            blake3_hash TEXT NOT NULL,
            PRIMARY KEY (book_hash, level, node_index)
        );

        CREATE TABLE IF NOT EXISTS kek_versions (
            version INTEGER PRIMARY KEY,
            kek_id TEXT NOT NULL UNIQUE,
            created_ts INTEGER NOT NULL,
            retired_ts INTEGER,
            kdf_params TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS sync_state (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        "#,
    )?;
    Ok(())
}

pub fn current_schema_version(conn: &Connection) -> Result<i64> {
    let version = conn.query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
        row.get::<_, i64>(0)
    })?;
    Ok(version)
}

pub fn migrate(conn: &Connection) -> Result<()> {
    init_schema(conn)?;
    let version = current_schema_version(conn)?;
    if version < 2 {
        add_column_if_missing(conn, "conflict_log", "local_json", "TEXT")?;
        add_column_if_missing(conn, "conflict_log", "remote_json", "TEXT")?;
        add_column_if_missing(conn, "conflict_log", "resolved_ts", "INTEGER")?;
    }
    if version < 3 {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS sync_state (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
            [],
        )?;
    }
    if version < CURRENT_SCHEMA_VERSION {
        conn.execute(
            "UPDATE schema_version SET version = ?1",
            params![CURRENT_SCHEMA_VERSION],
        )?;
    }
    Ok(())
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    column_type: &str,
) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for existing in columns {
        if existing? == column {
            return Ok(());
        }
    }
    conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {column_type}"),
        [],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_initializes_and_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        init_schema(&conn).unwrap();
        migrate(&conn).unwrap();
        assert_eq!(
            current_schema_version(&conn).unwrap(),
            CURRENT_SCHEMA_VERSION
        );
    }

    #[test]
    fn blob_and_meta_indexes_can_be_written() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        conn.execute(
            "INSERT INTO blob_index(book_hash, last_remote_size, local_file_path) VALUES (?1, ?2, ?3)",
            params!["book-1", 10_i64, "encrypted-path"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO meta_index(meta_id, book_hash, last_meta_hash) VALUES (?1, ?2, ?3)",
            params!["meta-1", "book-1", "hash-1"],
        )
        .unwrap();

        let size: i64 = conn
            .query_row(
                "SELECT last_remote_size FROM blob_index WHERE book_hash = ?1",
                params!["book-1"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(size, 10);
    }
}
