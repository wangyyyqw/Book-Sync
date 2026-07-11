use super::{RemoteFileInfo, RemoteStorage, RemoteVersion, VersionedObject};
use crate::{Result, SyncError};
use async_trait::async_trait;
use futures::TryStreamExt;
use quick_xml::Reader;
use quick_xml::events::Event;
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode, Url};
use std::time::Duration;
use tokio::io::AsyncRead;

#[derive(Debug, Clone)]
pub struct WebDavConfig {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub root_dir: String,
}

#[derive(Debug, Clone)]
pub struct WebDavStorage {
    client: Client,
    base_url: Url,
    username: Option<String>,
    password: Option<String>,
    root_dir: String,
}

impl WebDavStorage {
    pub fn new(config: WebDavConfig) -> Result<Self> {
        let mut base_url =
            Url::parse(&config.url).map_err(|err| SyncError::InvalidArg(err.to_string()))?;
        if !base_url.path().ends_with('/') {
            let path = format!("{}/", base_url.path());
            base_url.set_path(&path);
        }
        Ok(Self {
            client: Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(60))
                .pool_idle_timeout(Duration::from_secs(90))
                .build()
                .map_err(map_reqwest_error)?,
            base_url,
            username: config.username,
            password: config.password,
            root_dir: config.root_dir.trim_matches('/').to_string(),
        })
    }

    fn validate_remote_path(remote_path: &str) -> Result<String> {
        if remote_path.starts_with('/') || remote_path.contains("..") {
            return Err(SyncError::InvalidArg(format!(
                "invalid remote path: {remote_path}"
            )));
        }
        Ok(remote_path.trim_matches('/').to_string())
    }

    fn webdav_path(&self, remote_path: &str) -> Result<String> {
        let remote_path = Self::validate_remote_path(remote_path)?;
        let path = if self.root_dir.is_empty() {
            remote_path
        } else if remote_path.is_empty() {
            self.root_dir.clone()
        } else {
            format!("{}/{}", self.root_dir, remote_path)
        };
        Ok(path)
    }

    fn url_for_path(&self, webdav_path: &str) -> Result<Url> {
        let encoded = webdav_path
            .split('/')
            .filter(|part| !part.is_empty())
            .map(url_encode_segment)
            .collect::<Vec<_>>()
            .join("/");
        self.base_url
            .join(&encoded)
            .map_err(|err| SyncError::InvalidArg(err.to_string()))
    }

    fn url_for_remote_path(&self, remote_path: &str) -> Result<Url> {
        let path = self.webdav_path(remote_path)?;
        self.url_for_path(&path)
    }

    fn apply_auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match (&self.username, &self.password) {
            (Some(username), Some(password)) => request.basic_auth(username, Some(password)),
            (Some(username), None) => request.basic_auth(username, Option::<String>::None),
            _ => request,
        }
    }

    async fn send_with_retry<F>(&self, label: &'static str, mut build: F) -> Result<Response>
    where
        F: FnMut() -> Result<RequestBuilder>,
    {
        const MAX_ATTEMPTS: u32 = 4;
        let mut attempt = 0;
        loop {
            match build()?.send().await {
                Ok(response)
                    if is_retryable_status(response.status()) && attempt + 1 < MAX_ATTEMPTS =>
                {
                    let status = response.status();
                    let backoff = Duration::from_millis(120_u64 << attempt);
                    eprintln!(
                        "[webdav / {label}] attempt {attempt} returned HTTP {status}; retry in {backoff:?}"
                    );
                    tokio::time::sleep(backoff).await;
                }
                Ok(response) => return Ok(response),
                Err(err) if is_retryable_reqwest_error(&err) && attempt + 1 < MAX_ATTEMPTS => {
                    let backoff = Duration::from_millis(120_u64 << attempt);
                    eprintln!(
                        "[webdav / {label}] attempt {attempt} failed: {err}; retry in {backoff:?}"
                    );
                    tokio::time::sleep(backoff).await;
                }
                Err(err) => return Err(map_reqwest_error(err)),
            }
            attempt += 1;
        }
    }

    async fn ensure_parent_dirs(&self, remote_path: &str) -> Result<()> {
        let webdav_path = self.webdav_path(remote_path)?;
        let Some((parent, _)) = webdav_path.rsplit_once('/') else {
            return Ok(());
        };
        let mut current = String::new();
        for segment in parent.split('/').filter(|segment| !segment.is_empty()) {
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(segment);
            let url = self.url_for_path(&current)?;
            let response = self
                .send_with_retry("MKCOL", || {
                    Ok(self.apply_auth(self.client.request(mkcol_method()?, url.clone())))
                })
                .await?;
            match response.status() {
                StatusCode::CREATED
                | StatusCode::OK
                | StatusCode::METHOD_NOT_ALLOWED
                | StatusCode::CONFLICT => {}
                status => return Err(http_status_error(status, "MKCOL")),
            }
        }
        Ok(())
    }

    fn strip_root_from_href(&self, href: &str) -> Option<String> {
        let parsed = self.base_url.join(href).ok()?;
        let base_path = self.base_url.path().trim_matches('/');
        let mut path = parsed.path().trim_matches('/').to_string();
        if !base_path.is_empty() {
            path = path.strip_prefix(base_path)?.trim_matches('/').to_string();
        }
        if !self.root_dir.is_empty() {
            path = path
                .strip_prefix(&self.root_dir)?
                .trim_matches('/')
                .to_string();
        }
        if path.is_empty() { None } else { Some(path) }
    }
}

