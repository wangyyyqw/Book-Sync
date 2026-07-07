pub mod file;
pub mod s3;
pub mod webdav;

use crate::{Result, SyncError};
use async_trait::async_trait;
use tokio::io::AsyncRead;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFileInfo {
    pub path: String,
    pub size: u64,
    pub mtime: i64,
    pub etag: Option<String>,
}

#[async_trait]
pub trait RemoteStorage: Send + Sync {
    async fn exists(&self, remote_path: &str) -> Result<bool>;
    async fn read_object(&self, remote_path: &str) -> Result<Vec<u8>>;
    async fn read_object_optional(&self, remote_path: &str) -> Result<Option<Vec<u8>>>;
    async fn write_object(&self, remote_path: &str, data: &[u8]) -> Result<()>;
    async fn remove(&self, remote_path: &str) -> Result<()>;
    async fn list_prefix(&self, prefix: &str) -> Result<Vec<RemoteFileInfo>>;
    async fn stat(&self, remote_path: &str) -> Result<RemoteFileInfo>;
    async fn upload_large(
        &self,
        remote_path: &str,
        stream: Box<dyn AsyncRead + Unpin + Send>,
        total_size: u64,
    ) -> Result<()>;
    async fn download_large(&self, remote_path: &str) -> Result<Box<dyn AsyncRead + Unpin + Send>>;
}

pub fn unsupported_storage() -> SyncError {
    SyncError::Storage("remote storage adapters are implemented in M2".to_string())
}
