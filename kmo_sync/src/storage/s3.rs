use super::{RemoteFileInfo, RemoteStorage};
use crate::{Result, SyncError};
use async_trait::async_trait;
use bytes::Bytes;
use futures::TryStreamExt;
use object_store::aws::AmazonS3Builder;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, PutPayload};
use std::io::Cursor;
use tokio::io::{AsyncRead, AsyncReadExt};

#[derive(Debug, Clone)]
pub struct S3Config {
    pub endpoint: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    pub region: String,
    pub root_prefix: String,
    pub path_style: bool,
    pub allow_http: bool,
}

#[derive(Debug)]
pub struct S3Storage {
    inner: object_store::aws::AmazonS3,
    root_prefix: String,
}

impl S3Storage {
    pub fn new(config: S3Config) -> Result<Self> {
        let inner = AmazonS3Builder::new()
            .with_endpoint(config.endpoint)
            .with_bucket_name(config.bucket)
            .with_access_key_id(config.access_key)
            .with_secret_access_key(config.secret_key)
            .with_region(config.region)
            .with_allow_http(config.allow_http)
            .with_virtual_hosted_style_request(!config.path_style)
            .build()
            .map_err(map_object_store_error)?;

        Ok(Self {
            inner,
            root_prefix: config.root_prefix.trim_matches('/').to_string(),
        })
    }

    fn full_path(&self, remote_path: &str) -> Result<ObjectPath> {
        if remote_path.starts_with('/') || remote_path.contains("..") {
            return Err(SyncError::InvalidArg(format!(
                "invalid remote path: {remote_path}"
            )));
        }
        let trimmed = remote_path.trim_matches('/');
        let full = if self.root_prefix.is_empty() {
            trimmed.to_string()
        } else if trimmed.is_empty() {
            self.root_prefix.clone()
        } else {
            format!("{}/{}", self.root_prefix, trimmed)
        };
        ObjectPath::parse(full).map_err(|err| SyncError::Storage(err.to_string()))
    }

    fn strip_root(&self, path: &ObjectPath) -> String {
        let path = path.to_string();
        if self.root_prefix.is_empty() {
            return path;
        }
        path.strip_prefix(&format!("{}/", self.root_prefix))
            .unwrap_or(&path)
            .to_string()
    }
}

#[async_trait]
impl RemoteStorage for S3Storage {
    async fn exists(&self, remote_path: &str) -> Result<bool> {
        let p = self.full_path(remote_path)?;
        with_retry("exists", || async {
            match self.inner.head(&p).await {
                Ok(_) => Ok(true),
                Err(object_store::Error::NotFound { .. }) => Ok(false),
                Err(err) => Err(err),
            }
        })
        .await
    }

    async fn read_object(&self, remote_path: &str) -> Result<Vec<u8>> {
        let p = self.full_path(remote_path)?;
        with_retry("read_object", || async {
            let result = self.inner.get(&p).await?;
            let bytes = result.bytes().await?;
            Ok(bytes.to_vec())
        })
        .await
    }

    async fn read_object_optional(&self, remote_path: &str) -> Result<Option<Vec<u8>>> {
        let p = self.full_path(remote_path)?;
        with_retry("read_object_optional", || async {
            match self.inner.get(&p).await {
                Ok(result) => {
                    let bytes = result.bytes().await?;
                    Ok(Some(bytes.to_vec()))
                }
                Err(object_store::Error::NotFound { .. }) => Ok(None),
                Err(err) => Err(err),
            }
        })
        .await
    }

    async fn write_object(&self, remote_path: &str, data: &[u8]) -> Result<()> {
        let p = self.full_path(remote_path)?;
        let payload = PutPayload::from(data.to_vec());
        let _ = with_retry("write_object", || async {
            self.inner.put(&p, payload.clone()).await
        })
        .await?;
        Ok(())
    }

