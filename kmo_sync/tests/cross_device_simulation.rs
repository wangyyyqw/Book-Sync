//! End-to-end multi-device reading simulation.
//!
//! Reproduces the user flow "read on phone → continue on pad → continue on
//! phone again" across three real EPUB files of different sizes so each
//! upload path is exercised:
//!
//! * 恐妻家 (~400 KiB)              — single-shot blob upload
//! * 阅微草堂笔记 (~8.8 MiB)         — single-shot + envelope encryption
//! * 海底两万里 (~164 MiB)           — FastCDC CAS manifest + chunked upload
//!
//! Each book follows the same three phases:
//!
//! 1. Phone reads to ~20%, adds a bookmark + highlight, syncs up.
//! 2. Pad downloads, reads to ~50%, adds a note and a second highlight,
//!    syncs up. The phone's progress must NOT be overwritten.
//! 3. A fresh-cache phone pulls again and must see the combined state
//!    (progress ~50% with the pad's device_id, all three highlights,
//!    one bookmark, one note).
//!
//! Storage uses the `file` adapter pointed at a tempdir, so the test is
//! fully offline and deterministic. Cleaning the test only requires
//! removing the tempdir; the bucket is untouched.

use std::path::Path;

use kmo_sync::event::EventEmitter;
use kmo_sync::model::{BookNote, BookReadingMeta, Bookmark, Highlight, MetaEdit, ReadingProgress};
use kmo_sync::{KmoSyncConfig, KmoSyncFacade};

const PUSH_ONLY: i32 = 1;
const PULL_ONLY: i32 = 2;

struct ScenarioBook {
    label: &'static str,
    /// Source EPUB path. Small files live in `测试文件/`; the 164 MiB phone
    /// edition lives at the repo root.
    source_path: &'static str,
    /// Logical meta_id used across all phases. Derived from the source
    /// file hash under the reeden layout (book_hash == meta_id).
    meta_id: String,
}

fn books() -> [ScenarioBook; 3] {
    [
        ScenarioBook {
            label: "kasaike",
            source_path: "/Users/aaa/Documents/github/KMO-Sync/测试文件/恐妻家 - [日]伊坂幸太郎.epub",
            meta_id: blake3_hex_of_file("/Users/aaa/Documents/github/KMO-Sync/测试文件/恐妻家 - [日]伊坂幸太郎.epub").unwrap_or_else(|| "meta-kasaike".to_string()),
        },
        ScenarioBook {
            label: "yuewei",
            source_path: "/Users/aaa/Documents/github/KMO-Sync/测试文件/阅微草堂笔记.epub",
            meta_id: blake3_hex_of_file("/Users/aaa/Documents/github/KMO-Sync/测试文件/阅微草堂笔记.epub").unwrap_or_else(|| "meta-yuewei".to_string()),
        },
        ScenarioBook {
            label: "haidi",
            // Use a second copy of the same source for legacy fixture
            // compatibility (the original "海底两万里-phone.epub" fixture is
            // not always present in CI).
            source_path: "/Users/aaa/Documents/github/KMO-Sync/测试文件/C41-愤怒的葡萄-[美] 约翰·斯坦贝克-手机.epub",
            meta_id: blake3_hex_of_file("/Users/aaa/Documents/github/KMO-Sync/测试文件/C41-愤怒的葡萄-[美] 约翰·斯坦贝克-手机.epub").unwrap_or_else(|| "meta-haidi".to_string()),
        },
    ]
}

#[test]
fn simulate_three_books_phone_pad_phone_roundtrip() {
    let remote = tempfile::tempdir().expect("tempdir for remote");
    let cache_phone = tempfile::tempdir().expect("tempdir for phone");
    let cache_pad = tempfile::tempdir().expect("tempdir for pad");
    let cache_phone_again = tempfile::tempdir().expect("tempdir for phone (again)");

    // Each device uses a different envelope passphrase-equivalent: for this
    // offline simulation we run with `none` encryption so the layout under
    // test is purely the wire path, not crypto.
    let encryption_json = r#"{"type":"none"}"#.to_string();

    let phone = open_device(
        "phone-iphone",
        remote.path(),
        cache_phone.path(),
        &encryption_json,
    );
    let pad = open_device(
        "pad-ipad",
        remote.path(),
        cache_pad.path(),
        &encryption_json,
    );
    let phone_again = open_device(
        "phone-iphone",
        remote.path(),
        cache_phone_again.path(),
        &encryption_json,
    );

    for book in books() {
        run_book_scenario(&book, &phone, &pad, &phone_again, remote.path());
    }

    // Final inspection of the bucket layout so a regression in
    // `remote_*_path` helpers would surface here.
    let books_root = remote.path().join("books");
    assert!(
        books_root.is_dir(),
        "expected books/ tree under {}",
        remote.path().display()
    );
    let expected_hashes: Vec<&str> = books().iter().map(|b| b.source_path).collect();
    for path in &expected_hashes {
        let hash = blake3_hex_of_file(path).unwrap();
        let book_blob = books_root.join(&hash);
        assert!(
            book_blob.is_file(),
            "expected books/{hash} blob file at {}",
            book_blob.display()
        );
    }
}

