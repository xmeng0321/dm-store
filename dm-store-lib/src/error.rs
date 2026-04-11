use thiserror::Error;

#[derive(Debug, Error)]
pub enum DmStoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("invalid path: {path} -- {reason}")]
    InvalidPath { path: String, reason: String },

    #[error("path not found: {0}")]
    NotFound(String),

    #[error("parameter is read-only: {0}")]
    ReadOnly(String),

    #[error("not a multi-instance object: {0}")]
    NotMultiInstance(String),

    #[error("cannot delete non-instance path: {0}")]
    NotAnInstance(String),

    #[error("path already exists: {0}")]
    AlreadyExists(String),

    #[error("session already closed")]
    SessionClosed,

    #[error("schema error: {0}")]
    Schema(String),
}
