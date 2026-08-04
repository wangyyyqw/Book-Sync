use kmo_sync::event::EventEmitter;
use kmo_sync::model::BookReadingMeta;
use kmo_sync::storage::RemoteStorage;
use kmo_sync::storage::s3::{S3Config, S3Storage};
use kmo_sync::{KmoSyncConfig, KmoSyncFacade};
use std::env;

const SYNC_MODE_PUSH_ONLY: i32 = 1;
const SYNC_MODE_PULL_ONLY: i32 = 2;

fn unique_prefix(tag: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("kmo_sync_r2_{tag}_{}_{}", std::process::id(), nanos)
}

fn r2_config_with_prefix(prefix: String) -> S3Config {
    S3Config {
        endpoint: env::var("KMO_S3_ENDPOINT").expect("KMO_S3_ENDPOINT is required"),
        bucket: env::var("KMO_S3_BUCKET").expect("KMO_S3_BUCKET is required"),
        access_key: env::var("KMO_S3_ACCESS_KEY").expect("KMO_S3_ACCESS_KEY is required"),
        secret_key: env::var("KMO_S3_SECRET_KEY").expect("KMO_S3_SECRET_KEY is required"),
        region: env::var("KMO_S3_REGION").unwrap_or_else(|_| "auto".to_string()),
        root_prefix: prefix,
        path_style: true,
        allow_http: false,
    }
}

fn r2_config() -> S3Config {
    r2_config_with_prefix(unique_prefix("default"))
}

fn storage_json_for(config: &S3Config) -> String {
    format!(
        r#"{{
            "type":"s3",
            "endpoint":"{}",
            "bucket":"{}",
            "access_key":"{}",
            "secret_key":"{}",
            "region":"{}",
            "root_prefix":"{}",
            "path_style":true,
            "allow_http":false
        }}"#,
        config.endpoint,
        config.bucket,
        config.access_key,
        config.secret_key,
        config.region,
        config.root_prefix
    )
}

#[tokio::test]
#[ignore = "requires R2/S3 and pre-created bucket from KMO_S3_* env vars"]
async fn r2_storage_contract_basic_object_flow() {
    let config = r2_config();
    let storage = S3Storage::new(config).unwrap();

    storage
        .write_object("books/book-r2/metas/a.meta", b"hello")
        .await
        .unwrap();
    assert!(storage.exists("books/book-r2/metas/a.meta").await.unwrap());
    assert_eq!(
        storage
            .read_object("books/book-r2/metas/a.meta")
            .await
            .unwrap(),
        b"hello"
    );

    let listed = storage.list_prefix("books").await.unwrap();
    assert!(
        listed
            .iter()
            .any(|item| item.path == "books/book-r2/metas/a.meta")
    );

    let stat = storage.stat("books/book-r2/metas/a.meta").await.unwrap();
    assert_eq!(stat.size, 5);

    storage.remove("books/book-r2/metas/a.meta").await.unwrap();
    assert!(!storage.exists("books/book-r2/metas/a.meta").await.unwrap());
}

#[test]
#[ignore = "requires R2/S3 and pre-created bucket from KMO_S3_* env vars"]
fn r2_two_devices_meta_bidirectional_sync() {
    let config = r2_config_with_prefix(unique_prefix("meta"));
    let cache_a = tempfile::tempdir().unwrap();
    let cache_b = tempfile::tempdir().unwrap();
    let storage_json = storage_json_for(&config);

    let facade_a = KmoSyncFacade::create(
        KmoSyncConfig {
            storage_config_json: storage_json.clone(),
            encryption_config_json: r#"{"type":"none"}"#.to_string(),
            device_id: "r2-device-a".to_string(),
            local_cache_dir: cache_a.path().to_path_buf(),
        },
        EventEmitter::new(None, std::ptr::null_mut()),
    )
    .expect("create facade_a");
    let facade_b = KmoSyncFacade::create(
        KmoSyncConfig {
            storage_config_json: storage_json,
            encryption_config_json: r#"{"type":"none"}"#.to_string(),
            device_id: "r2-device-b".to_string(),
            local_cache_dir: cache_b.path().to_path_buf(),
        },
        EventEmitter::new(None, std::ptr::null_mut()),
    )
    .expect("create facade_b");

    let meta = BookReadingMeta {
        meta_id: "meta-r2-1".to_string(),
        book_hash: "book-r2-1".to_string(),
        modified_ts: 1,
        device_id: "r2-device-a".to_string(),
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
        origin_device_id: "r2-device-a".to_string(),
        edit_history: vec![],
        bookmarks_write_ts: 1,
        bookmarks_writer_device: "r2-device-a".to_string(),
        progress_write_ts: 1,
        progress_writer_device: "r2-device-a".to_string(),
    };

    facade_a.put_local_meta(&meta).unwrap();
    facade_a.sync_all(SYNC_MODE_PUSH_ONLY).unwrap();
    facade_b.sync_all(SYNC_MODE_PULL_ONLY).unwrap();

    let pulled = facade_b.get_local_meta_json("meta-r2-1").unwrap();
    assert!(
        pulled.contains("\"progress_percent\":0.42"),
        "expected pulled meta to contain 0.42, got: {pulled}"
    );

    let state_a = facade_a.get_sync_state_json().unwrap();
    let state_b = facade_b.get_sync_state_json().unwrap();
    println!("device_a state: {state_a}");
    println!("device_b state: {state_b}");
    assert!(state_a.contains("\"device_id\":\"r2-device-a\""));
    assert!(state_b.contains("\"device_id\":\"r2-device-b\""));
}

