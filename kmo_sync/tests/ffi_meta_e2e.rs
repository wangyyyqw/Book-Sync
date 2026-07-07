use kmo_sync::ffi::{
    kmo_sync_all, kmo_sync_create, kmo_sync_destroy, kmo_sync_free_string, kmo_sync_get_local_meta,
    kmo_sync_put_local_meta_json,
};
use std::ffi::{CStr, CString};
use std::ptr;

#[test]
fn ffi_android_ios_meta_roundtrip_with_shared_remote() {
    let remote = tempfile::tempdir().unwrap();
    let android_cache = tempfile::tempdir().unwrap();
    let ios_cache = tempfile::tempdir().unwrap();

    let storage_json = CString::new(format!(
        r#"{{"type":"file","root_dir":"{}"}}"#,
        remote.path().to_string_lossy()
    ))
    .unwrap();
    let encryption_json = CString::new(r#"{"type":"none"}"#).unwrap();
    let android_device = CString::new("android-sample").unwrap();
    let ios_device = CString::new("ios-sample").unwrap();
    let android_cache = CString::new(android_cache.path().to_string_lossy().to_string()).unwrap();
    let ios_cache = CString::new(ios_cache.path().to_string_lossy().to_string()).unwrap();

    let android = kmo_sync_create(
        storage_json.as_ptr(),
        encryption_json.as_ptr(),
        android_device.as_ptr(),
        android_cache.as_ptr(),
        None,
        ptr::null_mut(),
    );
    let ios = kmo_sync_create(
        storage_json.as_ptr(),
        encryption_json.as_ptr(),
        ios_device.as_ptr(),
        ios_cache.as_ptr(),
        None,
        ptr::null_mut(),
    );

    assert!(!android.is_null());
    assert!(!ios.is_null());

    let android_meta = CString::new(sample_meta_json("android-sample", 0.42, 10)).unwrap();
    assert_eq!(
        kmo_sync_put_local_meta_json(android, android_meta.as_ptr()),
        0
    );
    assert_eq!(kmo_sync_all(android, 1), 0);

    let shared_meta = CString::new("shared-meta").unwrap();
    assert_eq!(kmo_sync_all(ios, 2), 0);
    let pulled_ios = read_meta_json(ios, shared_meta.as_ptr());
    assert!(pulled_ios.contains("\"progress_percent\":0.42"));

    let ios_meta = CString::new(sample_meta_json("ios-sample", 0.84, 20)).unwrap();
    assert_eq!(kmo_sync_put_local_meta_json(ios, ios_meta.as_ptr()), 0);
    assert_eq!(kmo_sync_all(ios, 1), 0);

    assert_eq!(kmo_sync_all(android, 2), 0);
    let pulled_android = read_meta_json(android, shared_meta.as_ptr());
    assert!(pulled_android.contains("\"progress_percent\":0.84"));

    kmo_sync_destroy(android);
    kmo_sync_destroy(ios);
}

fn read_meta_json(
    handle: *mut kmo_sync::ffi::kmo_sync_t,
    meta_id: *const std::os::raw::c_char,
) -> String {
    let raw = kmo_sync_get_local_meta(handle, meta_id);
    assert!(!raw.is_null());
    let value = unsafe { CStr::from_ptr(raw) }.to_string_lossy().to_string();
    kmo_sync_free_string(raw);
    value
}

fn sample_meta_json(device_id: &str, progress: f64, logical_ts: i64) -> String {
    format!(
        r#"{{
            "meta_id":"shared-meta",
            "book_hash":"shared-meta",
            "modified_ts":{logical_ts},
            "device_id":"{device_id}",
            "progress":{{
                "cfi_position":"epubcfi(/6/2)",
                "progress_percent":{progress},
                "chapter_id":"chapter-1"
            }},
            "bookmarks":[],
            "highlights":[],
            "notes":[],
            "wall_clock_ts":{logical_ts},
            "logical_ts":{logical_ts},
            "origin_device_id":"{device_id}",
            "edit_history":[]
        }}"#
    )
}