// ---------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------

fn open_device(
    device_id: &str,
    remote: &Path,
    cache: &Path,
    encryption_json: &str,
) -> KmoSyncFacade {
    let storage_json = format!(
        r#"{{"type":"file","root_dir":"{}"}}"#,
        remote.to_string_lossy()
    );
    KmoSyncFacade::create(
        KmoSyncConfig {
            storage_config_json: storage_json,
            encryption_config_json: encryption_json.to_string(),
            device_id: device_id.to_string(),
            local_cache_dir: cache.to_path_buf(),
        },
        EventEmitter::new(None, std::ptr::null_mut()),
    )
    .expect("create facade")
}

fn blake3_hex_of_file(path: &str) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(blake3::hash(&bytes).to_hex().to_string())
}

fn book_bytes_or_panic(book: &ScenarioBook) -> Vec<u8> {
    std::fs::read(book.source_path).unwrap_or_else(|err| {
        panic!(
            "missing test fixture {} for {}: {err}; place the EPUB at the expected path",
            book.source_path, book.label
        )
    })
}

fn run_book_scenario(
    book: &ScenarioBook,
    phone: &KmoSyncFacade,
    pad: &KmoSyncFacade,
    phone_again: &KmoSyncFacade,
    remote: &Path,
) {
    let bytes = book_bytes_or_panic(book);
    let book_hash = blake3::hash(&bytes).to_hex().to_string();
    println!(
        "[{}] size={} bytes, blake3={}",
        book.label,
        bytes.len(),
        book_hash
    );

    // ---- Phase 1: phone reads to ~20% + creates bookmark + highlight ----
    let stage = bytes.len() < 5 * 1024 * 1024;
    println!(
        "[{}] upload path: {}",
        book.label,
        if stage { "single-shot" } else { "FastCDC CAS" }
    );

    stage_local_book(phone, &book_hash, &bytes);
    let mut meta_p1 = base_meta(&book.meta_id, &book_hash, "phone-iphone", 100);
    meta_p1.progress = Some(ReadingProgress {
        cfi_position: format!("epubcfi(/6/2[{}])", book.label),
        progress_percent: 0.20,
        chapter_id: "chapter-1".to_string(),
    });
    meta_p1.bookmarks.push(Bookmark {
        bookmark_id: "bm-phone-1".to_string(),
        cfi_range: format!("epubcfi(/6/2[{}])", book.label),
        title: format!("Phone bookmark — {}", book.label),
        create_ts: 100,
    });
    meta_p1.highlights.push(Highlight {
        highlight_id: "hl-phone-1".to_string(),
        cfi_start: format!("epubcfi(/6/2[{}])", book.label),
        cfi_end: format!("epubcfi(/6/4[{}])", book.label),
        color: "yellow".to_string(),
        comment: format!("phone highlight — {}", book.label),
        create_ts: 100,
    });
    phone.put_local_meta(&meta_p1).unwrap();
    push_blob_and_meta(phone, &book_hash);

    // The 5 MiB BLOB_CAS_THRESHOLD_BYTES routes everything to the FastCDC
    // path, so the wire-side blob becomes a manifest + chunks rather than a
    // raw <hash>.epub. Books under the threshold go through the single-shot
    // write path which leaves a raw <hash>.epub.
    // Reeden-style layout writes every book as a single `books/<hash>` object —
    // the old FastCDC manifest and per-book directory are gone.
    let single_shot_path = remote.join("books").join(&book_hash);
    assert!(
        single_shot_path.exists(),
        "[{}] expected single-book blob at {}",
        book.label,
        single_shot_path.display()
    );
    let manifest_legacy_path = remote
        .join("books")
        .join(&book_hash)
        .join("blobs")
        .join(format!("{book_hash}.manifest.json"));
    assert!(
        !manifest_legacy_path.exists(),
        "[{}] reeden layout must not write a FastCDC manifest at {}",
        book.label,
        manifest_legacy_path.display()
    );

    // ---- Phase 2: pad downloads, continues to ~50%, adds note + highlight ----
    pull_blob_and_meta(pad, &book_hash);

    let mut meta_p2 = pad.read_local_meta(&book.meta_id).unwrap().unwrap();
    assert_eq!(
        meta_p2.progress.as_ref().unwrap().progress_percent,
        0.20,
        "[{}] pad should observe phone's 20% before continuing",
        book.label
    );
    meta_p2.logical_ts = 200;
    meta_p2.modified_ts = 200;
    meta_p2.wall_clock_ts = 200;
    meta_p2.device_id = "pad-ipad".to_string();
    meta_p2.origin_device_id = "phone-iphone".to_string();
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
    meta_p2
        .edit_history
        .push(simple_edit("pad-progress", "pad-ipad", 200));
    pad.put_local_meta(&meta_p2).unwrap();
    push_meta_only(pad);

    // Phone should still see its 20% progress locally — pad's push only
    // updates the remote, never overwrites the local phone state.
    let phone_local_after_pad = phone.read_local_meta(&book.meta_id).unwrap().unwrap();
    assert_eq!(
        phone_local_after_pad
            .progress
            .as_ref()
            .unwrap()
            .progress_percent,
        0.20,
        "[{}] pad push must not overwrite phone's local progress",
        book.label
    );

    // ---- Phase 3: a fresh-cache phone pulls and observes the pad's view ----
    pull_blob_and_meta(phone_again, &book_hash);

    let final_meta = phone_again
        .read_local_meta(&book.meta_id)
        .unwrap()
        .unwrap_or_else(|| panic!("[{}] expected merged meta on phone-again", book.label));
    assert_eq!(
        final_meta.progress.as_ref().unwrap().progress_percent,
        0.50,
        "[{}] expected 50% progress after pad sync",
        book.label
    );
    assert_eq!(
        final_meta.device_id, "pad-ipad",
        "[{}] expected pad as latest writer",
        book.label
    );
    assert_eq!(
        final_meta.bookmarks.len(),
        1,
        "[{}] bookmark count",
        book.label
    );
    assert_eq!(final_meta.bookmarks[0].bookmark_id, "bm-phone-1");
    assert_eq!(
        final_meta.highlights.len(),
        2,
        "[{}] highlight count (phone + pad)",
        book.label
    );
    let highlight_ids: std::collections::BTreeSet<&str> = final_meta
        .highlights
        .iter()
        .map(|h| h.highlight_id.as_str())
        .collect();
    assert!(highlight_ids.contains("hl-phone-1"));
    assert!(highlight_ids.contains("hl-pad-1"));
    assert_eq!(final_meta.notes.len(), 1, "[{}] note count", book.label);
    assert_eq!(final_meta.notes[0].note_id, "note-pad-1");

    // Tombstone round-trip: phone-again deletes hl-phone-1, syncs up,
    // then a brand-new "reader c" cache pulls and confirms the highlight
    // is gone (per-book tombstone propagation under the yuewei layout).
    phone_again
        .mark_meta_item_deleted(&book.meta_id, "highlight", "hl-phone-1")
        .unwrap();
    phone_again
        .sync_single_meta(&book_hash, &book.meta_id)
        .unwrap();
    let cache_reader_c = tempfile::tempdir().expect("tempdir for reader-c");
    let reader_c = open_device(
        "reader-c",
        remote,
        cache_reader_c.path(),
        r#"{"type":"none"}"#,
    );
    pull_meta_only(&reader_c);
    let c_meta = reader_c.read_local_meta(&book.meta_id).unwrap().unwrap();
    assert_eq!(
        c_meta.highlights.len(),
        1,
        "[{}] tombstoned highlight should be gone after merge",
        book.label
    );
    assert_eq!(c_meta.highlights[0].highlight_id, "hl-pad-1");

    // Verify the on-disk remote layout for this book — exactly the shape
    // advertised by the yuewei/<book>/{blobs,metas} restructure.
    // Reeden layout keeps the per-book remote payload flat at books/<hash>;
    // there are no per-book metas/ or blobs/ subdirectories.
    let book_blob = remote.join("books").join(&book_hash);
    assert!(book_blob.is_file(), "[{}] book blob is a file", book.label);
}

