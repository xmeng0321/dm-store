use thiserror::Error;

#[derive(Debug, Error)]
pub enum DmManagerError {
    #[error("dm-store error: {0}")]
    Store(#[from] dm_store_lib::DmStoreError),

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("schema error: {0}")]
    Schema(String),

    #[error("path not found in schema: {0}")]
    NotInSchema(String),

    #[error("path is read-only: {0}")]
    ReadOnly(String),

    #[error("path not in database: {0}")]
    NotInDb(String),

    #[error("invalid value for {path}: {reason}")]
    InvalidValue { path: String, reason: String },

    #[error("not a multi-instance object: {0}")]
    NotMultiInstance(String),

    #[error("hook error for {path}: {reason}")]
    HookError { path: String, reason: String },

    #[error("session closed")]
    SessionClosed,
}