#[async_trait]
impl RemoteStorage for WebDavStorage {
    async fn exists(&self, remote_path: &str) -> Result<bool> {
        let response = self
            .send_with_retry("HEAD", || {
                Ok(self.apply_auth(self.client.head(self.url_for_remote_path(remote_path)?)))
            })
            .await?;
        match response.status() {
            status if status.is_success() => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            status => Err(http_status_error(status, "HEAD")),
        }
    }

    async fn read_object(&self, remote_path: &str) -> Result<Vec<u8>> {
        let response = self
            .send_with_retry("GET", || {
                Ok(self.apply_auth(self.client.get(self.url_for_remote_path(remote_path)?)))
            })
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Err(SyncError::Storage(format!(
                "object not found: {remote_path}"
            )));
        }
        if !response.status().is_success() {
            return Err(http_status_error(response.status(), "GET"));
        }
        Ok(response.bytes().await.map_err(map_reqwest_error)?.to_vec())
    }

    async fn read_object_optional(&self, remote_path: &str) -> Result<Option<Vec<u8>>> {
        let response = self
            .send_with_retry("GET", || {
                Ok(self.apply_auth(self.client.get(self.url_for_remote_path(remote_path)?)))
            })
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(http_status_error(response.status(), "GET"));
        }
        Ok(Some(
            response.bytes().await.map_err(map_reqwest_error)?.to_vec(),
        ))
    }

    async fn read_object_versioned(&self, remote_path: &str) -> Result<Option<VersionedObject>> {
        let response = self
            .send_with_retry("GET", || {
                Ok(self.apply_auth(self.client.get(self.url_for_remote_path(remote_path)?)))
            })
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(http_status_error(response.status(), "GET"));
        }
        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let data = response.bytes().await.map_err(map_reqwest_error)?.to_vec();
        Ok(Some(VersionedObject {
            data,
            version: RemoteVersion {
                etag,
                version: None,
            },
        }))
    }

    async fn write_object(&self, remote_path: &str, data: &[u8]) -> Result<()> {
        self.ensure_parent_dirs(remote_path).await?;
        let response = self
            .send_with_retry("PUT", || {
                Ok(self.apply_auth(
                    self.client
                        .put(self.url_for_remote_path(remote_path)?)
                        .body(data.to_vec()),
                ))
            })
            .await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(http_status_error(response.status(), "PUT"))
        }
    }

    async fn write_object_conditional(
        &self,
        remote_path: &str,
        data: &[u8],
        expected: Option<&RemoteVersion>,
    ) -> Result<bool> {
        self.ensure_parent_dirs(remote_path).await?;
        let expected_etag = match expected {
            None => None,
            Some(version) => Some(version.etag.as_deref().ok_or_else(|| {
                SyncError::Storage(
                    "WebDAV server did not provide an ETag required for safe concurrent sync"
                        .to_string(),
                )
            })?),
        };
        let response = self
            .send_with_retry("conditional PUT", || {
                let request = self
                    .client
                    .put(self.url_for_remote_path(remote_path)?)
                    .body(data.to_vec());
                let request = if let Some(etag) = expected_etag {
                    request.header(reqwest::header::IF_MATCH, etag)
                } else {
                    request.header(reqwest::header::IF_NONE_MATCH, "*")
                };
                Ok(self.apply_auth(request))
            })
            .await?;
        match response.status() {
            status if status.is_success() => Ok(true),
            StatusCode::PRECONDITION_FAILED | StatusCode::CONFLICT => Ok(false),
            status => Err(http_status_error(status, "conditional PUT")),
        }
    }

    async fn remove(&self, remote_path: &str) -> Result<()> {
        let response = self
            .send_with_retry("DELETE", || {
                Ok(self.apply_auth(self.client.delete(self.url_for_remote_path(remote_path)?)))
            })
            .await?;
        match response.status() {
            status if status.is_success() => Ok(()),
            StatusCode::NOT_FOUND => Ok(()),
            status => Err(http_status_error(status, "DELETE")),
        }
    }

    async fn list_prefix(&self, prefix: &str) -> Result<Vec<RemoteFileInfo>> {
        let response = self
            .send_with_retry("PROPFIND", || {
                Ok(self.apply_auth(
                    self.client
                        .request(propfind_method()?, self.url_for_remote_path(prefix)?)
                        .header("Depth", "infinity")
                        .header("Content-Type", "application/xml")
                        .body(propfind_body()),
                ))
            })
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        if !response.status().is_success() {
            return Err(http_status_error(response.status(), "PROPFIND"));
        }
        let xml = response.text().await.map_err(map_reqwest_error)?;
        let mut infos = parse_propfind(&xml, self)?;
        infos.retain(|info| info.path != prefix.trim_matches('/'));
        infos.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(infos)
    }

    async fn stat(&self, remote_path: &str) -> Result<RemoteFileInfo> {
        let response = self
            .send_with_retry("HEAD", || {
                Ok(self.apply_auth(self.client.head(self.url_for_remote_path(remote_path)?)))
            })
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Err(SyncError::Storage(format!(
                "object not found: {remote_path}"
            )));
        }
        if !response.status().is_success() {
            return Err(http_status_error(response.status(), "HEAD"));
        }
        Ok(RemoteFileInfo {
            path: remote_path.to_string(),
            size: response
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0),
            mtime: response
                .headers()
                .get(reqwest::header::LAST_MODIFIED)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_http_date_millis)
                .unwrap_or(0),
            etag: response
                .headers()
                .get(reqwest::header::ETAG)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
        })
    }

    async fn upload_large(
        &self,
        remote_path: &str,
        stream: Box<dyn AsyncRead + Unpin + Send>,
        _total_size: u64,
    ) -> Result<()> {
        self.ensure_parent_dirs(remote_path).await?;
        let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(stream));
        let response = self
            .apply_auth(
                self.client
                    .put(self.url_for_remote_path(remote_path)?)
                    .body(body),
            )
            .send()
            .await
            .map_err(map_reqwest_error)?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(http_status_error(response.status(), "streaming PUT"))
        }
    }

    async fn download_large(&self, remote_path: &str) -> Result<Box<dyn AsyncRead + Unpin + Send>> {
        let response = self
            .apply_auth(self.client.get(self.url_for_remote_path(remote_path)?))
            .send()
            .await
            .map_err(map_reqwest_error)?;
        if !response.status().is_success() {
            return Err(http_status_error(response.status(), "streaming GET"));
        }
        let stream = response.bytes_stream().map_err(std::io::Error::other);
        Ok(Box::new(tokio_util::io::StreamReader::new(stream)))
    }
}

