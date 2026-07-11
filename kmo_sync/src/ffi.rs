#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::SyncError;
use crate::event::{EventCallback, EventEmitter};
use crate::facade::{KmoSyncConfig, KmoSyncFacade};
use crate::model::BookReadingMeta;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

pub struct KmoSyncHandle {
    facade: KmoSyncFacade,
    last_error: Mutex<Option<String>>,
}

static HANDLES: OnceLock<Mutex<HashMap<usize, Arc<KmoSyncHandle>>>> = OnceLock::new();
static NEXT_HANDLE_ID: AtomicUsize = AtomicUsize::new(1);

#[allow(non_camel_case_types)]
pub type kmo_sync_t = KmoSyncHandle;

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_create(
    storage_config_json: *const c_char,
    encryption_config_json: *const c_char,
    device_id: *const c_char,
    local_cache_dir: *const c_char,
    callback: EventCallback,
    user_data: *mut c_void,
) -> *mut kmo_sync_t {
    let result = create_inner(
        storage_config_json,
        encryption_config_json,
        device_id,
        local_cache_dir,
        callback,
        user_data,
    );

    match result {
        Ok(handle) => {
            let id = NEXT_HANDLE_ID.fetch_add(1, Ordering::Relaxed);
            let Ok(mut handles) = handle_registry().lock() else {
                return ptr::null_mut();
            };
            handles.insert(id, Arc::new(handle));
            id as *mut kmo_sync_t
        }
        Err(_) => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_destroy(sync: *mut kmo_sync_t) {
    if sync.is_null() {
        return;
    }
    if let Ok(mut handles) = handle_registry().lock() {
        handles.remove(&(sync as usize));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_all(sync: *mut kmo_sync_t, mode: i32) -> i32 {
    with_handle(sync, |handle| handle.facade.sync_all(mode))
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_single_meta(
    sync: *mut kmo_sync_t,
    book_hash: *const c_char,
    meta_id: *const c_char,
) -> i32 {
    let Some(book_hash) = read_c_string(book_hash) else {
        return set_error(sync, SyncError::InvalidArg("book_hash is null".to_string()));
    };
    let Some(meta_id) = read_c_string(meta_id) else {
        return set_error(sync, SyncError::InvalidArg("meta_id is null".to_string()));
    };
    with_handle(sync, |handle| {
        handle.facade.sync_single_meta(&book_hash, &meta_id)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_book(sync: *mut kmo_sync_t, book_hash: *const c_char) -> i32 {
    let Some(book_hash) = read_c_string(book_hash) else {
        return set_error(sync, SyncError::InvalidArg("book_hash is null".to_string()));
    };
    with_handle(sync, |handle| handle.facade.sync_book(&book_hash))
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_put_local_book(
    sync: *mut kmo_sync_t,
    book_hash: *const c_char,
    local_file_path: *const c_char,
) -> i32 {
    let Some(book_hash) = read_c_string(book_hash) else {
        return set_error(sync, SyncError::InvalidArg("book_hash is null".to_string()));
    };
    let Some(local_file_path) = read_c_string(local_file_path) else {
        return set_error(
            sync,
            SyncError::InvalidArg("local_file_path is null".to_string()),
        );
    };
    with_handle(sync, |handle| {
        handle
            .facade
            .put_local_book(&book_hash, &PathBuf::from(local_file_path))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_put_local_meta_json(
    sync: *mut kmo_sync_t,
    meta_json: *const c_char,
) -> i32 {
    let Some(meta_json) = read_c_string(meta_json) else {
        return set_error(sync, SyncError::InvalidArg("meta_json is null".to_string()));
    };
    let meta: BookReadingMeta = match serde_json::from_str(&meta_json) {
        Ok(meta) => meta,
        Err(err) => return set_error(sync, err.into()),
    };
    with_handle(sync, |handle| handle.facade.put_local_meta(&meta))
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_resolve_meta_file_conflict(
    sync: *mut kmo_sync_t,
    meta_id: *const c_char,
    chosen_version: *const c_char,
) -> i32 {
    let Some(meta_id) = read_c_string(meta_id) else {
        return set_error(sync, SyncError::InvalidArg("meta_id is null".to_string()));
    };
    let Some(chosen_version) = read_c_string(chosen_version) else {
        return set_error(
            sync,
            SyncError::InvalidArg("chosen_version is null".to_string()),
        );
    };
    with_handle(sync, |handle| {
        handle
            .facade
            .resolve_meta_conflict(&meta_id, &chosen_version)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_mark_meta_item_deleted(
    sync: *mut kmo_sync_t,
    meta_id: *const c_char,
    item_type: *const c_char,
    item_uuid: *const c_char,
) -> i32 {
    let Some(meta_id) = read_c_string(meta_id) else {
        return set_error(sync, SyncError::InvalidArg("meta_id is null".to_string()));
    };
    let Some(item_type) = read_c_string(item_type) else {
        return set_error(sync, SyncError::InvalidArg("item_type is null".to_string()));
    };
    let Some(item_uuid) = read_c_string(item_uuid) else {
        return set_error(sync, SyncError::InvalidArg("item_uuid is null".to_string()));
    };
    with_handle(sync, |handle| {
        handle
            .facade
            .mark_meta_item_deleted(&meta_id, &item_type, &item_uuid)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_undo_deletion(
    sync: *mut kmo_sync_t,
    meta_id: *const c_char,
    item_uuid: *const c_char,
) -> i32 {
    let Some(meta_id) = read_c_string(meta_id) else {
        return set_error(sync, SyncError::InvalidArg("meta_id is null".to_string()));
    };
    let Some(item_uuid) = read_c_string(item_uuid) else {
        return set_error(sync, SyncError::InvalidArg("item_uuid is null".to_string()));
    };
    with_handle(sync, |handle| {
        handle.facade.undo_deletion(&meta_id, &item_uuid)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_resolve_tombstone_revival(
    sync: *mut kmo_sync_t,
    meta_id: *const c_char,
    item_uuid: *const c_char,
    resolution: *const c_char,
) -> i32 {
    let Some(meta_id) = read_c_string(meta_id) else {
        return set_error(sync, SyncError::InvalidArg("meta_id is null".to_string()));
    };
    let Some(item_uuid) = read_c_string(item_uuid) else {
        return set_error(sync, SyncError::InvalidArg("item_uuid is null".to_string()));
    };
    let Some(resolution) = read_c_string(resolution) else {
        return set_error(
            sync,
            SyncError::InvalidArg("resolution is null".to_string()),
        );
    };
    with_handle(sync, |handle| {
        handle
            .facade
            .resolve_tombstone_revival(&meta_id, &item_uuid, &resolution)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_resolve_blob_conflict(
    sync: *mut kmo_sync_t,
    book_hash: *const c_char,
    chosen_version: *const c_char,
) -> i32 {
    let Some(book_hash) = read_c_string(book_hash) else {
        return set_error(sync, SyncError::InvalidArg("book_hash is null".to_string()));
    };
    let Some(chosen_version) = read_c_string(chosen_version) else {
        return set_error(
            sync,
            SyncError::InvalidArg("chosen_version is null".to_string()),
        );
    };
    with_handle(sync, |handle| {
        handle
            .facade
            .resolve_blob_conflict(&book_hash, &chosen_version)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_rotate_envelope_kek(
    sync: *mut kmo_sync_t,
    new_encryption_config_json: *const c_char,
) -> i32 {
    let Some(new_encryption_config_json) = read_c_string(new_encryption_config_json) else {
        return set_error(
            sync,
            SyncError::InvalidArg("new_encryption_config_json is null".to_string()),
        );
    };
    if sync.is_null() {
        return 5;
    }
    let Some(handle) = get_handle(sync) else {
        return 5;
    };
    match handle
        .facade
        .rotate_envelope_kek(&new_encryption_config_json)
    {
        Ok(_) => 0,
        Err(err) => {
            let code = err.code();
            set_last_error(&handle, err.to_string());
            code
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_set_network_type(sync: *mut kmo_sync_t, network_type: i32) -> i32 {
    with_handle(sync, |handle| handle.facade.set_network_type(network_type))
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_set_blob_byte_limit(sync: *mut kmo_sync_t, byte_limit: i64) -> i32 {
    with_handle(sync, |handle| handle.facade.set_blob_byte_limit(byte_limit))
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_pause(sync: *mut kmo_sync_t) -> i32 {
    with_handle(sync, |handle| handle.facade.pause())
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_resume(sync: *mut kmo_sync_t) -> i32 {
    with_handle(sync, |handle| handle.facade.resume())
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_get_local_meta(
    sync: *mut kmo_sync_t,
    meta_id: *const c_char,
) -> *mut c_char {
    let Some(meta_id) = read_c_string(meta_id) else {
        set_error(sync, SyncError::InvalidArg("meta_id is null".to_string()));
        return ptr::null_mut();
    };

    if sync.is_null() {
        return ptr::null_mut();
    }

    let Some(handle) = get_handle(sync) else {
        return ptr::null_mut();
    };
    match handle.facade.get_local_meta_json(&meta_id) {
        Ok(json) => string_to_raw(json),
        Err(err) => {
            set_last_error(&handle, err.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_get_sync_state(sync: *mut kmo_sync_t) -> *mut c_char {
    if sync.is_null() {
        return ptr::null_mut();
    }

    let Some(handle) = get_handle(sync) else {
        return ptr::null_mut();
    };
    match handle.facade.get_sync_state_json() {
        Ok(json) => string_to_raw(json),
        Err(err) => {
            set_last_error(&handle, err.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_last_error(sync: *mut kmo_sync_t) -> *mut c_char {
    if sync.is_null() {
        return string_to_raw("sync handle is null".to_string());
    }
    let Some(handle) = get_handle(sync) else {
        return string_to_raw("sync handle is closed".to_string());
    };
    let message = handle
        .last_error
        .lock()
        .map(|value| value.clone().unwrap_or_default())
        .unwrap_or_else(|_| "last-error mutex poisoned".to_string());
    string_to_raw(message)
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_free_string(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    unsafe {
        drop(CString::from_raw(s));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kmo_sync_get_version() -> i32 {
    KmoSyncFacade::version()
}

fn create_inner(
    storage_config_json: *const c_char,
    encryption_config_json: *const c_char,
    device_id: *const c_char,
    local_cache_dir: *const c_char,
    callback: EventCallback,
    user_data: *mut c_void,
) -> Result<KmoSyncHandle, SyncError> {
    let storage_config_json = read_c_string(storage_config_json)
        .ok_or_else(|| SyncError::InvalidArg("storage_config_json is null".to_string()))?;
    let encryption_config_json = read_c_string(encryption_config_json)
        .ok_or_else(|| SyncError::InvalidArg("encryption_config_json is null".to_string()))?;
    let device_id = read_c_string(device_id)
        .ok_or_else(|| SyncError::InvalidArg("device_id is null".to_string()))?;
    let local_cache_dir = read_c_string(local_cache_dir)
        .ok_or_else(|| SyncError::InvalidArg("local_cache_dir is null".to_string()))?;

    let config = KmoSyncConfig {
        storage_config_json,
        encryption_config_json,
        device_id,
        local_cache_dir: PathBuf::from(local_cache_dir),
    };
    let facade = KmoSyncFacade::create(config, EventEmitter::new(callback, user_data))?;
    Ok(KmoSyncHandle {
        facade,
        last_error: Mutex::new(None),
    })
}

fn with_handle<F>(sync: *mut kmo_sync_t, op: F) -> i32
where
    F: FnOnce(&KmoSyncHandle) -> Result<(), SyncError>,
{
    if sync.is_null() {
        return 5;
    }
    let Some(handle) = get_handle(sync) else {
        return 5;
    };
    match op(&handle) {
        Ok(()) => 0,
        Err(err) => {
            let code = err.code();
            set_last_error(&handle, err.to_string());
            code
        }
    }
}

fn set_error(sync: *mut kmo_sync_t, err: SyncError) -> i32 {
    let code = err.code();
    if let Some(handle) = get_handle(sync) {
        set_last_error(&handle, err.to_string());
    }
    code
}

fn handle_registry() -> &'static Mutex<HashMap<usize, Arc<KmoSyncHandle>>> {
    HANDLES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn get_handle(sync: *mut kmo_sync_t) -> Option<Arc<KmoSyncHandle>> {
    if sync.is_null() {
        return None;
    }
    handle_registry()
        .lock()
        .ok()?
        .get(&(sync as usize))
        .cloned()
}

fn set_last_error(handle: &KmoSyncHandle, message: String) {
    if let Ok(mut last_error) = handle.last_error.lock() {
        *last_error = Some(message);
    }
}

fn read_c_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let c_str = unsafe { CStr::from_ptr(ptr) };
    c_str.to_str().ok().map(ToOwned::to_owned)
}

fn string_to_raw(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn ffi_create_destroy_smoke() {
        let dir = tempfile::tempdir().unwrap();
        let storage = CString::new(r#"{"type":"memory"}"#).unwrap();
        let encryption = CString::new(r#"{"type":"none"}"#).unwrap();
        let device = CString::new("device-a").unwrap();
        let cache = CString::new(dir.path().to_string_lossy().to_string()).unwrap();

        let handle = kmo_sync_create(
            storage.as_ptr(),
            encryption.as_ptr(),
            device.as_ptr(),
            cache.as_ptr(),
            None,
            ptr::null_mut(),
        );
        assert!(!handle.is_null());
        assert_eq!(kmo_sync_all(handle, 0), 0);
        kmo_sync_destroy(handle);
    }

    #[test]
    fn ffi_handle_registry_serializes_lifetime_with_concurrent_calls() {
        let dir = tempfile::tempdir().unwrap();
        let storage = CString::new(r#"{"type":"memory"}"#).unwrap();
        let encryption = CString::new(r#"{"type":"none"}"#).unwrap();
        let device = CString::new("device-concurrent").unwrap();
        let cache = CString::new(dir.path().to_string_lossy().to_string()).unwrap();
        let handle = kmo_sync_create(
            storage.as_ptr(),
            encryption.as_ptr(),
            device.as_ptr(),
            cache.as_ptr(),
            None,
            ptr::null_mut(),
        );
        let id = handle as usize;
        let threads: Vec<_> = (0..4)
            .map(|_| {
                std::thread::spawn(move || {
                    for _ in 0..20 {
                        let code = kmo_sync_all(id as *mut kmo_sync_t, 0);
                        assert!(code == 0 || code == 5);
                    }
                })
            })
            .collect();
        kmo_sync_destroy(handle);
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(kmo_sync_all(id as *mut kmo_sync_t, 0), 5);
    }
}