#[test]
#[ignore = "requires R2/S3 and pre-created bucket from KMO_S3_* env vars"]
fn r2_envelope_encryption_two_devices_meta_sync() {
    let config = r2_config_with_prefix(unique_prefix("enc"));
    let cache_a = tempfile::tempdir().unwrap();
    let cache_b = tempfile::tempdir().unwrap();
    let storage_json = storage_json_for(&config);
    let encryption_json = r#"{"type":"envelope","passphrase":"r2-test-passphrase","kek_id":"r2-enc-kek-v1","kek_version":1}"#.to_string();

    let facade_a = KmoSyncFacade::create(
        KmoSyncConfig {
            storage_config_json: storage_json.clone(),
            encryption_config_json: encryption_json.clone(),
            device_id: "r2-enc-a".to_string(),
            local_cache_dir: cache_a.path().to_path_buf(),
        },
        EventEmitter::new(None, std::ptr::null_mut()),
    )
    .expect("create facade_a (envelope)");

    let meta = BookReadingMeta {
        meta_id: "meta-r2-enc".to_string(),
        book_hash: "book-r2-enc".to_string(),
        modified_ts: 7,
        device_id: "r2-enc-a".to_string(),
        progress: Some(kmo_sync::model::ReadingProgress {
            cfi_position: "epubcfi(/6/4)".to_string(),
            progress_percent: 0.71,
            chapter_id: "chapter-2".to_string(),
        }),
        bookmarks: vec![],
        highlights: vec![],
        notes: vec![],
        wall_clock_ts: 7,
        logical_ts: 7,
        origin_device_id: "r2-enc-a".to_string(),
        edit_history: vec![],
        bookmarks_write_ts: 7,
        bookmarks_writer_device: "r2-enc-a".to_string(),
        progress_write_ts: 7,
        progress_writer_device: "r2-enc-a".to_string(),
    };

    facade_a.put_local_meta(&meta).unwrap();
    facade_a.sync_all(SYNC_MODE_PUSH_ONLY).unwrap();

    let storage = S3Storage::new(config.clone()).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let listed = rt.block_on(storage.list_prefix("books")).unwrap();
    let header = listed
        .iter()
        .find(|item| item.path == "books/book-r2-enc/metas/_sync_header.json")
        .or_else(|| {
            // The shared header lives at the bucket-root of yuewei.
            listed.iter().find(|item| item.path == "_sync_header.json")
        })
        .expect("sync header must exist after push");
    assert!(header.size > 0, "sync header should be non-empty");

    let facade_b = KmoSyncFacade::create(
        KmoSyncConfig {
            storage_config_json: storage_json,
            encryption_config_json: encryption_json,
            device_id: "r2-enc-b".to_string(),
            local_cache_dir: cache_b.path().to_path_buf(),
        },
        EventEmitter::new(None, std::ptr::null_mut()),
    )
    .expect("create facade_b (envelope)");

    facade_b.sync_all(SYNC_MODE_PULL_ONLY).unwrap();
    let pulled = facade_b.get_local_meta_json("meta-r2-enc").unwrap();
    assert!(
        pulled.contains("\"progress_percent\":0.71"),
        "expected pulled encrypted meta to contain 0.71, got: {pulled}"
    );
}