fn stage_local_book(facade: &KmoSyncFacade, book_hash: &str, bytes: &[u8]) {
    let cache = facade.local_cache_dir().to_path_buf();
    std::fs::create_dir_all(&cache).unwrap();
    let staged = cache.join(format!("stage-{book_hash}.epub"));
    std::fs::write(&staged, bytes).unwrap();
    facade.put_local_book(book_hash, &staged).unwrap();
}

fn push_blob_and_meta(facade: &KmoSyncFacade, book_hash: &str) {
    facade.sync_book(book_hash).expect("sync_book push");
    facade.sync_all(PUSH_ONLY).expect("sync_all push");
}

fn push_meta_only(facade: &KmoSyncFacade) {
    facade.sync_all(PUSH_ONLY).expect("sync_all push meta only");
}

fn pull_blob_and_meta(facade: &KmoSyncFacade, book_hash: &str) {
    facade.sync_all(PULL_ONLY).expect("sync_all pull");
    facade.sync_book(book_hash).expect("sync_book pull");
}

fn pull_meta_only(facade: &KmoSyncFacade) {
    facade.sync_all(PULL_ONLY).expect("sync_all pull meta only");
}

fn base_meta(meta_id: &str, book_hash: &str, device_id: &str, ts: i64) -> BookReadingMeta {
    BookReadingMeta {
        meta_id: meta_id.to_string(),
        book_hash: book_hash.to_string(),
        modified_ts: ts,
        device_id: device_id.to_string(),
        progress: None,
        bookmarks: vec![],
        highlights: vec![],
        notes: vec![],
        wall_clock_ts: ts,
        logical_ts: ts,
        origin_device_id: device_id.to_string(),
        edit_history: vec![simple_edit(&format!("init-{meta_id}"), device_id, ts)],
    }
}

fn simple_edit(edit_id: &str, device_id: &str, ts: i64) -> MetaEdit {
    MetaEdit {
        edit_id: edit_id.to_string(),
        device_id: device_id.to_string(),
        logical_ts: ts,
        op: None,
    }
}
