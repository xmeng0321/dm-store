use rusqlite::Connection;

use crate::error::DmStoreError;

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS dm_object (
    id          INTEGER PRIMARY KEY,
    path        TEXT NOT NULL UNIQUE,
    path_hash   INTEGER NOT NULL,
    parent_path TEXT,
    is_multi    INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_object_hash ON dm_object(path_hash);
CREATE INDEX IF NOT EXISTS idx_object_parent ON dm_object(parent_path);

CREATE TABLE IF NOT EXISTS dm_param (
    id          INTEGER PRIMARY KEY,
    path        TEXT NOT NULL UNIQUE,
    path_hash   INTEGER NOT NULL,
    object_path TEXT NOT NULL,
    name        TEXT NOT NULL,
    value       TEXT,
    param_type  TEXT NOT NULL DEFAULT 'string',
    writable    INTEGER NOT NULL DEFAULT 1,
    FOREIGN KEY (object_path) REFERENCES dm_object(path) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_param_hash ON dm_param(path_hash);
CREATE INDEX IF NOT EXISTS idx_param_object ON dm_param(object_path);

CREATE TABLE IF NOT EXISTS dm_schema_object (
    id          INTEGER PRIMARY KEY,
    path        TEXT NOT NULL UNIQUE,
    path_hash   INTEGER NOT NULL,
    parent_path TEXT,
    is_multi    INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_schema_object_hash ON dm_schema_object(path_hash);
CREATE INDEX IF NOT EXISTS idx_schema_object_parent ON dm_schema_object(parent_path);

CREATE TABLE IF NOT EXISTS dm_schema_param (
    id          INTEGER PRIMARY KEY,
    path        TEXT NOT NULL UNIQUE,
    path_hash   INTEGER NOT NULL,
    object_path TEXT NOT NULL,
    name        TEXT NOT NULL,
    value       TEXT,
    param_type  TEXT NOT NULL DEFAULT 'string',
    writable    INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX IF NOT EXISTS idx_schema_param_hash ON dm_schema_param(path_hash);
CREATE INDEX IF NOT EXISTS idx_schema_param_object ON dm_schema_param(object_path);
";

pub fn init_db(conn: &Connection) -> Result<(), DmStoreError> {
    // Performance pragmas
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA cache_size = -8000;",
    )?;

    conn.execute_batch(SCHEMA_SQL)?;

    Ok(())
}