fn parse_propfind(xml: &str, storage: &WebDavStorage) -> Result<Vec<RemoteFileInfo>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut infos = Vec::new();
    let mut current = PropfindEntry::default();
    let mut tag = String::new();
    let mut in_response = false;
    let mut in_collection = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                let name = String::from_utf8_lossy(element.local_name().as_ref()).to_string();
                if name == "response" {
                    in_response = true;
                    in_collection = false;
                    current = PropfindEntry::default();
                }
                if in_response {
                    if name == "collection" {
                        in_collection = true;
                    }
                    tag = name;
                }
            }
            Ok(Event::Empty(element)) if in_response => {
                let name = String::from_utf8_lossy(element.local_name().as_ref()).to_string();
                if name == "collection" {
                    in_collection = true;
                }
            }
            Ok(Event::Text(text)) if in_response => {
                let text = text
                    .unescape()
                    .map_err(|err| SyncError::Storage(err.to_string()))?;
                match tag.as_str() {
                    "href" => current.href = text.to_string(),
                    "getcontentlength" => current.size = text.parse::<u64>().unwrap_or(0),
                    "getlastmodified" => current.mtime = parse_http_date_millis(&text).unwrap_or(0),
                    "getetag" => current.etag = Some(text.to_string()),
                    _ => {}
                }
            }
            Ok(Event::End(element)) => {
                let name = String::from_utf8_lossy(element.local_name().as_ref()).to_string();
                if name == "response" {
                    if !in_collection
                        && let Some(path) = storage.strip_root_from_href(&current.href)
                    {
                        infos.push(RemoteFileInfo {
                            path,
                            size: current.size,
                            mtime: current.mtime,
                            etag: current.etag.clone(),
                        });
                    }
                    in_response = false;
                    tag.clear();
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(SyncError::Storage(err.to_string())),
            _ => {}
        }
    }
    Ok(infos)
}

