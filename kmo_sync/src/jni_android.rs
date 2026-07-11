#![allow(non_snake_case)]

use crate::ffi;
use jni::JNIEnv;
use jni::JavaVM;
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::{jint, jlong, jstring};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

struct AndroidEventCallback {
    vm: JavaVM,
    callback: GlobalRef,
}

static CALLBACKS: OnceLock<Mutex<HashMap<usize, Arc<AndroidEventCallback>>>> = OnceLock::new();
static NEXT_CALLBACK_ID: AtomicUsize = AtomicUsize::new(1);

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kmosync_KmoSyncJni_create(
    mut env: JNIEnv,
    _class: JClass,
    storage_config_json: JString,
    encryption_config_json: JString,
    device_id: JString,
    local_cache_dir: JString,
    callback: JObject,
) -> jlong {
    let Some(storage_config_json) = jstring_to_cstring(&mut env, storage_config_json) else {
        return 0;
    };
    let Some(encryption_config_json) = jstring_to_cstring(&mut env, encryption_config_json) else {
        return 0;
    };
    let Some(device_id) = jstring_to_cstring(&mut env, device_id) else {
        return 0;
    };
    let Some(local_cache_dir) = jstring_to_cstring(&mut env, local_cache_dir) else {
        return 0;
    };

    if callback.is_null() {
        return 0;
    }
    let Ok(vm) = env.get_java_vm() else {
        return 0;
    };
    let Ok(callback) = env.new_global_ref(callback) else {
        return 0;
    };
    let callback_id = NEXT_CALLBACK_ID.fetch_add(1, Ordering::Relaxed);
    let Ok(mut callbacks) = callback_registry().lock() else {
        return 0;
    };
    callbacks.insert(callback_id, Arc::new(AndroidEventCallback { vm, callback }));
    drop(callbacks);

    let handle = ffi::kmo_sync_create(
        storage_config_json.as_ptr(),
        encryption_config_json.as_ptr(),
        device_id.as_ptr(),
        local_cache_dir.as_ptr(),
        Some(android_event_callback),
        callback_id as *mut c_void,
    );
    if handle.is_null() {
        if let Ok(mut callbacks) = callback_registry().lock() {
            callbacks.remove(&callback_id);
        }
        return 0;
    }
    let Ok(mut handles) = ANDROID_HANDLE_CALLBACKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    else {
        ffi::kmo_sync_destroy(handle);
        if let Ok(mut callbacks) = callback_registry().lock() {
            callbacks.remove(&callback_id);
        }
        return 0;
    };
    handles.insert(handle as usize, callback_id);
    handle as jlong
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kmosync_KmoSyncJni_destroy(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    ffi::kmo_sync_destroy(handle_to_ptr(handle));
    if let Some(callback_id) = ANDROID_HANDLE_CALLBACKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .ok()
        .and_then(|mut values| values.remove(&(handle as usize)))
    {
        if let Ok(mut callbacks) = callback_registry().lock() {
            callbacks.remove(&callback_id);
        }
    }
}

static ANDROID_HANDLE_CALLBACKS: OnceLock<Mutex<HashMap<usize, usize>>> = OnceLock::new();

fn callback_registry() -> &'static Mutex<HashMap<usize, Arc<AndroidEventCallback>>> {
    CALLBACKS.get_or_init(|| Mutex::new(HashMap::new()))
}

