use kmo_sync::storage::RemoteStorage;
use kmo_sync::storage::webdav::{WebDavConfig, WebDavStorage};
use kmo_sync::{KmoSyncConfig, KmoSyncFacade};
use std::env;

fn webdav_config() -> WebDavConfig {
    WebDavConfig {
        url: env::var("KMO_WEBDAV_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".to_string()),
        username: env::var("KMO_WEBDAV_USERNAME").ok(),
        password: env::var("KMO_WEBDAV_PASSWORD").ok(),
        root_dir: format!("kmo_sync_test_{}", std::process::id()),
    }
}

#[tokio::test]
#[ignore = "requires WebDAV server from KMO_WEBDAV_* env vars"]
async fn webdav_storage_contract_basic_object_flow() {
    let config = webdav_config();
    let storage = WebDavStorage::new(config.clone()).unwrap();

    storage
        .write_object("metas/a.meta", b"hello")
        .await
        .unwrap();
    assert!(storage.exists("metas/a.meta").await.unwrap());
    assert_eq!(storage.read_object("metas/a.meta").await.unwrap(), b"hello");

    let listed = storage.list_prefix("metas").await.unwrap();
    assert!(listed.iter().any(|item| item.path == "metas/a.meta"));

    let stat = storage.stat("metas/a.meta").await.unwrap();
    assert_eq!(stat.size, 5);

    storage.remove("metas/a.meta").await.unwrap();
    assert!(!storage.exists("metas/a.meta").await.unwrap());
}

#[test]
#[ignore = "requires WebDAV server from KMO_WEBDAV_* env vars"]
fn meta_sync_webdav_two_devices() {
    let config = webdav_config();
    let cache_a = tempfile::tempdir().unwrap();
    let cache_b = tempfile::tempdir().unwrap();

    let username = config.username.unwrap_or_default();
    let password = config.password.unwrap_or_default();
    let storage_json = format!(
        r#"{{
            "type":"webdav",
            "url":"{}",
            "username":"{}",
            "password":"{}",
            "root_dir":"{}"
        }}"#,
        config.url, username, password, config.root_dir
    );

    let facade_a = KmoSyncFacade::create(
        KmoSyncConfig {
            storage_config_json: storage_json.clone(),
            encryption_config_json: r#"{"type":"none"}"#.to_string(),
            device_id: "device-a".to_string(),
            local_cache_dir: cache_a.path().to_path_buf(),
        },
        kmo_sync::event::EventEmitter::new(None, std::ptr::null_mut()),
    )
    .unwrap();
    let facade_b = KmoSyncFacade::create(
        KmoSyncConfig {
            storage_config_json: storage_json,
            encryption_config_json: r#"{"type":"none"}"#.to_string(),
            device_id: "device-b".to_string(),
            local_cache_dir: cache_b.path().to_path_buf(),
        },
        kmo_sync::event::EventEmitter::new(None, std::ptr::null_mut()),
    )
    .unwrap();

    let meta = kmo_sync::model::BookReadingMeta {
        meta_id: "meta-webdav-1".to_string(),
        book_hash: "book-webdav-1".to_string(),
        modified_ts: 1,
        device_id: "device-a".to_string(),
        progress: Some(kmo_sync::model::ReadingProgress {
            cfi_position: "epubcfi(/6/2)".to_string(),
            progress_percent: 0.42,
            chapter_id: "chapter-1".to_string(),
        }),
        bookmarks: vec![],
        highlights: vec![],
        notes: vec![],
        wall_clock_ts: 1,
        logical_ts: 1,
        origin_device_id: "device-a".to_string(),
        edit_history: vec![],
    };

    facade_a.put_local_meta(&meta).unwrap();
    facade_a
        .sync_single_meta("book-webdav-1", "meta-webdav-1")
        .unwrap();
    facade_b
        .sync_single_meta("book-webdav-1", "meta-webdav-1")
        .unwrap();

    let pulled = facade_b.get_local_meta_json("meta-webdav-1").unwrap();
    assert!(pulled.contains("\"progress_percent\":0.42"));
}
