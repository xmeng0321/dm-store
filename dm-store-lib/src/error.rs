use thiserror::Error;

#[derive(Debug, Error)]
pub enum DmStoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// Like `Sqlite`, but with a short operation description so production
    /// logs can tell which query failed without a stack trace.
    #[error("SQLite error while {context}: {source}")]
    SqliteOp {
        #[source]
        source: rusqlite::Error,
        context: String,
    },

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

/// Attach a short operation description to a `Result<_, rusqlite::Error>`.
///
/// Turns opaque `SQLite error: no such table` messages into
/// `SQLite error while loading cache: no such table`.
pub(crate) trait ResultExt<T> {
    fn ctx(self, context: &'static str) -> Result<T, DmStoreError>;
    fn ctx_with<F>(self, context: F) -> Result<T, DmStoreError>
    where
        F: FnOnce() -> String;
}

impl<T> ResultExt<T> for Result<T, rusqlite::Error> {
    fn ctx(self, context: &'static str) -> Result<T, DmStoreError> {
        self.map_err(|source| DmStoreError::SqliteOp {
            source,
            context: context.to_string(),
        })
    }

    fn ctx_with<F>(self, context: F) -> Result<T, DmStoreError>
    where
        F: FnOnce() -> String,
    {
        self.map_err(|source| DmStoreError::SqliteOp {
            source,
            context: context(),
        })
    }
}