#[derive(Default)]
struct PropfindEntry {
    href: String,
    size: u64,
    mtime: i64,
    etag: Option<String>,
}

fn propfind_body() -> &'static str {
    r#"<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:getcontentlength/>
    <D:getlastmodified/>
    <D:getetag/>
    <D:resourcetype/>
  </D:prop>
</D:propfind>"#
}

fn url_encode_segment(segment: &str) -> String {
    use std::fmt::Write;

    let mut encoded = String::new();
    for byte in segment.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char);
            }
            other => {
                let _ = write!(encoded, "%{other:02X}");
            }
        }
    }
    encoded
}

fn propfind_method() -> Result<Method> {
    Method::from_bytes(b"PROPFIND").map_err(|err| SyncError::Internal(err.to_string()))
}

fn mkcol_method() -> Result<Method> {
    Method::from_bytes(b"MKCOL").map_err(|err| SyncError::Internal(err.to_string()))
}

fn parse_http_date_millis(value: &str) -> Option<i64> {
    httpdate::parse_http_date(value)
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
}

fn map_reqwest_error(err: reqwest::Error) -> SyncError {
    if err.is_timeout() || err.is_connect() {
        SyncError::Network(err.to_string())
    } else {
        SyncError::Storage(err.to_string())
    }
}

fn is_retryable_reqwest_error(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect() || err.is_request()
}

fn is_retryable_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::TOO_MANY_REQUESTS
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    )
}

fn http_status_error(status: StatusCode, operation: &str) -> SyncError {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            SyncError::Storage(format!("{operation} unauthorized: HTTP {status}"))
        }
        StatusCode::TOO_MANY_REQUESTS
        | StatusCode::BAD_GATEWAY
        | StatusCode::SERVICE_UNAVAILABLE
        | StatusCode::GATEWAY_TIMEOUT => {
            SyncError::Network(format!("{operation} failed: HTTP {status}"))
        }
        other => SyncError::Storage(format!("{operation} failed: HTTP {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webdav_paths_are_normalized() {
        let storage = WebDavStorage::new(WebDavConfig {
            url: "http://127.0.0.1:8080".to_string(),
            username: Some("kmo".to_string()),
            password: Some("kmo".to_string()),
            root_dir: "kmo_sync/".to_string(),
        })
        .unwrap();

        assert_eq!(
            storage.webdav_path("metas/a.meta").unwrap(),
            "kmo_sync/metas/a.meta"
        );
        assert!(storage.webdav_path("../bad").is_err());
        assert!(storage.webdav_path("/bad").is_err());
    }

    #[test]
    fn propfind_parser_uses_mtime_size_without_etag() {
        let storage = WebDavStorage::new(WebDavConfig {
            url: "http://127.0.0.1:8080".to_string(),
            username: None,
            password: None,
            root_dir: "kmo_sync".to_string(),
        })
        .unwrap();
        let xml = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/kmo_sync/metas/</D:href>
    <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop></D:propstat>
  </D:response>
  <D:response>
    <D:href>/kmo_sync/metas/a.meta</D:href>
    <D:propstat><D:prop><D:getcontentlength>5</D:getcontentlength></D:prop></D:propstat>
  </D:response>
</D:multistatus>"#;

        let infos = parse_propfind(xml, &storage).unwrap();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].path, "metas/a.meta");
        assert_eq!(infos[0].size, 5);
        assert_eq!(infos[0].etag, None);
    }
}
