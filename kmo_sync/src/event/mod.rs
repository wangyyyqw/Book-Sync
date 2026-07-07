use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::os::raw::{c_char, c_void};

pub type EventCallback =
    Option<unsafe extern "C" fn(event_type: i32, json_data: *const c_char, user_data: *mut c_void)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum EventType {
    SyncStart = 1,
    SyncProgress = 2,
    BookChanged = 3,
    ConflictFound = 4,
    SecurityWarning = 5,
    SyncComplete = 6,
    Error = 7,
    BlobConflict = 8,
    DataConflict = 9,
    TombstoneRevival = 10,
    MergeProgress = 11,
    ClockDriftWarning = 12,
}

#[derive(Clone)]
pub struct EventEmitter {
    callback: EventCallback,
    user_data: usize,
}

impl EventEmitter {
    pub fn new(callback: EventCallback, user_data: *mut c_void) -> Self {
        Self {
            callback,
            user_data: user_data as usize,
        }
    }

    pub fn emit_json(&self, event_type: EventType, json: &str) {
        let Some(callback) = self.callback else {
            return;
        };

        if let Ok(c_json) = CString::new(json) {
            unsafe {
                callback(
                    event_type as i32,
                    c_json.as_ptr(),
                    self.user_data as *mut c_void,
                );
            }
        }
    }

    pub fn emit<T: Serialize>(&self, event_type: EventType, payload: &T) {
        if let Ok(json) = serde_json::to_string(payload) {
            self.emit_json(event_type, &json);
        }
    }
}

unsafe impl Send for EventEmitter {}
unsafe impl Sync for EventEmitter {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CALLBACK_COUNT: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn callback(
        event_type: i32,
        json_data: *const c_char,
        _user_data: *mut c_void,
    ) {
        assert_eq!(event_type, EventType::SyncStart as i32);
        assert!(!json_data.is_null());
        CALLBACK_COUNT.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn event_callback_receives_json() {
        CALLBACK_COUNT.store(0, Ordering::SeqCst);
        let emitter = EventEmitter::new(Some(callback), std::ptr::null_mut());
        emitter.emit(EventType::SyncStart, &serde_json::json!({"mode":"test"}));
        assert_eq!(CALLBACK_COUNT.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn null_callback_is_noop() {
        let emitter = EventEmitter::new(None, std::ptr::null_mut());
        emitter.emit(EventType::SyncStart, &serde_json::json!({"mode":"test"}));
    }
}
