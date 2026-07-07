use thiserror::Error;

pub type Result<T> = std::result::Result<T, SyncError>;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("network error: {0}")]
    Network(String),
    #[error("invalid argument: {0}")]
    InvalidArg(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("crypto error: {0}")]
    Crypto(String),
    #[error("conflict error: {0}")]
    Conflict(String),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("protobuf encode error: {0}")]
    ProstEncode(#[from] prost::EncodeError),
    #[error("protobuf decode error: {0}")]
    ProstDecode(#[from] prost::DecodeError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("version mismatch: {0}")]
    VersionMismatch(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl SyncError {
    pub fn code(&self) -> i32 {
        match self {
            SyncError::Network(_) => 1,
            SyncError::Storage(_) => 2,
            SyncError::Crypto(_) => 3,
            SyncError::Conflict(_) => 4,
            SyncError::InvalidArg(_) => 5,
            SyncError::VersionMismatch(_) => 11,
            SyncError::Database(_)
            | SyncError::ProstEncode(_)
            | SyncError::ProstDecode(_)
            | SyncError::Io(_)
            | SyncError::Json(_)
            | SyncError::Internal(_) => 6,
        }
    }
}