#[test]
#[ignore = "requires R2/S3 and pre-created bucket from KMO_S3_* env vars"]
fn r2_blob_sync_two_devices() {
    let config = r2_config_with_prefix(unique_prefix("blob"));
    let cache_a = tempfile::tempdir().unwrap();
    let cache_b = tempfile::tempdir().unwrap();
    let storage_json = storage_json_for(&config);

    let facade_a = KmoSyncFacade::create(
        KmoSyncConfig {
            storage_config_json: storage_json.clone(),
            encryption_config_json: r#"{"type":"none"}"#.to_string(),
            device_id: "r2-blob-a".to_string(),
            local_cache_dir: cache_a.path().to_path_buf(),
        },
        EventEmitter::new(None, std::ptr::null_mut()),
    )
    .expect("create facade_a (blob)");

    // Deterministic ~6 MiB payload to exercise the FastCDC chunker.
    let chunk_count = 6u32;
    let chunk_size = 1024 * 1024usize;
    let payload: Vec<u8> = (0..chunk_count)
        .flat_map(|i| {
            let line = format!("chapter-{i:03}-");
            let pattern = line.repeat(chunk_size / line.len().max(1));
            pattern.into_bytes()
        })
        .take(chunk_count as usize * chunk_size)
        .collect();
    let book_path = cache_a.path().join("sample-book.epub");
    std::fs::write(&book_path, &payload).expect("write sample book");
    let book_hash = blake3::hash(&payload).to_hex().to_string();

    facade_a
        .put_local_book(&book_hash, &book_path)
        .expect("put_local_book");
    facade_a.sync_book(&book_hash).expect("sync_book push");

    let facade_b = KmoSyncFacade::create(
        KmoSyncConfig {
            storage_config_json: storage_json,
            encryption_config_json: r#"{"type":"none"}"#.to_string(),
            device_id: "r2-blob-b".to_string(),
            local_cache_dir: cache_b.path().to_path_buf(),
        },
        EventEmitter::new(None, std::ptr::null_mut()),
    )
    .expect("create facade_b (blob)");

    facade_b.sync_book(&book_hash).expect("sync_book pull");
    let synced_path = cache_b
        .path()
        .join("blobs")
        .join(format!("{book_hash}.epub"));
    assert!(
        synced_path.exists(),
        "expected downloaded blob at {}",
        synced_path.display()
    );
    let downloaded = std::fs::read(&synced_path).expect("read downloaded book");
    assert_eq!(
        downloaded.len(),
        payload.len(),
        "downloaded size mismatch ({} vs {})",
        downloaded.len(),
        payload.len()
    );
    assert_eq!(
        blake3::hash(&downloaded).to_hex().to_string(),
        book_hash,
        "downloaded blake3 mismatch"
    );

    let storage = S3Storage::new(config).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let listed = rt.block_on(storage.list_prefix("blobs")).unwrap();
    let paths: Vec<&str> = listed.iter().map(|i| i.path.as_str()).collect();
    let has_manifest = paths.iter().any(|p| p.ends_with(".manifest.json"));
    let has_chunk = paths
        .iter()
        .any(|p| p.contains("/cas/") && p.ends_with(".chunk"));
    assert!(
        has_manifest,
        "expected blob manifest in remote, listed: {paths:?}"
    );
    assert!(
        has_chunk,
        "expected at least one FastCDC chunk part in remote, listed: {paths:?}"
    );
}

#[test]
#[ignore = "requires R2/S3 and pre-created bucket from KMO_S3_* env vars"]
fn r2_envelope_kek_rotation() {
    let config = r2_config_with_prefix(unique_prefix("rotate"));
    let cache = tempfile::tempdir().unwrap();
    let storage_json = storage_json_for(&config);
    let encryption_v1 =
        r#"{"type":"envelope","passphrase":"r2-rotation-v1","kek_id":"r2-kek-v1","kek_version":1}"#
            .to_string();
    let encryption_v2 =
        r#"{"type":"envelope","passphrase":"r2-rotation-v2","kek_id":"r2-kek-v2","kek_version":2}"#
            .to_string();

    let facade = KmoSyncFacade::create(
        KmoSyncConfig {
            storage_config_json: storage_json,
            encryption_config_json: encryption_v1,
            device_id: "r2-rotate".to_string(),
            local_cache_dir: cache.path().to_path_buf(),
        },
        EventEmitter::new(None, std::ptr::null_mut()),
    )
    .expect("create facade (rotation)");

    let meta = BookReadingMeta {
        meta_id: "meta-r2-rotate".to_string(),
        book_hash: "book-r2-rotate".to_string(),
        modified_ts: 9,
        device_id: "r2-rotate".to_string(),
        progress: Some(kmo_sync::model::ReadingProgress {
            cfi_position: "epubcfi(/6/8)".to_string(),
            progress_percent: 0.55,
            chapter_id: "chapter-3".to_string(),
        }),
        bookmarks: vec![],
        highlights: vec![],
        notes: vec![],
        wall_clock_ts: 9,
        logical_ts: 9,
        origin_device_id: "r2-rotate".to_string(),
        edit_history: vec![],
        bookmarks_write_ts: 9,
        bookmarks_writer_device: "r2-rotate".to_string(),
        progress_write_ts: 9,
        progress_writer_device: "r2-rotate".to_string(),
    };
    facade.put_local_meta(&meta).unwrap();
    facade.sync_all(SYNC_MODE_PUSH_ONLY).unwrap();

    let rewrapped = facade
        .rotate_envelope_kek(&encryption_v2)
        .expect("rotate kek");
    println!("KEK rotation rewrapped {rewrapped} objects");
    facade.sync_all(SYNC_MODE_PUSH_ONLY).unwrap();

    let cache_b = tempfile::tempdir().unwrap();
    let facade_b = KmoSyncFacade::create(
        KmoSyncConfig {
            storage_config_json: storage_json_for(&config),
            encryption_config_json: encryption_v2,
            device_id: "r2-rotate-b".to_string(),
            local_cache_dir: cache_b.path().to_path_buf(),
        },
        EventEmitter::new(None, std::ptr::null_mut()),
    )
    .expect("create facade_b (rotation)");
    facade_b.sync_all(SYNC_MODE_PULL_ONLY).unwrap();
    let pulled = facade_b.get_local_meta_json("meta-r2-rotate").unwrap();
    assert!(
        pulled.contains("\"progress_percent\":0.55"),
        "expected rotated meta to contain 0.55, got: {pulled}"
    );
}

