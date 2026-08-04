//! 模拟 Android demo 重复运行：缓存/远端保留，第二轮用回拨的固定时间戳。
//!
//! 复现真机上的两个失败：
//!   1. 平板拉不到手机"新"进度（远端残留的旧进度带更大的时间戳，LWW 胜出）
//!   2. 删除 tombstone 失效（meta.logical_ts 被回拨，新 tombstone 的时间戳
//!      小于旧 revival，is_revived 误判为"已复活"）

use kmo_sync::event::EventEmitter;
use kmo_sync::model::{BookReadingMeta, Bookmark, ReadingProgress};
use kmo_sync::{KmoSyncConfig, KmoSyncFacade};

fn meta(progress: f64, ts: i64, device: &str, bookmarks: Vec<Bookmark>) -> BookReadingMeta {
    BookReadingMeta {
        meta_id: "book-santi".to_string(),
        book_hash: "book-santi".to_string(),
        modified_ts: ts,
        device_id: device.to_string(),
        progress: Some(ReadingProgress {
            cfi_position: "gzh://book-santi".to_string(),
            progress_percent: progress,
            chapter_id: "3".to_string(),
        }),
        bookmarks,
        highlights: vec![],
        notes: vec![],
        wall_clock_ts: ts,
        logical_ts: ts,
        origin_device_id: device.to_string(),
        edit_history: vec![],
        progress_write_ts: ts,
        progress_writer_device: device.to_string(),
        bookmarks_write_ts: ts,
        bookmarks_writer_device: device.to_string(),
    }
}

fn bm(id: &str, title: &str, ts: i64) -> Bookmark {
    Bookmark {
        bookmark_id: id.to_string(),
        cfi_range: "range".to_string(),
        title: title.to_string(),
        create_ts: ts,
    }
}

fn facade(cache: &std::path::Path, remote: &std::path::Path, device: &str) -> KmoSyncFacade {
    let config = KmoSyncConfig {
        storage_config_json: format!(
            r#"{{"type":"file","root_dir":"{}"}}"#,
            remote.to_string_lossy()
        ),
        encryption_config_json: r#"{"type":"none"}"#.to_string(),
        device_id: device.to_string(),
        local_cache_dir: cache.to_path_buf(),
    };
    KmoSyncFacade::create(config, EventEmitter::new(None, std::ptr::null_mut())).unwrap()
}

#[test]
fn repeated_run_with_regressed_clock_keeps_lww_consistent() {
    let remote = tempfile::tempdir().unwrap();
    let cache_a = tempfile::tempdir().unwrap();
    let cache_b = tempfile::tempdir().unwrap();
    let a = facade(cache_a.path(), remote.path(), "phone-1");
    let b = facade(cache_b.path(), remote.path(), "pad-1");

    // ---------------- 第一轮：完整场景（30% -> 65% -> 删除 -> 复活） ----------------
    a.put_local_meta(&meta(
        0.3,
        100,
        "phone-1",
        vec![bm("bm-phone-1", "序章", 100)],
    ))
    .unwrap();
    a.sync_single_meta("book-santi", "book-santi").unwrap();

    b.sync_single_meta("book-santi", "book-santi").unwrap();
    let pulled = b.read_local_meta("book-santi").unwrap().unwrap();
    assert_eq!(pulled.progress.unwrap().progress_percent, 0.3);

    b.put_local_meta(&meta(
        0.65,
        200,
        "pad-1",
        vec![
            bm("bm-phone-1", "序章", 100),
            bm("bm-pad-1", "黑暗森林", 200),
        ],
    ))
    .unwrap();
    b.sync_single_meta("book-santi", "book-santi").unwrap();

    a.sync_single_meta("book-santi", "book-santi").unwrap();
    assert_eq!(
        a.read_local_meta("book-santi")
            .unwrap()
            .unwrap()
            .progress
            .unwrap()
            .progress_percent,
        0.65
    );

    a.mark_meta_item_deleted("book-santi", "bookmark", "bm-pad-1")
        .unwrap();
    a.sync_single_meta("book-santi", "book-santi").unwrap();
    b.sync_single_meta("book-santi", "book-santi").unwrap();
    let b_after_delete = b.read_local_meta("book-santi").unwrap().unwrap();
    assert_eq!(b_after_delete.bookmarks.len(), 1, "第一轮：删除应已传播");

    a.undo_deletion("book-santi", "bm-pad-1").unwrap();
    a.sync_single_meta("book-santi", "book-santi").unwrap();
    b.sync_single_meta("book-santi", "book-santi").unwrap();
    let b_after_revive = b.read_local_meta("book-santi").unwrap().unwrap();
    assert_eq!(b_after_revive.bookmarks.len(), 2, "第一轮：复活应已传播");

    // ---------------- 第二轮：demo 重跑，时间戳回拨到 100/200 ----------------
    a.put_local_meta(&meta(
        0.3,
        100,
        "phone-1",
        vec![bm("bm-phone-1", "序章", 100)],
    ))
    .unwrap();
    a.sync_single_meta("book-santi", "book-santi").unwrap();
    b.sync_single_meta("book-santi", "book-santi").unwrap();

    let b_second = b.read_local_meta("book-santi").unwrap().unwrap();
    assert_eq!(
        b_second.progress.unwrap().progress_percent,
        0.3,
        "第二轮：手机重读到 30% 必须覆盖平板残留的 65%（写入时间戳不得回拨）"
    );

    a.mark_meta_item_deleted("book-santi", "bookmark", "bm-pad-1")
        .unwrap();
    a.sync_single_meta("book-santi", "book-santi").unwrap();
    b.sync_single_meta("book-santi", "book-santi").unwrap();
    let b_second_delete = b.read_local_meta("book-santi").unwrap().unwrap();
    assert_eq!(
        b_second_delete.bookmarks.len(),
        1,
        "第二轮：删除必须再次生效（新 tombstone 的时间戳必须大于旧 revival）"
    );
}
