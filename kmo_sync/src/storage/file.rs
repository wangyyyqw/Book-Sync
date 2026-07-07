use super::{RemoteFileInfo, RemoteStorage};
use crate::{Result, SyncError};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use tokio::fs;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Clone)]
pub struct FileStorage {
    root: PathBuf,
}

impl FileStorage {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn full_path(&self, remote_path: &str) -> Result<PathBuf> {
        let path = Path::new(remote_path);
        if path.is_absolute() || remote_path.contains("..") {
            return Err(SyncError::InvalidArg(format!(
                "invalid remote path: {remote_path}"
            )));
        }
        Ok(self.root.join(path))
    }

    async fn ensure_parent(path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        Ok(())
    }

    fn info_from_metadata(path: String, metadata: std::fs::Metadata) -> Result<RemoteFileInfo> {
        let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
        let mtime = modified
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        Ok(RemoteFileInfo {
            path,
            size: metadata.len(),
            mtime,
            etag: None,
        })
    }
}

#[async_trait]
impl RemoteStorage for FileStorage {
    async fn exists(&self, remote_path: &str) -> Result<bool> {
        Ok(fs::metadata(self.full_path(remote_path)?).await.is_ok())
    }

    async fn read_object(&self, remote_path: &str) -> Result<Vec<u8>> {
        fs::read(self.full_path(remote_path)?)
            .await
            .map_err(SyncError::from)
    }

    async fn read_object_optional(&self, remote_path: &str) -> Result<Option<Vec<u8>>> {
        match fs::read(self.full_path(remote_path)?).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    async fn write_object(&self, remote_path: &str, data: &[u8]) -> Result<()> {
        let path = self.full_path(remote_path)?;
        Self::ensure_parent(&path).await?;
        fs::write(path, data).await?;
        Ok(())
    }

    async fn remove(&self, remote_path: &str) -> Result<()> {
        let path = self.full_path(remote_path)?;
        match fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    async fn list_prefix(&self, prefix: &str) -> Result<Vec<RemoteFileInfo>> {
        let root = self.full_path(prefix)?;
        let mut results = Vec::new();
        if fs::metadata(&root).await.is_err() {
            return Ok(results);
        }
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let mut entries = fs::read_dir(&dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                let metadata = entry.metadata().await?;
                let path = entry.path();
                if metadata.is_dir() {
                    stack.push(path);
                } else {
                    let rel = path
                        .strip_prefix(&self.root)
                        .map_err(|err| SyncError::Internal(err.to_string()))?
                        .to_string_lossy()
                        .replace('\\', "/");
                    results.push(Self::info_from_metadata(rel, metadata)?);
                }
            }
        }
        results.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(results)
    }

    async fn stat(&self, remote_path: &str) -> Result<RemoteFileInfo> {
        let path = self.full_path(remote_path)?;
        let metadata = fs::metadata(&path).await?;
        Self::info_from_metadata(remote_path.to_string(), metadata)
    }

    async fn upload_large(
        &self,
        remote_path: &str,
        mut stream: Box<dyn AsyncRead + Unpin + Send>,
        _total_size: u64,
    ) -> Result<()> {
        let path = self.full_path(remote_path)?;
        Self::ensure_parent(&path).await?;
        let mut file = fs::File::create(path).await?;
        let mut buf = [0_u8; 64 * 1024];
        loop {
            let read = stream.read(&mut buf).await?;
            if read == 0 {
                break;
            }
            file.write_all(&buf[..read]).await?;
        }
        file.flush().await?;
        Ok(())
    }

    async fn download_large(&self, remote_path: &str) -> Result<Box<dyn AsyncRead + Unpin + Send>> {
        let file = fs::File::open(self.full_path(remote_path)?).await?;
        Ok(Box::new(file))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_storage_contract_basic_object_flow() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileStorage::new(dir.path().to_path_buf());

        assert!(!storage.exists("metas/a.meta").await.unwrap());
        storage
            .write_object("metas/a.meta", b"hello")
            .await
            .unwrap();
        assert!(storage.exists("metas/a.meta").await.unwrap());
        assert_eq!(storage.read_object("metas/a.meta").await.unwrap(), b"hello");

        let listed = storage.list_prefix("metas").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, "metas/a.meta");

        let stat = storage.stat("metas/a.meta").await.unwrap();
        assert_eq!(stat.size, 5);

        storage.remove("metas/a.meta").await.unwrap();
        assert!(!storage.exists("metas/a.meta").await.unwrap());
    }
}