unsafe extern "C" fn android_event_callback(
    event_type: i32,
    json_data: *const c_char,
    user_data: *mut c_void,
) {
    if json_data.is_null() {
        return;
    }
    let callback = callback_registry()
        .lock()
        .ok()
        .and_then(|callbacks| callbacks.get(&(user_data as usize)).cloned());
    let Some(callback) = callback else {
        return;
    };
    let Ok(mut env) = callback.vm.attach_current_thread() else {
        return;
    };
    let json = unsafe { CStr::from_ptr(json_data) }.to_string_lossy();
    let Ok(json) = env.new_string(json.as_ref()) else {
        return;
    };
    let json_object = JObject::from(json);
    let _ = env.call_method(
        callback.callback.as_obj(),
        "onEvent",
        "(ILjava/lang/String;)V",
        &[JValue::Int(event_type), JValue::Object(&json_object)],
    );
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kmosync_KmoSyncJni_syncAll(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    mode: jint,
) -> jint {
    ffi::kmo_sync_all(handle_to_ptr(handle), mode)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kmosync_KmoSyncJni_syncSingleMeta(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    book_hash: JString,
    meta_id: JString,
) -> jint {
    let Some(book_hash) = jstring_to_cstring(&mut env, book_hash) else {
        return 5;
    };
    let Some(meta_id) = jstring_to_cstring(&mut env, meta_id) else {
        return 5;
    };
    ffi::kmo_sync_single_meta(handle_to_ptr(handle), book_hash.as_ptr(), meta_id.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kmosync_KmoSyncJni_syncBook(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    book_hash: JString,
) -> jint {
    with_cstring(&mut env, book_hash, |book_hash| {
        ffi::kmo_sync_book(handle_to_ptr(handle), book_hash)
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kmosync_KmoSyncJni_putLocalBook(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    book_hash: JString,
    local_file_path: JString,
) -> jint {
    let Some(book_hash) = jstring_to_cstring(&mut env, book_hash) else {
        return 5;
    };
    let Some(local_file_path) = jstring_to_cstring(&mut env, local_file_path) else {
        return 5;
    };
    ffi::kmo_sync_put_local_book(
        handle_to_ptr(handle),
        book_hash.as_ptr(),
        local_file_path.as_ptr(),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kmosync_KmoSyncJni_putLocalMetaJson(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    meta_json: JString,
) -> jint {
    with_cstring(&mut env, meta_json, |meta_json| {
        ffi::kmo_sync_put_local_meta_json(handle_to_ptr(handle), meta_json)
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kmosync_KmoSyncJni_resolveMetaConflict(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    meta_id: JString,
    chosen_version: JString,
) -> jint {
    let Some(meta_id) = jstring_to_cstring(&mut env, meta_id) else {
        return 5;
    };
    let Some(chosen_version) = jstring_to_cstring(&mut env, chosen_version) else {
        return 5;
    };
    ffi::kmo_sync_resolve_meta_file_conflict(
        handle_to_ptr(handle),
        meta_id.as_ptr(),
        chosen_version.as_ptr(),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kmosync_KmoSyncJni_resolveBlobConflict(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    book_hash: JString,
    chosen_version: JString,
) -> jint {
    let Some(book_hash) = jstring_to_cstring(&mut env, book_hash) else {
        return 5;
    };
    let Some(chosen_version) = jstring_to_cstring(&mut env, chosen_version) else {
        return 5;
    };
    ffi::kmo_sync_resolve_blob_conflict(
        handle_to_ptr(handle),
        book_hash.as_ptr(),
        chosen_version.as_ptr(),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kmosync_KmoSyncJni_rotateEnvelopeKek(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    new_encryption_config_json: JString,
) -> jint {
    let Some(new_encryption_config_json) = jstring_to_cstring(&mut env, new_encryption_config_json)
    else {
        return 5;
    };
    ffi::kmo_sync_rotate_envelope_kek(handle_to_ptr(handle), new_encryption_config_json.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kmosync_KmoSyncJni_markMetaItemDeleted(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    meta_id: JString,
    item_type: JString,
    item_uuid: JString,
) -> jint {
    let Some(meta_id) = jstring_to_cstring(&mut env, meta_id) else {
        return 5;
    };
    let Some(item_type) = jstring_to_cstring(&mut env, item_type) else {
        return 5;
    };
    let Some(item_uuid) = jstring_to_cstring(&mut env, item_uuid) else {
        return 5;
    };
    ffi::kmo_sync_mark_meta_item_deleted(
        handle_to_ptr(handle),
        meta_id.as_ptr(),
        item_type.as_ptr(),
        item_uuid.as_ptr(),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kmosync_KmoSyncJni_undoDeletion(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    meta_id: JString,
    item_uuid: JString,
) -> jint {
    let Some(meta_id) = jstring_to_cstring(&mut env, meta_id) else {
        return 5;
    };
    let Some(item_uuid) = jstring_to_cstring(&mut env, item_uuid) else {
        return 5;
    };
    ffi::kmo_sync_undo_deletion(handle_to_ptr(handle), meta_id.as_ptr(), item_uuid.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kmosync_KmoSyncJni_resolveTombstoneRevival(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    meta_id: JString,
    item_uuid: JString,
    resolution: JString,
) -> jint {
    let Some(meta_id) = jstring_to_cstring(&mut env, meta_id) else {
        return 5;
    };
    let Some(item_uuid) = jstring_to_cstring(&mut env, item_uuid) else {
        return 5;
    };
    let Some(resolution) = jstring_to_cstring(&mut env, resolution) else {
        return 5;
    };
    ffi::kmo_sync_resolve_tombstone_revival(
        handle_to_ptr(handle),
        meta_id.as_ptr(),
        item_uuid.as_ptr(),
        resolution.as_ptr(),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kmosync_KmoSyncJni_getLocalMeta(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    meta_id: JString,
) -> jstring {
    let Some(meta_id) = jstring_to_cstring(&mut env, meta_id) else {
        return ptr::null_mut();
    };
    let raw = ffi::kmo_sync_get_local_meta(handle_to_ptr(handle), meta_id.as_ptr());
    raw_string_to_jstring(&mut env, raw)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kmosync_KmoSyncJni_getSyncState(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jstring {
    let raw = ffi::kmo_sync_get_sync_state(handle_to_ptr(handle));
    raw_string_to_jstring(&mut env, raw)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kmosync_KmoSyncJni_setNetworkType(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    network_type: jint,
) -> jint {
    ffi::kmo_sync_set_network_type(handle_to_ptr(handle), network_type)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kmosync_KmoSyncJni_setBlobByteLimit(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    byte_limit: jlong,
) -> jint {
    ffi::kmo_sync_set_blob_byte_limit(handle_to_ptr(handle), byte_limit)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kmosync_KmoSyncJni_pause(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jint {
    ffi::kmo_sync_pause(handle_to_ptr(handle))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kmosync_KmoSyncJni_resume(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jint {
    ffi::kmo_sync_resume(handle_to_ptr(handle))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kmosync_KmoSyncJni_lastError(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jstring {
    let raw = ffi::kmo_sync_last_error(handle_to_ptr(handle));
    raw_string_to_jstring(&mut env, raw)
}

fn with_cstring<F>(env: &mut JNIEnv, value: JString, call: F) -> jint
where
    F: FnOnce(*const c_char) -> jint,
{
    let Some(value) = jstring_to_cstring(env, value) else {
        return 5;
    };
    call(value.as_ptr())
}

fn jstring_to_cstring(env: &mut JNIEnv, value: JString) -> Option<CString> {
    if value.is_null() {
        return None;
    }
    let value: String = env.get_string(&value).ok()?.into();
    CString::new(value).ok()
}

fn raw_string_to_jstring(env: &mut JNIEnv, raw: *mut c_char) -> jstring {
    if raw.is_null() {
        return ptr::null_mut();
    }
    let value = unsafe { CStr::from_ptr(raw) }.to_string_lossy().to_string();
    ffi::kmo_sync_free_string(raw);
    env.new_string(value)
        .map(|value| value.into_raw())
        .unwrap_or(ptr::null_mut())
}

fn handle_to_ptr(handle: jlong) -> *mut ffi::kmo_sync_t {
    handle as *mut ffi::kmo_sync_t
}
