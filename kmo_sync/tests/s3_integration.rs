use kmo_sync::storage::RemoteStorage;
use kmo_sync::storage::s3::{S3Config, S3Storage};
use kmo_sync::{KmoSyncConfig, KmoSyncFacade};
use std::env;

fn s3_config() -> S3Config {
    S3Config {
        endpoint: env::var("KMO_S3_ENDPOINT")
            .unwrap_or_else(|_| "http://127.0.0.1:9000".to_string()),
        bucket: env::var("KMO_S3_BUCKET").unwrap_or_else(|_| "kmo-test".to_string()),
        access_key: env::var("KMO_S3_ACCESS_KEY").unwrap_or_else(|_| "minioadmin".to_string()),
        secret_key: env::var("KMO_S3_SECRET_KEY").unwrap_or_else(|_| "minioadmin".to_string()),
        region: env::var("KMO_S3_REGION").unwrap_or_else(|_| "us-east-1".to_string()),
        root_prefix: format!("yuewei_kmo_sync_test_{}", std::process::id()),
        path_style: true,
        allow_http: true,
    }
}

#[tokio::test]
#[ignore = "requires MinIO/S3 and pre-created bucket from KMO_S3_* env vars"]
async fn s3_storage_contract_basic_object_flow() {
    let config = s3_config();
    let storage = S3Storage::new(config.clone()).unwrap();

    storage
        .write_object("metas/a.meta", b"hello")
        .await
        .unwrap();
    assert!(storage.exists("metas/a.meta").await.unwrap());
    assert_eq!(storage.read_object("metas/a.meta").await.unwrap(), b"hello");
    let stale = storage
        .read_object_versioned("metas/a.meta")
        .await
        .unwrap()
        .unwrap();
    assert!(
        storage
            .write_object_conditional("metas/a.meta", b"new", Some(&stale.version))
            .await
            .unwrap()
    );
    assert!(
        !storage
            .write_object_conditional("metas/a.meta", b"lost", Some(&stale.version))
            .await
            .unwrap()
    );

    let listed = storage.list_prefix("metas").await.unwrap();
    assert!(listed.iter().any(|item| item.path == "metas/a.meta"));

    let stat = storage.stat("metas/a.meta").await.unwrap();
    assert_eq!(stat.size, 3);

    storage.remove("metas/a.meta").await.unwrap();
    assert!(!storage.exists("metas/a.meta").await.unwrap());
}

#[test]
#[ignore = "requires MinIO/S3 and pre-created bucket from KMO_S3_* env vars"]
fn meta_sync_minio_two_devices() {
    let config = s3_config();
    let cache_a = tempfile::tempdir().unwrap();
    let cache_b = tempfile::tempdir().unwrap();

    let storage_json = format!(
        r#"{{
            "type":"s3",
            "endpoint":"{}",
            "bucket":"{}",
            "access_key":"{}",
            "secret_key":"{}",
            "region":"{}",
            "root_prefix":"{}",
            "path_style":true,
            "allow_http":true
        }}"#,
        config.endpoint,
        config.bucket,
        config.access_key,
        config.secret_key,
        config.region,
        config.root_prefix
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
        meta_id: "meta-minio-1".to_string(),
        book_hash: "book-minio-1".to_string(),
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
        bookmarks_write_ts: 1,
        bookmarks_writer_device: "device-a".to_string(),
        progress_write_ts: 1,
        progress_writer_device: "device-a".to_string(),
    };

    facade_a.put_local_meta(&meta).unwrap();
    facade_a
        .sync_single_meta("book-minio-1", "meta-minio-1")
        .unwrap();
    facade_b
        .sync_single_meta("book-minio-1", "meta-minio-1")
        .unwrap();

    let pulled = facade_b.get_local_meta_json("meta-minio-1").unwrap();
    assert!(pulled.contains("\"progress_percent\":0.42"));
}