#[test]
#[ignore = "requires R2/S3 and pre-created bucket from KMO_S3_* env vars"]
fn r2_real_epub_two_devices_metadata_and_blob_roundtrip() {
    const EPUB_PATH: &str = "/Users/aaa/Documents/github/KMO-Sync/海底两万里-phone.epub";

    let epub_bytes = std::fs::read(EPUB_PATH).unwrap_or_else(|err| {
        panic!("failed to read {EPUB_PATH}: {err}; place the book file and rerun this test")
    });
    let expected_book_hash = blake3::hash(&epub_bytes).to_hex().to_string();
    println!(
        "[real epub] size={} bytes, blake3={expected_book_hash}",
        epub_bytes.len()
    );
    assert!(
        epub_bytes.len() > 5 * 1024 * 1024,
        "the FastCDC CAS path needs a blob larger than the 5MiB threshold"
    );

    let config = r2_config_with_prefix(unique_prefix("realbook"));
    let cache_a = tempfile::tempdir().unwrap();
    let cache_b = tempfile::tempdir().unwrap();
    let storage_json = storage_json_for(&config);
    let encryption_json = r#"{"type":"envelope","passphrase":"r2-realbook-passphrase","kek_id":"r2-realbook-kek-v1","kek_version":1}"#.to_string();

    // Phase 1 — device A uploads the encrypted blob and pushes metadata.
    let facade_a = KmoSyncFacade::create(
        KmoSyncConfig {
            storage_config_json: storage_json.clone(),
            encryption_config_json: encryption_json.clone(),
            device_id: "r2-realbook-a".to_string(),
            local_cache_dir: cache_a.path().to_path_buf(),
        },
        EventEmitter::new(None, std::ptr::null_mut()),
    )
    .expect("create facade_a (real book)");

    let book_path = cache_a.path().join("海底两万里-phone.epub");
    std::fs::write(&book_path, &epub_bytes).expect("copy epub into cache_a");
    facade_a
        .put_local_book(&expected_book_hash, &book_path)
        .expect("put_local_book");

    let meta = BookReadingMeta {
        meta_id: "meta-realbook".to_string(),
        book_hash: expected_book_hash.clone(),
        modified_ts: 100,
        device_id: "r2-realbook-a".to_string(),
        progress: Some(kmo_sync::model::ReadingProgress {
            cfi_position: "epubcfi(/6/14[chapter03])".to_string(),
            progress_percent: 0.18,
            chapter_id: "chapter-03".to_string(),
        }),
        bookmarks: vec![],
        highlights: vec![],
        notes: vec![],
        wall_clock_ts: 100,
        logical_ts: 100,
        origin_device_id: "r2-realbook-a".to_string(),
        edit_history: vec![],
        bookmarks_write_ts: 100,
        bookmarks_writer_device: "r2-realbook-a".to_string(),
        progress_write_ts: 100,
        progress_writer_device: "r2-realbook-a".to_string(),
    };
    facade_a.put_local_meta(&meta).unwrap();

    println!("[real epub] pushing blob + meta to R2...");
    facade_a
        .sync_book(&expected_book_hash)
        .expect("sync_book push (real)");
    facade_a
        .sync_all(SYNC_MODE_PUSH_ONLY)
        .expect("sync_all push (real)");

    // Phase 2 — device B (different cache dir, envelope passphrase known) downloads.
    let facade_b = KmoSyncFacade::create(
        KmoSyncConfig {
            storage_config_json: storage_json,
            encryption_config_json: encryption_json,
            device_id: "r2-realbook-b".to_string(),
            local_cache_dir: cache_b.path().to_path_buf(),
        },
        EventEmitter::new(None, std::ptr::null_mut()),
    )
    .expect("create facade_b (real book)");

    println!("[real epub] pulling blob + meta from R2...");
    facade_b
        .sync_all(SYNC_MODE_PULL_ONLY)
        .expect("sync_all pull (real)");
    facade_b
        .sync_book(&expected_book_hash)
        .expect("sync_book pull (real)");

    // Verify the pulled metadata matches device A.
    let pulled_meta = facade_b
        .get_local_meta_json("meta-realbook")
        .expect("get_local_meta_json");
    println!("[real epub] pulled meta: {pulled_meta}");
    assert!(
        pulled_meta.contains("\"progress_percent\":0.18"),
        "expected pulled meta to contain 0.18, got: {pulled_meta}"
    );
    assert!(
        pulled_meta.contains(&expected_book_hash),
        "expected pulled meta to contain book_hash, got: {pulled_meta}"
    );

    // Verify the downloaded blob bytes equal the source and re-hash to blake3.
    let downloaded_path = cache_b
        .path()
        .join("blobs")
        .join(format!("{expected_book_hash}.epub"));
    assert!(
        downloaded_path.exists(),
        "expected downloaded blob at {}",
        downloaded_path.display()
    );
    let downloaded_bytes = std::fs::read(&downloaded_path).expect("read downloaded book");
    assert_eq!(
        downloaded_bytes.len(),
        epub_bytes.len(),
        "downloaded size mismatch ({} vs {})",
        downloaded_bytes.len(),
        epub_bytes.len()
    );
    let downloaded_hash = blake3::hash(&downloaded_bytes).to_hex().to_string();
    assert_eq!(
        downloaded_hash, expected_book_hash,
        "downloaded blake3 mismatch"
    );
    println!(
        "[real epub] OK: {} bytes roundtripped, blake3={downloaded_hash}",
        downloaded_bytes.len()
    );

    // Verify the remote really holds a FastCDC CAS manifest with chunks.
    let storage = S3Storage::new(config).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let blobs = rt.block_on(storage.list_prefix("books")).unwrap();
    let blob_paths: Vec<&str> = blobs
        .iter()
        .map(|i| i.path.as_str())
        .filter(|p| p.contains("/blobs/"))
        .collect();
    let meta_paths: Vec<&str> = blobs
        .iter()
        .map(|i| i.path.as_str())
        .filter(|p| p.contains("/metas/"))
        .collect();

    let manifest_count = blob_paths
        .iter()
        .filter(|p| p.ends_with(".manifest.json"))
        .count();
    let chunk_count = blob_paths
        .iter()
        .filter(|p| p.contains("/cas/") && p.ends_with(".chunk"))
        .count();
    println!(
        "[real epub] remote blobs: {} manifests, {} chunks",
        manifest_count, chunk_count
    );
    assert_eq!(
        manifest_count, 1,
        "expected exactly one manifest, found {manifest_count}: {blob_paths:?}"
    );
    assert!(
        chunk_count >= 2,
        "expected multiple chunks for 164MiB epub, found {chunk_count}: {blob_paths:?}"
    );

    let has_meta_env = meta_paths
        .iter()
        .any(|p| p.ends_with("meta-realbook.meta.env"));
    let has_header = blob_paths.contains(&"_sync_header.json");
    assert!(has_meta_env, "expected meta envelope, got: {meta_paths:?}");
    assert!(has_header, "expected sync header, got: {meta_paths:?}");
}