    async fn remove(&self, remote_path: &str) -> Result<()> {
        let p = self.full_path(remote_path)?;
        match self.inner.delete(&p).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(err) => Err(map_object_store_error(err)),
        }
    }

    async fn list_prefix(&self, prefix: &str) -> Result<Vec<RemoteFileInfo>> {
        let prefix_obj = self.full_path(prefix)?;
        with_retry("list_prefix", || async {
            let mut stream = self.inner.list(Some(&prefix_obj));
            let mut infos = Vec::new();
            loop {
                match stream.try_next().await {
                    Ok(Some(meta)) => infos.push(RemoteFileInfo {
                        path: self.strip_root(&meta.location),
                        size: meta.size as u64,
                        mtime: meta.last_modified.timestamp_millis(),
                        etag: meta.e_tag,
                    }),
                    Ok(None) => break,
                    Err(e) => return Err(e),
                }
            }
            infos.sort_by(|a, b| a.path.cmp(&b.path));
            Ok(infos)
        })
        .await
    }

    async fn stat(&self, remote_path: &str) -> Result<RemoteFileInfo> {
        let p = self.full_path(remote_path)?;
        let meta = with_retry("stat", || async { self.inner.head(&p).await }).await?;
        Ok(RemoteFileInfo {
            path: remote_path.to_string(),
            size: meta.size as u64,
            mtime: meta.last_modified.timestamp_millis(),
            etag: meta.e_tag,
        })
    }

    async fn upload_large(
        &self,
        remote_path: &str,
        mut stream: Box<dyn AsyncRead + Unpin + Send>,
        _total_size: u64,
    ) -> Result<()> {
        let mut upload = self
            .inner
            .put_multipart(&self.full_path(remote_path)?)
            .await
            .map_err(map_object_store_error)?;
        let mut buf = vec![0_u8; 8 * 1024 * 1024];
        loop {
            let read = stream.read(&mut buf).await?;
            if read == 0 {
                break;
            }
            upload
                .put_part(PutPayload::from(Bytes::copy_from_slice(&buf[..read])))
                .await
                .map_err(map_object_store_error)?;
        }
        upload.complete().await.map_err(map_object_store_error)?;
        Ok(())
    }

    async fn download_large(&self, remote_path: &str) -> Result<Box<dyn AsyncRead + Unpin + Send>> {
        let bytes = self.read_object(remote_path).await?;
        Ok(Box::new(Cursor::new(bytes)))
    }
}

async fn with_retry<F, Fut, T>(label: &'static str, mut op: F) -> crate::Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = object_store::Result<T>>,
{
    // R2 occasionally returns bodies that the S3 XML decoder can't parse
    // (e.g. trailing whitespace, malformed ETags, intermittent 5xx shapes).
    // Retry a few times with exponential backoff before surfacing the error.
    const MAX_ATTEMPTS: u32 = 5;
    let mut last_err: Option<object_store::Error> = None;
    let mut attempt: u32 = 0;
    while attempt < MAX_ATTEMPTS {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if matches!(e, object_store::Error::NotFound { .. })
                    || matches!(e, object_store::Error::InvalidPath { .. })
                {
                    return Err(map_object_store_error(e));
                }
                let backoff = std::time::Duration::from_millis(120_u64 << attempt);
                eprintln!("[s3 / {label}] attempt {attempt} failed: {e}; retry in {backoff:?}");
                tokio::time::sleep(backoff).await;
                last_err = Some(e);
                attempt += 1;
            }
        }
    }
    Err(map_object_store_error(
        last_err.expect("with_retry: error must be present"),
    ))
}

fn map_object_store_error(err: object_store::Error) -> SyncError {
    match err {
        object_store::Error::NotFound { path, .. } => {
            SyncError::Storage(format!("object not found: {path}"))
        }
        object_store::Error::AlreadyExists { path, .. } => {
            SyncError::Storage(format!("object already exists: {path}"))
        }
        object_store::Error::Precondition { path, .. } => {
            SyncError::Storage(format!("object precondition failed: {path}"))
        }
        object_store::Error::InvalidPath { source } => SyncError::InvalidArg(source.to_string()),
        other => SyncError::Storage(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s3_root_prefix_paths_are_normalized() {
        let config = S3Config {
            endpoint: "http://127.0.0.1:9000".to_string(),
            bucket: "kmo-test".to_string(),
            access_key: "minioadmin".to_string(),
            secret_key: "minioadmin".to_string(),
            region: "us-east-1".to_string(),
            root_prefix: "yuewei/".to_string(),
            path_style: true,
            allow_http: true,
        };
        let storage = S3Storage::new(config).unwrap();
        assert_eq!(
            storage
                .full_path("books/77b88663/metas/a.meta")
                .unwrap()
                .to_string(),
            "yuewei/books/77b88663/metas/a.meta"
        );
    }
}