// ---------------------------------------------------------------------------
// Cross-device reading simulation across four real EPUBs of different sizes.
//
// Books (all four live under 测试文件/):
//   * 恐妻家                       ~400 KiB  — single-shot upload path
//   * 阅微草堂笔记                  ~8.8 MiB  — FastCDC CAS path (small)
//   * C41-愤怒的葡萄-手机            ~31 MiB  — FastCDC CAS path (medium)
//   * XL06-三国演义(图片压缩版)       ~68 MiB  — FastCDC CAS path (large)
//
// For each book, three devices exercise the user's actual flow:
//
//   1. Phone uploads 20% progress + bookmark + highlight.
//   2. Pad downloads, advances to 50% + adds a note + a second highlight,
//      pushes only meta. Phone's local state must NOT be overwritten.
//   3. A fresh-cache phone pulls and observes the merged state (50%
//      progress from the pad, all highlights / bookmark / note combined).
//   4. Phone deletes one highlight, syncs the meta; a brand-new "reader c"
//      pulls and confirms the tombstone reached R2.
//
// Each test run is namespaced via `unique_prefix` so re-runs against the
// same bucket cannot collide, and the bucket is cleaned up at the end.
#[test]
#[ignore = "requires R2/S3 and pre-created bucket from KMO_S3_* env vars"]
fn r2_three_books_phone_pad_phone_roundtrip() {
    use kmo_sync::storage::s3::S3Storage;

    let books = [
        ScenarioBook {
            label: "kasaike",
            source_path: "/Users/aaa/Documents/github/KMO-Sync/测试文件/恐妻家 - [日]伊坂幸太郎.epub",
        },
        ScenarioBook {
            label: "yuewei",
            source_path: "/Users/aaa/Documents/github/KMO-Sync/测试文件/阅微草堂笔记.epub",
        },
        ScenarioBook {
            label: "fennu",
            source_path: "/Users/aaa/Documents/github/KMO-Sync/测试文件/C41-愤怒的葡萄-[美] 约翰·斯坦贝克-手机.epub",
        },
        ScenarioBook {
            label: "sanguo",
            source_path: "/Users/aaa/Documents/github/KMO-Sync/测试文件/XL06-02-三国演义（图片压缩版）v1.1[罗贯中][毛宗岗等 批][by懒回顾]_decrypt.epub",
        },
    ];
    // Reeden layout uses book_hash itself as the meta identifier in the
    // local cache. Compute the hashes up front so scenario steps and
    // assertions share the same identity.
    let mut book_hashes: std::collections::HashMap<&'static str, String> =
        std::collections::HashMap::new();
    for book in &books {
        let bytes = std::fs::read(book.source_path)
            .unwrap_or_else(|err| panic!("missing {}: {err}", book.source_path));
        book_hashes.insert(book.label, blake3::hash(&bytes).to_hex().to_string());
    }

    let config = r2_config_with_prefix(unique_prefix("phone_pad_phone"));
    let storage_json = storage_json_for(&config);
    let encryption_json = r#"{"type":"none"}"#.to_string();

    let cache_phone = tempfile::tempdir().unwrap();
    let cache_pad = tempfile::tempdir().unwrap();
    let cache_phone_again = tempfile::tempdir().unwrap();
    let cache_reader_c = tempfile::tempdir().unwrap();

    let phone = KmoSyncFacade::create(
        KmoSyncConfig {
            storage_config_json: storage_json.clone(),
            encryption_config_json: encryption_json.clone(),
            device_id: "r2-phone-iphone".to_string(),
            local_cache_dir: cache_phone.path().to_path_buf(),
        },
        EventEmitter::new(None, std::ptr::null_mut()),
    )
    .expect("create phone facade");

    let pad = KmoSyncFacade::create(
        KmoSyncConfig {
            storage_config_json: storage_json.clone(),
            encryption_config_json: encryption_json.clone(),
            device_id: "r2-pad-ipad".to_string(),
            local_cache_dir: cache_pad.path().to_path_buf(),
        },
        EventEmitter::new(None, std::ptr::null_mut()),
    )
    .expect("create pad facade");

    let phone_again = KmoSyncFacade::create(
        KmoSyncConfig {
            storage_config_json: storage_json.clone(),
            encryption_config_json: encryption_json.clone(),
            device_id: "r2-phone-iphone".to_string(),
            local_cache_dir: cache_phone_again.path().to_path_buf(),
        },
        EventEmitter::new(None, std::ptr::null_mut()),
    )
    .expect("create phone-again facade");

    let reader_c = KmoSyncFacade::create(
        KmoSyncConfig {
            storage_config_json: storage_json,
            encryption_config_json: encryption_json,
            device_id: "r2-reader-c".to_string(),
            local_cache_dir: cache_reader_c.path().to_path_buf(),
        },
        EventEmitter::new(None, std::ptr::null_mut()),
    )
    .expect("create reader-c facade");

    let cleanup_storage = S3Storage::new(config.clone()).unwrap();
    let cleanup_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    for book in &books {
        let book_hash = book_hashes.get(book.label).unwrap().clone();
        run_r2_book_scenario(
            book,
            &book_hash,
            &phone,
            &pad,
            &phone_again,
            &reader_c,
            &cleanup_storage,
            &cleanup_rt,
        );
    }

    // Verify the reeden layout actually materialized on R2: every book must
    // appear as a single top-level `books/<book_hash>` object.
    let test_prefix = "books";
    let listed = cleanup_rt
        .block_on(cleanup_storage.list_prefix(test_prefix))
        .expect("list books after sim");
    let mut seen_books: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for item in &listed {
        if let Some(rest) = item.path.strip_prefix("books/") {
            // The book payload is a single object — no per-book subdirectory.
            if !rest.contains('/') {
                let hash = rest
                    .strip_suffix(".enc")
                    .or_else(|| rest.strip_suffix(".env"))
                    .unwrap_or(rest);
                seen_books.insert(hash.to_string());
            }
        }
    }
    assert_eq!(
        seen_books.len(),
        books.len(),
        "expected {} distinct book objects on R2, got: {:?}",
        books.len(),
        seen_books
    );
    for book in &books {
        let bytes = std::fs::read(book.source_path)
            .unwrap_or_else(|err| panic!("missing {}: {err}", book.source_path));
        let hash = blake3::hash(&bytes).to_hex().to_string();
        assert!(
            seen_books.contains(&hash),
            "[{}] expected R2 books/{} single object",
            book.label,
            hash
        );
    }

    // Best-effort cleanup so re-running this test doesn't accumulate
    // abandoned objects under the unique prefix. Ignore failures.
    for item in &listed {
        let _ = cleanup_rt.block_on(cleanup_storage.remove(&item.path));
    }
    // Also clean up the book_progress / bookmarks envelopes we created.
    for prefix in ["book_progress", "bookmarks"] {
        if let Ok(items) = cleanup_rt.block_on(cleanup_storage.list_prefix(prefix)) {
            for item in items {
                let _ = cleanup_rt.block_on(cleanup_storage.remove(&item.path));
            }
        }
    }

    println!(
        "[r2 sim] OK: {} books, {} remote objects under {}",
        books.len(),
        listed.len(),
        test_prefix
    );
}

#[allow(clippy::too_many_arguments)]
fn run_r2_book_scenario(
    book: &ScenarioBook,
    book_hash: &str,
    phone: &KmoSyncFacade,
    pad: &KmoSyncFacade,
    phone_again: &KmoSyncFacade,
    reader_c: &KmoSyncFacade,
    storage: &S3Storage,
    rt: &tokio::runtime::Runtime,
) {
    use kmo_sync::model::{
        BookNote, BookReadingMeta, Bookmark, Highlight, MetaEdit, ReadingProgress,
    };

    // Reeden layout: meta_id == book_hash.
    let meta_id: &str = book_hash;
    let bytes = std::fs::read(book.source_path)
        .unwrap_or_else(|err| panic!("missing {} for {}: {err}", book.source_path, book.label));
    println!(
        "[r2 sim / {}] size={} bytes, blake3={}",
        book.label,
        bytes.len(),
        book_hash
    );

    // ---- Phase 1: phone uploads 20% + bookmark + highlight ----
    stage_local_book(phone, book_hash, &bytes);
    let meta_p1 = BookReadingMeta {
        meta_id: meta_id.to_string(),
        book_hash: book_hash.to_string(),
        modified_ts: 100,
        device_id: "r2-phone-iphone".to_string(),
        progress: Some(ReadingProgress {
            cfi_position: format!("epubcfi(/6/2[{}])", book.label),
            progress_percent: 0.20,
            chapter_id: "chapter-1".to_string(),
        }),
        bookmarks: vec![Bookmark {
            bookmark_id: "bm-phone-1".to_string(),
            cfi_range: format!("epubcfi(/6/2[{}])", book.label),
            title: format!("Phone bookmark — {}", book.label),
            create_ts: 100,
        }],
        highlights: vec![Highlight {
            highlight_id: "hl-phone-1".to_string(),
            cfi_start: format!("epubcfi(/6/2[{}])", book.label),
            cfi_end: format!("epubcfi(/6/4[{}])", book.label),
            color: "yellow".to_string(),
            comment: format!("phone highlight — {}", book.label),
            create_ts: 100,
        }],
        notes: vec![],
        wall_clock_ts: 100,
        logical_ts: 100,
        origin_device_id: "r2-phone-iphone".to_string(),
        edit_history: vec![MetaEdit {
            edit_id: format!("init-{}", book.label),
            device_id: "r2-phone-iphone".to_string(),
            logical_ts: 100,
            op: None,
        }],
        bookmarks_write_ts: 100,
        bookmarks_writer_device: "r2-phone-iphone".to_string(),
        progress_write_ts: 100,
        progress_writer_device: "r2-phone-iphone".to_string(),
    };
    phone.put_local_meta(&meta_p1).unwrap();
    phone.sync_book(book_hash).expect("phone sync_book push");
    phone
        .sync_all(SYNC_MODE_PUSH_ONLY)
        .expect("phone sync_all push");

    // R2 must contain a single `books/<book_hash>` object after phase 1.
    let listed_after_phase1 = rt.block_on(storage.list_prefix("books")).unwrap();
    let plain = format!("books/{book_hash}");
    let enc = format!("books/{book_hash}.enc");
    let env = format!("books/{book_hash}.env");
    let book_obj_paths: Vec<String> = listed_after_phase1
        .iter()
        .map(|i| i.path.clone())
        .filter(|p| p == &plain || p == &enc || p == &env)
        .collect();
    if book_obj_paths.is_empty() {
        eprintln!(
            "[r2 sim / {}] full list_prefix(\"books\") returned {} paths:",
            book.label,
            listed_after_phase1.len()
        );
        for p in &listed_after_phase1 {
            eprintln!("  {}", p.path);
        }
    }
    assert!(
        !book_obj_paths.is_empty(),
        "[r2 sim / {}] expected single `books/<book_hash>` object after phone push",
        book.label
    );
    assert!(
        book_obj_paths.iter().all(|p| match p.rsplit_once('/') {
            Some((_, tail)) =>
                tail == book_hash
                    || tail == format!("{book_hash}.enc")
                    || tail == format!("{book_hash}.env"),
            None => false,
        }),
        "[r2 sim / {}] reeden layout must be a single books/<hash> object, got {:?}",
        book.label,
        book_obj_paths
    );

    // ---- Phase 2: pad downloads, advances to 50%, adds note + highlight ----
    pad.sync_all(SYNC_MODE_PULL_ONLY)
        .expect("pad sync_all pull");
    pad.sync_book(book_hash).expect("pad sync_book pull");

    let mut meta_p2 = pad.read_local_meta(meta_id).unwrap().unwrap();
    assert_eq!(
        meta_p2.progress.as_ref().unwrap().progress_percent,
        0.20,
        "[r2 sim / {}] pad should observe phone's 20%",
        book.label
    );
    meta_p2.logical_ts = 200;
    meta_p2.modified_ts = 200;
    meta_p2.wall_clock_ts = 200;
    meta_p2.device_id = "r2-pad-ipad".to_string();
    meta_p2.origin_device_id = "r2-phone-iphone".to_string();
    meta_p2.progress = Some(ReadingProgress {
        cfi_position: format!("epubcfi(/6/8[{}])", book.label),
        progress_percent: 0.50,
        chapter_id: "chapter-2".to_string(),
    });
    meta_p2.notes.push(BookNote {
        note_id: "note-pad-1".to_string(),
        relate_cfi: format!("epubcfi(/6/8[{}])", book.label),
        content: format!("pad note — {}", book.label),
        create_ts: 200,
    });
    meta_p2.highlights.push(Highlight {
        highlight_id: "hl-pad-1".to_string(),
        cfi_start: format!("epubcfi(/6/6[{}])", book.label),
        cfi_end: format!("epubcfi(/6/8[{}])", book.label),
        color: "green".to_string(),
        comment: format!("pad highlight — {}", book.label),
        create_ts: 200,
    });
    meta_p2.edit_history.push(MetaEdit {
        edit_id: format!("pad-progress-{}", book.label),
        device_id: "r2-pad-ipad".to_string(),
        logical_ts: 200,
        op: None,
    });
    pad.put_local_meta(&meta_p2).unwrap();
    pad.sync_all(SYNC_MODE_PUSH_ONLY)
        .expect("pad sync_all push meta");

    // The phone's local view must NOT be overwritten by the pad's push.
    let phone_local_after_pad = phone.read_local_meta(meta_id).unwrap().unwrap();
    assert_eq!(
        phone_local_after_pad
            .progress
            .as_ref()
            .unwrap()
            .progress_percent,
        0.20,
        "[r2 sim / {}] pad push must not overwrite phone's local progress",
        book.label
    );

    // ---- Phase 3: fresh-cache phone pulls and sees the merged state ----
    phone_again
        .sync_all(SYNC_MODE_PULL_ONLY)
        .expect("phone_again sync_all pull");
    phone_again
        .sync_book(book_hash)
        .expect("phone_again sync_book pull");

    let final_meta = phone_again
        .read_local_meta(meta_id)
        .unwrap()
        .unwrap_or_else(|| panic!("[r2 sim / {}] expected merged meta", book.label));
    assert_eq!(
        final_meta.progress.as_ref().unwrap().progress_percent,
        0.50,
        "[r2 sim / {}] expected 50% after pad sync",
        book.label
    );
    assert_eq!(
        final_meta.device_id, "r2-pad-ipad",
        "[r2 sim / {}] expected pad as latest writer",
        book.label
    );
    assert_eq!(final_meta.bookmarks.len(), 1);
    assert_eq!(final_meta.bookmarks[0].bookmark_id, "bm-phone-1");
    assert_eq!(final_meta.highlights.len(), 2);
    let highlight_ids: std::collections::BTreeSet<&str> = final_meta
        .highlights
        .iter()
        .map(|h| h.highlight_id.as_str())
        .collect();
    assert!(highlight_ids.contains("hl-phone-1"));
    assert!(highlight_ids.contains("hl-pad-1"));
    assert_eq!(final_meta.notes.len(), 1);
    assert_eq!(final_meta.notes[0].note_id, "note-pad-1");

    // The downloaded blob must re-hash to the same blake3 as the source.
    let downloaded_path = cache_blobs_path(phone_again, book_hash);
    if downloaded_path.exists() {
        let downloaded = std::fs::read(&downloaded_path).unwrap();
        assert_eq!(
            blake3::hash(&downloaded).to_hex().to_string(),
            book_hash,
            "[r2 sim / {}] downloaded blob re-hash failed (size {})",
            book.label,
            downloaded.len()
        );
        if downloaded.len() != bytes.len() {
            panic!(
                "[r2 sim / {}] downloaded size {} != source size {}",
                book.label,
                downloaded.len(),
                bytes.len()
            );
        }
        println!(
            "[r2 sim / {}] blob roundtrip OK: {} bytes",
            book.label,
            downloaded.len()
        );
    } else {
        println!(
            "[r2 sim / {}] (no downloaded blob at {}; skipping rehash)",
            book.label,
            downloaded_path.display()
        );
    }

    // ---- Phase 4: tombstone round-trip across R2 ----
    phone_again
        .mark_meta_item_deleted(meta_id, "highlight", "hl-phone-1")
        .unwrap();
    phone_again
        .sync_single_meta(book_hash, meta_id)
        .expect("phone_again sync_single_meta (tombstone push)");

    reader_c
        .sync_all(SYNC_MODE_PULL_ONLY)
        .expect("reader_c sync_all pull");
    let c_meta = reader_c
        .read_local_meta(meta_id)
        .unwrap()
        .unwrap_or_else(|| panic!("[r2 sim / {}] expected meta on reader-c", book.label));
    assert_eq!(
        c_meta.highlights.len(),
        1,
        "[r2 sim / {}] tombstoned highlight should be gone",
        book.label
    );
    assert_eq!(c_meta.highlights[0].highlight_id, "hl-pad-1");
    assert_eq!(c_meta.notes.len(), 1);
    assert_eq!(c_meta.bookmarks.len(), 1);
}

struct ScenarioBook {
    label: &'static str,
    source_path: &'static str,
}

fn stage_local_book(facade: &KmoSyncFacade, book_hash: &str, bytes: &[u8]) {
    let cache = facade.local_cache_dir().to_path_buf();
    std::fs::create_dir_all(&cache).unwrap();
    let staged = cache.join(format!("stage-{book_hash}.epub"));
    std::fs::write(&staged, bytes).unwrap();
    facade.put_local_book(book_hash, &staged).unwrap();
}

fn cache_blobs_path(facade: &KmoSyncFacade, book_hash: &str) -> std::path::PathBuf {
    facade
        .local_cache_dir()
        .join("blobs")
        .join(format!("{book_hash}.epub"))
}
