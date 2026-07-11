use crate::crypto::CryptoProvider;
use crate::crypto::age_crypto::AgeCrypto;
use crate::crypto::envelope::{EnvelopeCrypto, EnvelopeKdfParams};
use crate::crypto::noop::NoopCrypto;
use crate::event::{EventEmitter, EventType};
use crate::local_db;
use crate::model::{
    BookNote, BookReadingMeta, Bookmark, Highlight, MetaEdit, ReadingProgress, decode_meta,
    encode_meta, meta_hash,
};
use crate::storage::RemoteStorage;
use crate::storage::RemoteVersion;
use crate::storage::file::FileStorage;
use crate::storage::s3::{S3Config, S3Storage};
use crate::storage::webdav::{WebDavConfig, WebDavStorage};
use crate::tombstone::{Revival, Tombstone, TombstoneItemType, TombstoneSet};
use crate::{Result, SyncError};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StorageConfigJson {
    #[serde(rename = "type")]
    pub storage_type: Option<String>,
    pub root_dir: Option<PathBuf>,
    pub endpoint: Option<String>,
    pub bucket: Option<String>,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub region: Option<String>,
    pub root_prefix: Option<String>,
    pub path_style: Option<bool>,
    pub allow_http: Option<bool>,
    pub url: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct EncryptionConfigJson {
    #[serde(rename = "type")]
    pub encryption_type: Option<String>,
    pub identity: Option<String>,
    pub passphrase: Option<String>,
    pub kek_hex: Option<String>,
    pub kek_id: Option<String>,
    pub kek_version: Option<u32>,
    pub argon2_memory_cost: Option<u32>,
    pub argon2_time_cost: Option<u32>,
    pub argon2_parallelism: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct KmoSyncConfig {
    pub storage_config_json: String,
    pub encryption_config_json: String,
    pub device_id: String,
    pub local_cache_dir: PathBuf,
}

pub struct KmoSyncFacade {
    config: KmoSyncConfig,
    db: Mutex<Connection>,
    crypto: Mutex<Arc<dyn CryptoProvider>>,
    envelope_crypto: Mutex<Option<EnvelopeCrypto>>,
    storage: Arc<dyn RemoteStorage>,
    runtime: tokio::runtime::Runtime,
    events: EventEmitter,
    scheduler: Mutex<SchedulerState>,
    active_remote_namespace: Mutex<Option<String>>,
    operation_lock: Mutex<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SyncMode {
    Bidirectional,
    PushOnly,
    PullOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum NetworkType {
    Wifi,
    Cellular,
    Unknown,
}

impl TryFrom<i32> for NetworkType {
    type Error = SyncError;

    fn try_from(value: i32) -> Result<Self> {
        match value {
            0 => Ok(Self::Wifi),
            1 => Ok(Self::Cellular),
            2 => Ok(Self::Unknown),
            other => Err(SyncError::InvalidArg(format!(
                "invalid network type: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone)]
struct SchedulerState {
    network_type: NetworkType,
    blob_byte_limit: Option<u64>,
    blob_paused: bool,
}

impl Default for SchedulerState {
    fn default() -> Self {
        Self {
            network_type: NetworkType::Wifi,
            blob_byte_limit: None,
            blob_paused: false,
        }
    }
}

impl TryFrom<i32> for SyncMode {
    type Error = SyncError;

    fn try_from(value: i32) -> Result<Self> {
        match value {
            0 => Ok(Self::Bidirectional),
            1 => Ok(Self::PushOnly),
            2 => Ok(Self::PullOnly),
            other => Err(SyncError::InvalidArg(format!("invalid sync mode: {other}"))),
        }
    }
}

const REMOTE_PROTOCOL_VERSION: u32 = 7;
/// Reeden-style flat layout: `kmo-sync/books/<hash>`, `kmo-sync/book_progress/<hash>.json`,
/// `kmo-sync/bookmarks/<hash>.json`. No shared sync header, no CAS, no tombstones.
const EDIT_HISTORY_INLINE_LIMIT: usize = 1000;
const EDIT_HISTORY_RETAINED_INLINE: usize = 100;

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RemoteProgress {
    schema_version: u32,
    book_hash: String,
    progress: Option<ReadingProgress>,
    last_writer_device_id: String,
    last_write_ts: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RemoteBookmarks {
    schema_version: u32,
    book_hash: String,
    bookmarks: Vec<Bookmark>,
    highlights: Vec<Highlight>,
    notes: Vec<BookNote>,
    last_writer_device_id: String,
    last_write_ts: i64,
}

struct VersionedRemote<T> {
    value: Option<T>,
    version: Option<RemoteVersion>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ActiveRemoteNamespace {
    namespace: String,
    requires_envelope_encryption: bool,
}

impl KmoSyncFacade {
    fn crypto(&self) -> Arc<dyn CryptoProvider> {
        self.crypto.lock().expect("crypto mutex poisoned").clone()
    }

    pub fn create(config: KmoSyncConfig, events: EventEmitter) -> Result<Self> {
        let storage_config: StorageConfigJson =
            validate_json(&config.storage_config_json, "storage_config_json")?;
        let encryption: EncryptionConfigJson =
            validate_json(&config.encryption_config_json, "encryption_config_json")?;

        if config.device_id.trim().is_empty() {
            return Err(SyncError::InvalidArg("device_id is empty".to_string()));
        }
        if config.local_cache_dir.as_os_str().is_empty() {
            return Err(SyncError::InvalidArg(
                "local_cache_dir is empty".to_string(),
            ));
        }

        let db = local_db::open_database(Path::new(&config.local_cache_dir))?;
        let crypto = build_crypto(&encryption)?;
        let envelope_crypto = if encryption_type_is_envelope(&encryption) {
            Some(build_envelope_crypto(&encryption)?)
        } else {
            None
        };
        register_envelope_kek_version(&db, &encryption)?;
        let storage = build_storage(&storage_config, &config.local_cache_dir)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        detect_clock_drift(&db, &events)?;
        if !crypto.is_encrypted() {
            events.emit(
                EventType::SecurityWarning,
                &serde_json::json!({"message":"Encryption disabled"}),
            );
        }

        Ok(Self {
            config,
            db: Mutex::new(db),
            crypto: Mutex::new(crypto),
            envelope_crypto: Mutex::new(envelope_crypto),
            storage,
            runtime,
            events,
            scheduler: Mutex::new(SchedulerState::default()),
            active_remote_namespace: Mutex::new(None),
            operation_lock: Mutex::new(()),
        })
    }

    pub fn sync_all(&self, mode: i32) -> Result<()> {
        let _operation = self
            .operation_lock
            .lock()
            .map_err(|_| SyncError::Internal("operation mutex poisoned".to_string()))?;
        let mode = SyncMode::try_from(mode)?;
        self.events.emit(
            EventType::SyncStart,
            &serde_json::json!({"operation":"sync_all","mode":mode}),
        );
        self.runtime.block_on(self.sync_all_inner(mode))?;
        self.events.emit(
            EventType::SyncProgress,
            &serde_json::json!({"phase":"complete","progress":1.0}),
        );
        self.events.emit(
            EventType::SyncComplete,
            &serde_json::json!({"success":true}),
        );
        Ok(())
    }

    pub fn sync_single_meta(&self, book_hash: &str, meta_id: &str) -> Result<()> {
        let _operation = self
            .operation_lock
            .lock()
            .map_err(|_| SyncError::Internal("operation mutex poisoned".to_string()))?;
        validate_identifier(book_hash, "book_hash")?;
        validate_identifier(meta_id, "meta_id")?;
        self.events.emit(
            EventType::SyncStart,
            &serde_json::json!({
                "operation":"sync_single_meta",
                "book_hash":book_hash,
                "meta_id":meta_id
            }),
        );
        self.runtime.block_on(self.sync_single_meta_inner(
            book_hash,
            meta_id,
            SyncMode::Bidirectional,
        ))?;
        self.events.emit(
            EventType::BookChanged,
            &serde_json::json!({
                "book_hash":book_hash,
                "meta_id":meta_id,
                "change_type":"MetaData"
            }),
        );
        self.events.emit(
            EventType::SyncComplete,
            &serde_json::json!({"success":true}),
        );
        Ok(())
    }

    pub fn sync_book(&self, book_hash: &str) -> Result<()> {
        let _operation = self
            .operation_lock
            .lock()
            .map_err(|_| SyncError::Internal("operation mutex poisoned".to_string()))?;
        validate_identifier(book_hash, "book_hash")?;
        let decision = self.blob_policy_decision()?;
        if !decision.allowed {
            return Err(SyncError::Network(format!(
                "blob sync is paused: {}",
                decision.reason
            )));
        }
        self.events.emit(
            EventType::SyncStart,
            &serde_json::json!({"operation":"sync_book","book_hash":book_hash}),
        );
        self.runtime
            .block_on(self.sync_book_inner(book_hash, SyncMode::Bidirectional))?;
        self.events.emit(
            EventType::BookChanged,
            &serde_json::json!({"book_hash":book_hash,"change_type":"BlobFile"}),
        );
        self.events.emit(
            EventType::SyncComplete,
            &serde_json::json!({"success":true}),
        );
        Ok(())
    }

    pub fn set_network_type(&self, network_type: i32) -> Result<()> {
        let network_type = NetworkType::try_from(network_type)?;
        let mut scheduler = self
            .scheduler
            .lock()
            .map_err(|_| SyncError::Internal("scheduler mutex poisoned".to_string()))?;
        scheduler.network_type = network_type;
        Ok(())
    }

    pub fn set_blob_byte_limit(&self, byte_limit: i64) -> Result<()> {
        let limit = if byte_limit < 0 {
            None
        } else {
            Some(byte_limit as u64)
        };
        let mut scheduler = self
            .scheduler
            .lock()
            .map_err(|_| SyncError::Internal("scheduler mutex poisoned".to_string()))?;
        scheduler.blob_byte_limit = limit;
        Ok(())
    }

    pub fn pause(&self) -> Result<()> {
        let mut scheduler = self
            .scheduler
            .lock()
            .map_err(|_| SyncError::Internal("scheduler mutex poisoned".to_string()))?;
        scheduler.blob_paused = true;
        Ok(())
    }

    pub fn resume(&self) -> Result<()> {
        let mut scheduler = self
            .scheduler
            .lock()
            .map_err(|_| SyncError::Internal("scheduler mutex poisoned".to_string()))?;
        scheduler.blob_paused = false;
        Ok(())
    }

    pub fn put_local_book(&self, book_hash: &str, local_file_path: &Path) -> Result<()> {
        validate_identifier(book_hash, "book_hash")?;
        if !local_file_path.is_file() {
            return Err(SyncError::InvalidArg(format!(
                "local_file_path is not a file: {}",
                local_file_path.display()
            )));
        }
        let actual_hash = blake3_file_hex(local_file_path)?;
        if actual_hash != book_hash {
            return Err(SyncError::InvalidArg(format!(
                "book_hash mismatch: expected {book_hash}, got {actual_hash}"
            )));
        }
        let metadata = std::fs::metadata(local_file_path)?;
        self.upsert_blob_index(
            book_hash,
            metadata.len() as i64,
            None,
            0,
            &local_file_path.to_string_lossy(),
        )
    }

    pub fn get_local_meta_json(&self, meta_id: &str) -> Result<String> {
        validate_identifier(meta_id, "meta_id")?;

        match self.read_local_meta_priv(meta_id)? {
            Some(meta) => Ok(serde_json::to_string(&meta)?),
            None => Ok(serde_json::to_string(&serde_json::json!({
                "meta_id": meta_id,
                "exists": false
            }))?),
        }
    }

    pub fn put_local_meta(&self, meta: &BookReadingMeta) -> Result<()> {
        validate_identifier(&meta.meta_id, "meta_id")?;
        validate_identifier(&meta.book_hash, "book_hash")?;
        let meta = self.meta_with_archived_history(meta)?;
        let bytes = encode_meta(&meta)?;
        let path = self.local_meta_path(&meta.meta_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)?;
        Ok(())
    }

    pub fn read_local_meta(&self, meta_id: &str) -> Result<Option<BookReadingMeta>> {
        validate_identifier(meta_id, "meta_id")?;
        self.read_local_meta_priv(meta_id)
    }

    pub fn local_cache_dir(&self) -> &Path {
        &self.config.local_cache_dir
    }

    pub fn resolve_meta_conflict(&self, meta_id: &str, chosen_version: &str) -> Result<()> {
        let _operation = self
            .operation_lock
            .lock()
            .map_err(|_| SyncError::Internal("operation mutex poisoned".to_string()))?;
        if meta_id.trim().is_empty() {
            return Err(SyncError::InvalidArg("meta_id is empty".to_string()));
        }
        let chosen_version = chosen_version.trim().to_ascii_lowercase();
        if chosen_version != "local" && chosen_version != "remote" {
            return Err(SyncError::InvalidArg(
                "chosen_version must be 'local' or 'remote'".to_string(),
            ));
        }

        // Reeden LWW auto-resolves concurrent writes; explicit conflict
        // records are no longer produced. If a caller still invokes this (to
        // force a known-remote version into the cache) and no conflict record
        // exists, fall back to re-running a pull-only sync of the meta so the
        // local cache picks up whatever is on the remote side.
        let payload = match self.pending_meta_conflict_payload(meta_id) {
            Ok(p) => p,
            Err(_) => {
                if chosen_version == "local" {
                    // Local is already authoritative — nothing to do.
                    return Ok(());
                }
                let meta = self.read_local_meta(meta_id)?.ok_or_else(|| {
                    SyncError::InvalidArg(format!("no local meta cached for {meta_id}"))
                })?;
                self.runtime.block_on(self.sync_single_meta_inner(
                    &meta.book_hash,
                    meta_id,
                    SyncMode::PullOnly,
                ))?;
                self.events.emit(
                    EventType::SyncProgress,
                    &serde_json::json!({
                        "phase":"conflict_resolved",
                        "object_type":"meta",
                        "object_id":meta_id,
                        "chosen_version":"remote",
                        "noop_lww":true,
                    }),
                );
                return Ok(());
            }
        };
        let (conflict_id, local_json, remote_json) = payload;
        let chosen_json = if chosen_version == "local" {
            local_json
        } else {
            remote_json
        };
        let chosen: BookReadingMeta = serde_json::from_str(&chosen_json)?;
        self.put_local_meta(&chosen)?;
        let hash = hex::encode(meta_hash(&chosen)?);
        self.runtime.block_on(self.write_remote_progress(&chosen))?;
        self.runtime
            .block_on(self.write_remote_bookmarks(&chosen))?;
        self.upsert_meta_index(&chosen, hash, chosen.modified_ts)?;

        let db = self
            .db
            .lock()
            .map_err(|_| SyncError::Internal("database mutex poisoned".to_string()))?;
        db.execute(
            "UPDATE conflict_log SET resolved_ts = ?1 WHERE id = ?2",
            rusqlite::params![now_millis(), conflict_id],
        )?;
        drop(db);

        self.events.emit(
            EventType::SyncProgress,
            &serde_json::json!({
                "phase":"conflict_resolved",
                "object_type":"meta",
                "object_id":meta_id,
                "chosen_version":chosen_version
            }),
        );
        Ok(())
    }

    pub fn mark_meta_item_deleted(
        &self,
        meta_id: &str,
        item_type: &str,
        item_uuid: &str,
    ) -> Result<()> {
        if meta_id.trim().is_empty() {
            return Err(SyncError::InvalidArg("meta_id is empty".to_string()));
        }
        if item_uuid.trim().is_empty() {
            return Err(SyncError::InvalidArg("item_uuid is empty".to_string()));
        }
        let item_type = TombstoneItemType::parse(item_type)?;
        let mut meta = self
            .read_local_meta(meta_id)?
            .ok_or_else(|| SyncError::InvalidArg(format!("local meta not found: {meta_id}")))?;
        let deleted_item_json = snapshot_meta_item(&meta, &item_type, item_uuid)?;
        remove_meta_item(&mut meta, &item_type, item_uuid);
        meta.logical_ts += 1;
        meta.modified_ts = meta.modified_ts.max(meta.logical_ts);
        meta.wall_clock_ts = now_millis();
        meta.device_id = self.device_id().to_string();
        self.put_local_meta(&meta)?;

        // Tombstones are kept local-only; the deletion itself rides along inside
        // the next bookmarks sync (the item was already removed from `meta`
        // above by remove_meta_item, so the lww PUT will overwrite remote).
        let mut set = TombstoneSet::default();
        let local_tombstones = self.local_tombstone_path(meta_id);
        if local_tombstones.exists() {
            set = TombstoneSet::decode(&std::fs::read(&local_tombstones)?)?;
        }
        let mut tombstone = Tombstone::new(
            item_uuid.to_string(),
            item_type,
            meta.logical_ts,
            self.device_id().to_string(),
        );
        tombstone.deleted_item_json = Some(deleted_item_json);
        set.mark_deleted(tombstone);
        set.gc_expired();
        self.write_local_tombstones(meta_id, &set)?;
        self.events.emit(
            EventType::BookChanged,
            &serde_json::json!({
                "meta_id":meta_id,
                "change_type":"TombstoneAdded",
                "item_uuid":item_uuid
            }),
        );
        Ok(())
    }

    pub fn undo_deletion(&self, meta_id: &str, item_uuid: &str) -> Result<()> {
        if meta_id.trim().is_empty() {
            return Err(SyncError::InvalidArg("meta_id is empty".to_string()));
        }
        if item_uuid.trim().is_empty() {
            return Err(SyncError::InvalidArg("item_uuid is empty".to_string()));
        }
        let mut meta = self
            .read_local_meta(meta_id)?
            .ok_or_else(|| SyncError::InvalidArg(format!("local meta not found: {meta_id}")))?;
        let mut set = TombstoneSet::default();
        let local_path = self.local_tombstone_path(meta_id);
        if local_path.exists() {
            set = TombstoneSet::decode(&std::fs::read(&local_path)?)?;
        }
        // 找到要撤销的 tombstone，提取 item_type 和 deleted_at_logical_ts
        let tombstone = set
            .tombstones
            .iter()
            .find(|t| t.uuid == item_uuid)
            .cloned()
            .ok_or_else(|| SyncError::InvalidArg(format!("tombstone not found: {item_uuid}")))?;
        restore_meta_item(&mut meta, &tombstone)?;
        meta.logical_ts = meta.logical_ts.max(tombstone.deleted_at_logical_ts) + 1;
        meta.modified_ts = meta.modified_ts.max(meta.logical_ts);
        meta.wall_clock_ts = now_millis();
        meta.device_id = self.device_id().to_string();
        self.put_local_meta(&meta)?;
        // 移除本地 tombstone
        set.revive(item_uuid);
        // 记录 revival，使远端的同名 tombstone 在过滤时不生效
        set.add_revival(Revival {
            uuid: item_uuid.to_string(),
            item_type: tombstone.item_type,
            revived_at_logical_ts: meta.logical_ts,
            revived_at_wall_ts: now_millis(),
            revived_by_device: self.device_id().to_string(),
        });
        set.gc_expired();
        self.write_local_tombstones(meta_id, &set)?;
        self.events.emit(
            EventType::TombstoneRevival,
            &serde_json::json!({"meta_id":meta_id,"item_uuid":item_uuid}),
        );
        Ok(())
    }

    pub fn resolve_tombstone_revival(
        &self,
        meta_id: &str,
        item_uuid: &str,
        resolution: &str,
    ) -> Result<()> {
        if meta_id.trim().is_empty() {
            return Err(SyncError::InvalidArg("meta_id is empty".to_string()));
        }
        if item_uuid.trim().is_empty() {
            return Err(SyncError::InvalidArg("item_uuid is empty".to_string()));
        }
        let resolution = resolution.trim().to_ascii_lowercase();
        if resolution != "delete" && resolution != "restore" {
            return Err(SyncError::InvalidArg(
                "resolution must be 'delete' or 'restore'".to_string(),
            ));
        }

        // Tombstone revival is a local-only concept in the reeden layout: the
        // remote LWW merge already absorbs both sides, so callers that used to
        // trigger this path after a tombstone was revived by another device
        // now only need to keep the local tombstone bookkeeping in sync.
        if self
            .pending_tombstone_conflict_payload(meta_id, item_uuid)
            .is_err()
        {
            // Nothing to resolve against — leave local tombstones untouched
            // and report success so the API stays a no-op under LWW.
            self.events.emit(
                EventType::TombstoneRevival,
                &serde_json::json!({
                    "meta_id": meta_id,
                    "item_uuid": item_uuid,
                    "resolution": resolution,
                    "noop": true,
                }),
            );
            return Ok(());
        }

        let (conflict_id, tombstone_json, meta_json) =
            self.pending_tombstone_conflict_payload(meta_id, item_uuid)?;
        let tombstone: Tombstone = serde_json::from_str(&tombstone_json)?;
        let mut incoming_meta: BookReadingMeta = serde_json::from_str(&meta_json)?;

        if resolution == "restore" {
            let mut set = TombstoneSet::default();
            let local_path = self.local_tombstone_path(meta_id);
            if local_path.exists() {
                set = TombstoneSet::decode(&std::fs::read(&local_path)?)?;
            }
            set.revive(item_uuid);
            self.write_local_tombstones(meta_id, &set)?;
            self.put_local_meta(&incoming_meta)?;
            self.runtime
                .block_on(self.write_remote_progress(&incoming_meta))?;
            self.runtime
                .block_on(self.write_remote_bookmarks(&incoming_meta))?;
            let hash = hex::encode(meta_hash(&incoming_meta)?);
            self.upsert_meta_index(&incoming_meta, hash, incoming_meta.modified_ts)?;
        } else {
            remove_meta_item(&mut incoming_meta, &tombstone.item_type, item_uuid);
            self.put_local_meta(&incoming_meta)?;
            self.runtime
                .block_on(self.write_remote_progress(&incoming_meta))?;
            self.runtime
                .block_on(self.write_remote_bookmarks(&incoming_meta))?;
            let hash = hex::encode(meta_hash(&incoming_meta)?);
            self.upsert_meta_index(&incoming_meta, hash, incoming_meta.modified_ts)?;

            let mut set = TombstoneSet::default();
            let local_path = self.local_tombstone_path(meta_id);
            if local_path.exists() {
                set = TombstoneSet::decode(&std::fs::read(&local_path)?)?;
            }
            set.mark_deleted(tombstone);
            self.write_local_tombstones(meta_id, &set)?;
        }

        self.mark_conflict_resolved(conflict_id)?;
        self.events.emit(
            EventType::SyncProgress,
            &serde_json::json!({
                "phase":"tombstone_revival_resolved",
                "meta_id":meta_id,
                "item_uuid":item_uuid,
                "resolution":resolution
            }),
        );
        Ok(())
    }

    pub fn resolve_blob_conflict(&self, book_hash: &str, chosen_version: &str) -> Result<()> {
        let _operation = self
            .operation_lock
            .lock()
            .map_err(|_| SyncError::Internal("operation mutex poisoned".to_string()))?;
        if book_hash.trim().is_empty() {
            return Err(SyncError::InvalidArg("book_hash is empty".to_string()));
        }
        let chosen_version = chosen_version.trim().to_ascii_lowercase();
        if chosen_version != "local" && chosen_version != "remote" {
            return Err(SyncError::InvalidArg(
                "chosen_version must be 'local' or 'remote'".to_string(),
            ));
        }

        let conflict_id = self.pending_blob_conflict_id(book_hash)?;
        if chosen_version == "local" {
            let indexed_path = self.indexed_local_book_path(book_hash)?;
            let local_path = self.local_book_path(book_hash);
            let source_path = indexed_path.as_deref().unwrap_or(&local_path);
            if !source_path.exists() {
                return Err(SyncError::Conflict(format!(
                    "local blob not found for {book_hash}"
                )));
            }
            let actual_hash = blake3_file_hex(source_path)?;
            if actual_hash != book_hash {
                return Err(SyncError::Conflict(format!(
                    "local blob hash mismatch: expected {book_hash}, got {actual_hash}"
                )));
            }
            let bytes = std::fs::read(source_path)?;
            let encrypted = self.crypto().encrypt(&bytes)?;
            let remote_path = self.remote_book_path(book_hash);
            self.runtime
                .block_on(self.storage.write_object(&remote_path, &encrypted))?;
            if source_path != local_path {
                if let Some(parent) = local_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(source_path, &local_path)?;
            }
            let stat = self.runtime.block_on(self.storage.stat(&remote_path))?;
            self.upsert_blob_index(
                book_hash,
                stat.size as i64,
                stat.etag.as_deref(),
                stat.mtime,
                &local_path.to_string_lossy(),
            )?;
        }

        self.mark_conflict_resolved(conflict_id)?;
        self.events.emit(
            EventType::SyncProgress,
            &serde_json::json!({
                "phase":"blob_conflict_resolved",
                "book_hash":book_hash,
                "chosen_version":chosen_version
            }),
        );
        Ok(())
    }

    pub fn rotate_envelope_kek(&self, new_encryption_config_json: &str) -> Result<usize> {
        let _operation = self
            .operation_lock
            .lock()
            .map_err(|_| SyncError::Internal("operation mutex poisoned".to_string()))?;
        let new_encryption: EncryptionConfigJson =
            validate_json(new_encryption_config_json, "new_encryption_config_json")?;
        if !encryption_type_is_envelope(&new_encryption) {
            return Err(SyncError::InvalidArg(
                "new encryption config must use envelope encryption".to_string(),
            ));
        }
        let new_crypto = build_envelope_crypto(&new_encryption)?;
        let old_crypto = self
            .envelope_crypto
            .lock()
            .expect("envelope crypto mutex poisoned")
            .clone()
            .ok_or_else(|| {
                SyncError::InvalidArg(
                    "current sync instance is not using envelope encryption".to_string(),
                )
            })?;

        let rewrapped = self
            .runtime
            .block_on(self.rotate_envelope_kek_inner(&old_crypto, &new_crypto))?;
        {
            let db = self
                .db
                .lock()
                .map_err(|_| SyncError::Internal("database mutex poisoned".to_string()))?;
            register_envelope_kek_version(&db, &new_encryption)?;
        }
        {
            let mut envelope_crypto = self
                .envelope_crypto
                .lock()
                .expect("envelope crypto mutex poisoned");
            *envelope_crypto = Some(new_crypto.clone());
        }
        {
            let mut crypto = self.crypto.lock().expect("crypto mutex poisoned");
            *crypto = Arc::new(new_crypto);
        }
        self.events.emit(
            EventType::SecurityWarning,
            &serde_json::json!({
                "message":"Envelope KEK rotated",
                "rewrapped_objects":rewrapped
            }),
        );
        Ok(rewrapped)
    }

    fn upsert_meta_index(&self, meta: &BookReadingMeta, hash: String, sync_ts: i64) -> Result<()> {
        let db = self
            .db
            .lock()
            .map_err(|_| SyncError::Internal("database mutex poisoned".to_string()))?;
        db.execute(
            "INSERT INTO meta_index(meta_id, book_hash, last_meta_hash, last_sync_ts)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(meta_id) DO UPDATE SET
               book_hash = excluded.book_hash,
               last_meta_hash = excluded.last_meta_hash,
               last_sync_ts = excluded.last_sync_ts",
            rusqlite::params![meta.meta_id, meta.book_hash, hash, sync_ts],
        )?;
        Ok(())
    }

    fn read_local_meta_priv(&self, meta_id: &str) -> Result<Option<BookReadingMeta>> {
        let path = self.local_meta_path(meta_id);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(decode_meta(&std::fs::read(path)?)?))
    }

    fn local_meta_path(&self, meta_id: &str) -> PathBuf {
        self.config
            .local_cache_dir
            .join("metas")
            .join(format!("{meta_id}.meta"))
    }

    fn local_tombstone_path(&self, meta_id: &str) -> PathBuf {
        self.config
            .local_cache_dir
            .join("metas")
            .join(format!("{meta_id}.tombstones.json"))
    }

    fn read_local_tombstones(&self, meta_id: &str) -> Result<TombstoneSet> {
        let path = self.local_tombstone_path(meta_id);
        if !path.exists() {
            return Ok(TombstoneSet::default());
        }
        TombstoneSet::decode(&std::fs::read(path)?)
    }

    #[allow(dead_code)]
    fn read_local_tombstones_for_test(&self, meta_id: &str) -> Result<TombstoneSet> {
        self.read_local_tombstones(meta_id)
    }

    fn write_local_tombstones(&self, meta_id: &str, set: &TombstoneSet) -> Result<()> {
        let path = self.local_tombstone_path(meta_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, set.encode()?)?;
        Ok(())
    }

    fn local_history_path(&self, meta_id: &str) -> PathBuf {
        self.config
            .local_cache_dir
            .join("metas")
            .join(format!("{meta_id}.history.zst"))
    }

    fn meta_with_archived_history(&self, meta: &BookReadingMeta) -> Result<BookReadingMeta> {
        if meta.edit_history.len() <= EDIT_HISTORY_INLINE_LIMIT {
            return Ok(meta.clone());
        }

        let archive_path = self.local_history_path(&meta.meta_id);
        if let Some(parent) = archive_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let archive = encode_edit_history_archive(&meta.edit_history)?;
        std::fs::write(archive_path, archive)?;

        let mut compact = meta.clone();
        let keep_from = compact
            .edit_history
            .len()
            .saturating_sub(EDIT_HISTORY_RETAINED_INLINE);
        compact.edit_history = compact.edit_history.split_off(keep_from);
        Ok(compact)
    }

    async fn sync_all_inner(&self, mode: SyncMode) -> Result<()> {
        self.refresh_active_remote_namespace().await?;
        let meta_pairs = self.discover_meta_pairs(mode).await?;
        for (book_hash, meta_id) in meta_pairs {
            self.sync_single_meta_inner(&book_hash, &meta_id, mode)
                .await?;
        }
        self.events.emit(
            EventType::SyncProgress,
            &serde_json::json!({"phase":"meta","progress":1.0}),
        );

        let decision = self.blob_policy_decision()?;
        if !decision.allowed {
            self.events.emit(
                EventType::SyncProgress,
                &serde_json::json!({
                    "phase":"blob",
                    "progress":1.0,
                    "state":"paused",
                    "reason":decision.reason
                }),
            );
            return Ok(());
        }

        let mut transferred_bytes = 0_u64;
        let book_hashes = self.discover_book_hashes(mode).await?;
        for book_hash in book_hashes {
            let estimate = self.estimate_book_transfer_size(&book_hash, mode).await?;
            if let Some(limit) = decision.byte_limit
                && transferred_bytes.saturating_add(estimate) > limit
            {
                self.events.emit(
                    EventType::SyncProgress,
                    &serde_json::json!({
                        "phase":"blob",
                        "progress":1.0,
                        "state":"paused",
                        "reason":"byte_limit",
                        "byte_limit":limit,
                        "transferred_bytes":transferred_bytes
                    }),
                );
                return Ok(());
            }
            self.sync_book_inner(&book_hash, mode).await?;
            transferred_bytes = transferred_bytes.saturating_add(estimate);
        }
        self.events.emit(
            EventType::SyncProgress,
            &serde_json::json!({
                "phase":"blob",
                "progress":1.0,
                "transferred_bytes":transferred_bytes
            }),
        );
        Ok(())
    }

    async fn rotate_envelope_kek_inner(
        &self,
        old_crypto: &EnvelopeCrypto,
        new_crypto: &EnvelopeCrypto,
    ) -> Result<usize> {
        self.refresh_active_remote_namespace().await?;
        let mut remote_paths = Vec::new();
        for prefix in ["book_progress", "bookmarks", "books", "tombstones"] {
            let active_prefix = self.active_remote_path(prefix);
            for info in self.storage.list_prefix(&active_prefix).await? {
                if let Some(logical) = self.strip_active_remote_path(&info.path) {
                    remote_paths.push((logical, info.path));
                }
            }
        }
        let namespace = format!("kek-v{}-{}", new_crypto.kek_version(), now_millis());
        let mut rewrapped = 0usize;
        for (logical_path, active_path) in remote_paths {
            let input = self.transfer_path("rewrap-input");
            let output = self.transfer_path("rewrap-output");
            if let Some(parent) = input.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut remote = self.storage.download_large(&active_path).await?;
            let mut local = tokio::fs::File::create(&input).await?;
            tokio::io::copy(&mut remote, &mut local).await?;
            drop(local);
            old_crypto.rewrap_file(new_crypto, &input, &output)?;
            let size = std::fs::metadata(&output)?.len();
            let output_file = tokio::fs::File::open(&output).await?;
            let upload_result = self
                .storage
                .upload_large(
                    &format!("{namespace}/{logical_path}"),
                    Box::new(output_file),
                    size,
                )
                .await;
            let _ = std::fs::remove_file(input);
            let _ = std::fs::remove_file(output);
            upload_result?;
            rewrapped += 1;
        }
        let marker_path = "_active_namespace.json";
        let marker = self.storage.read_object_versioned(marker_path).await?;
        let payload = serde_json::to_vec(&ActiveRemoteNamespace {
            namespace: namespace.clone(),
            requires_envelope_encryption: true,
        })?;
        if !self
            .storage
            .write_object_conditional(
                marker_path,
                &payload,
                marker.as_ref().map(|value| &value.version),
            )
            .await?
        {
            return Err(SyncError::Conflict(
                "another device changed the active encryption namespace".to_string(),
            ));
        }
        *self
            .active_remote_namespace
            .lock()
            .map_err(|_| SyncError::Internal("namespace mutex poisoned".to_string()))? =
            Some(namespace);
        Ok(rewrapped)
    }

    async fn sync_single_meta_inner(
        &self,
        book_hash: &str,
        meta_id: &str,
        mode: SyncMode,
    ) -> Result<()> {
        validate_identifier(book_hash, "book_hash")?;
        validate_identifier(meta_id, "meta_id")?;
        self.refresh_active_remote_namespace().await?;
        const MAX_CAS_ATTEMPTS: usize = 8;
        for _ in 0..MAX_CAS_ATTEMPTS {
            if self
                .sync_single_meta_attempt(book_hash, meta_id, mode)
                .await?
            {
                return Ok(());
            }
            tokio::task::yield_now().await;
        }
        Err(SyncError::Conflict(format!(
            "metadata kept changing while syncing book {book_hash}"
        )))
    }

    async fn sync_single_meta_attempt(
        &self,
        book_hash: &str,
        meta_id: &str,
        mode: SyncMode,
    ) -> Result<bool> {
        let _ = self.remote_progress_path(book_hash);
        let _ = self.remote_bookmarks_path(book_hash);

        let local = if mode != SyncMode::PullOnly {
            self.read_local_meta_priv(meta_id)?
        } else {
            None
        };

        let versioned_progress = self.read_remote_progress(book_hash).await?;
        let versioned_bookmarks = self.read_remote_bookmarks(book_hash).await?;
        let remote_progress = if mode != SyncMode::PushOnly {
            versioned_progress.value.as_ref()
        } else {
            None
        };
        let remote_bookmarks = if mode != SyncMode::PushOnly {
            versioned_bookmarks.value.as_ref()
        } else {
            None
        };

        // --- Tombstone 同步：读本地 + 远端，合并后用于过滤已删除条目 ---
        let local_tombstones = self.read_local_tombstones(meta_id)?;
        let versioned_tombstones = self.read_remote_tombstones(book_hash).await?;
        let remote_tombstones = if mode != SyncMode::PushOnly {
            versioned_tombstones.value.as_ref()
        } else {
            None
        };
        let mut merged_tombstones = local_tombstones.clone();
        if let Some(rt) = remote_tombstones {
            merged_tombstones.merge(rt.clone());
        }
        merged_tombstones.gc_expired();

        let tombstone_local_changed = merged_tombstones != local_tombstones;
        if tombstone_local_changed {
            self.write_local_tombstones(meta_id, &merged_tombstones)?;
        }
        let tombstone_remote_changed = match remote_tombstones {
            None => !merged_tombstones.is_empty(),
            Some(rt) => rt != &merged_tombstones,
        };
        if tombstone_remote_changed
            && mode != SyncMode::PullOnly
            && !self
                .write_remote_tombstones_conditional(
                    book_hash,
                    &merged_tombstones,
                    versioned_tombstones.version.as_ref(),
                )
                .await?
        {
            return Ok(false);
        }

        let mut merged = merge_meta_with_remote(local.clone(), remote_progress, remote_bookmarks);

        // Keep book_hash / meta_id / origin_device_id populated so the meta
        // stays reachable by the caller's id slot. The new protocol stores
        // both per book_hash, but legacy callers still pass a separate meta_id.
        if merged.book_hash.is_empty() {
            merged.book_hash = book_hash.to_string();
        }
        if merged.meta_id.is_empty() {
            merged.meta_id = meta_id.to_string();
        }
        if merged.origin_device_id.is_empty() {
            merged.origin_device_id = if merged.device_id.is_empty() {
                self.device_id().to_string()
            } else {
                merged.device_id.clone()
            };
        }

        // 用合并后的 tombstone 过滤已删除条目（已被 revival 撤销的保留）
        Self::apply_tombstones_to_meta(&mut merged, &merged_tombstones);

        let remote_matches = remote_progress_matches(&merged, remote_progress)
            && remote_bookmarks_matches(&merged, remote_bookmarks);
        let local_matches = local.as_ref() == Some(&merged);

        if !remote_matches && mode != SyncMode::PullOnly {
            if !progress_matches_remote(&merged, remote_progress)
                && !self
                    .write_remote_progress_conditional(&merged, versioned_progress.version.as_ref())
                    .await?
            {
                return Ok(false);
            }
            if !bookmarks_matches_remote(&merged, remote_bookmarks)
                && !self
                    .write_remote_bookmarks_conditional(
                        &merged,
                        versioned_bookmarks.version.as_ref(),
                    )
                    .await?
            {
                return Ok(false);
            }
        }

        if !local_matches && mode != SyncMode::PushOnly {
            self.put_local_meta(&merged)?;
        }

        let hash = hex::encode(meta_hash(&merged)?);
        self.upsert_meta_index(&merged, hash, merged.modified_ts)?;
        Ok(true)
    }

    /// 用 tombstone set 过滤 meta 中已删除且未被 revive 的条目。
    /// 在 merge_meta_with_remote 之后调用，确保 union 救回的已删除条目被清除。
    fn apply_tombstones_to_meta(meta: &mut BookReadingMeta, set: &TombstoneSet) {
        for tombstone in &set.tombstones {
            // 如果该 tombstone 被 revival 撤销，跳过（保留条目）
            if set.is_revived(tombstone) {
                continue;
            }
            match tombstone.item_type {
                TombstoneItemType::Bookmark => {
                    meta.bookmarks.retain(|b| b.bookmark_id != tombstone.uuid);
                }
                TombstoneItemType::Highlight => {
                    meta.highlights.retain(|h| h.highlight_id != tombstone.uuid);
                }
                TombstoneItemType::Note => {
                    meta.notes.retain(|n| n.note_id != tombstone.uuid);
                }
            }
        }
    }

    #[allow(dead_code)]
    fn remote_book_dir(book_hash: &str) -> String {
        format!("books/{book_hash}")
    }

    async fn refresh_active_remote_namespace(&self) -> Result<()> {
        let namespace = match self
            .storage
            .read_object_optional("_active_namespace.json")
            .await?
        {
            Some(bytes) => {
                let marker: ActiveRemoteNamespace = serde_json::from_slice(&bytes)?;
                validate_identifier(&marker.namespace, "active namespace")?;
                if marker.requires_envelope_encryption && !self.crypto().uses_envelope_encryption()
                {
                    return Err(SyncError::Crypto(
                        "remote namespace requires envelope encryption".to_string(),
                    ));
                }
                Some(marker.namespace)
            }
            None => None,
        };
        *self
            .active_remote_namespace
            .lock()
            .map_err(|_| SyncError::Internal("namespace mutex poisoned".to_string()))? = namespace;
        Ok(())
    }

    fn active_remote_path(&self, logical_path: &str) -> String {
        self.active_remote_namespace
            .lock()
            .ok()
            .and_then(|namespace| namespace.clone())
            .map(|namespace| format!("{namespace}/{logical_path}"))
            .unwrap_or_else(|| logical_path.to_string())
    }

    fn strip_active_remote_path(&self, path: &str) -> Option<String> {
        let namespace = self
            .active_remote_namespace
            .lock()
            .ok()
            .and_then(|namespace| namespace.clone());
        match namespace {
            Some(namespace) => path
                .strip_prefix(&format!("{namespace}/"))
                .map(str::to_string),
            None => Some(path.to_string()),
        }
    }

    fn remote_book_path(&self, book_hash: &str) -> String {
        // remote_extension returns "" for plaintext, ".enc" for age, ".env" for envelope.
        let ext = self.crypto().remote_extension("");
        if ext.is_empty() {
            self.active_remote_path(&format!("books/{book_hash}"))
        } else {
            self.active_remote_path(&format!("books/{book_hash}{ext}"))
        }
    }

    fn remote_progress_path(&self, book_hash: &str) -> String {
        // For plaintext: "book_progress/<hash>.json"
        // For age:      "book_progress/<hash>.json.enc"
        // For envelope: "book_progress/<hash>.json.env"
        let ext = self.crypto().remote_extension("json");
        if ext == "json" {
            self.active_remote_path(&format!("book_progress/{book_hash}.json"))
        } else if let Some(stripped) = ext.strip_prefix("json.") {
            self.active_remote_path(&format!("book_progress/{book_hash}.json.{stripped}"))
        } else {
            self.active_remote_path(&format!("book_progress/{book_hash}.json{ext}"))
        }
    }

    fn remote_bookmarks_path(&self, book_hash: &str) -> String {
        let ext = self.crypto().remote_extension("json");
        if ext == "json" {
            self.active_remote_path(&format!("bookmarks/{book_hash}.json"))
        } else if let Some(stripped) = ext.strip_prefix("json.") {
            self.active_remote_path(&format!("bookmarks/{book_hash}.json.{stripped}"))
        } else {
            self.active_remote_path(&format!("bookmarks/{book_hash}.json{ext}"))
        }
    }

    fn remote_tombstones_path(&self, book_hash: &str) -> String {
        let ext = self.crypto().remote_extension("json");
        if ext == "json" {
            self.active_remote_path(&format!("tombstones/{book_hash}.json"))
        } else if let Some(stripped) = ext.strip_prefix("json.") {
            self.active_remote_path(&format!("tombstones/{book_hash}.json.{stripped}"))
        } else {
            self.active_remote_path(&format!("tombstones/{book_hash}.json{ext}"))
        }
    }

    async fn write_remote_progress(&self, meta: &BookReadingMeta) -> Result<()> {
        let payload = RemoteProgress {
            schema_version: REMOTE_PROTOCOL_VERSION,
            book_hash: meta.book_hash.clone(),
            progress: meta.progress.clone(),
            last_writer_device_id: meta.device_id.clone(),
            last_write_ts: meta.wall_clock_ts,
        };
        let bytes = serde_json::to_vec(&payload)?;
        let encrypted = self.crypto().encrypt(&bytes)?;
        self.storage
            .write_object(&self.remote_progress_path(&meta.book_hash), &encrypted)
            .await
    }

    async fn read_remote_progress(
        &self,
        book_hash: &str,
    ) -> Result<VersionedRemote<RemoteProgress>> {
        let path = self.remote_progress_path(book_hash);
        let Some(object) = self.storage.read_object_versioned(&path).await? else {
            return Ok(VersionedRemote {
                value: None,
                version: None,
            });
        };
        let bytes = self.crypto().decrypt(&object.data)?;
        let value: RemoteProgress = serde_json::from_slice(&bytes)?;
        validate_remote_object(value.schema_version, &value.book_hash, book_hash)?;
        Ok(VersionedRemote {
            value: Some(value),
            version: Some(object.version),
        })
    }

    async fn write_remote_bookmarks(&self, meta: &BookReadingMeta) -> Result<()> {
        let payload = RemoteBookmarks {
            schema_version: REMOTE_PROTOCOL_VERSION,
            book_hash: meta.book_hash.clone(),
            bookmarks: meta.bookmarks.clone(),
            highlights: meta.highlights.clone(),
            notes: meta.notes.clone(),
            last_writer_device_id: meta.device_id.clone(),
            last_write_ts: meta.wall_clock_ts,
        };
        let bytes = serde_json::to_vec(&payload)?;
        let encrypted = self.crypto().encrypt(&bytes)?;
        self.storage
            .write_object(&self.remote_bookmarks_path(&meta.book_hash), &encrypted)
            .await
    }

    async fn read_remote_bookmarks(
        &self,
        book_hash: &str,
    ) -> Result<VersionedRemote<RemoteBookmarks>> {
        let path = self.remote_bookmarks_path(book_hash);
        let Some(object) = self.storage.read_object_versioned(&path).await? else {
            return Ok(VersionedRemote {
                value: None,
                version: None,
            });
        };
        let bytes = self.crypto().decrypt(&object.data)?;
        let value: RemoteBookmarks = serde_json::from_slice(&bytes)?;
        validate_remote_object(value.schema_version, &value.book_hash, book_hash)?;
        Ok(VersionedRemote {
            value: Some(value),
            version: Some(object.version),
        })
    }

    async fn read_remote_tombstones(
        &self,
        book_hash: &str,
    ) -> Result<VersionedRemote<TombstoneSet>> {
        let path = self.remote_tombstones_path(book_hash);
        let Some(object) = self.storage.read_object_versioned(&path).await? else {
            return Ok(VersionedRemote {
                value: None,
                version: None,
            });
        };
        let bytes = self.crypto().decrypt(&object.data)?;
        Ok(VersionedRemote {
            value: Some(TombstoneSet::decode(&bytes)?),
            version: Some(object.version),
        })
    }

    async fn write_remote_progress_conditional(
        &self,
        meta: &BookReadingMeta,
        expected: Option<&RemoteVersion>,
    ) -> Result<bool> {
        let payload = RemoteProgress {
            schema_version: REMOTE_PROTOCOL_VERSION,
            book_hash: meta.book_hash.clone(),
            progress: meta.progress.clone(),
            last_writer_device_id: meta.device_id.clone(),
            last_write_ts: meta.wall_clock_ts,
        };
        let encrypted = self.crypto().encrypt(&serde_json::to_vec(&payload)?)?;
        self.storage
            .write_object_conditional(
                &self.remote_progress_path(&meta.book_hash),
                &encrypted,
                expected,
            )
            .await
    }

    async fn write_remote_bookmarks_conditional(
        &self,
        meta: &BookReadingMeta,
        expected: Option<&RemoteVersion>,
    ) -> Result<bool> {
        let payload = RemoteBookmarks {
            schema_version: REMOTE_PROTOCOL_VERSION,
            book_hash: meta.book_hash.clone(),
            bookmarks: meta.bookmarks.clone(),
            highlights: meta.highlights.clone(),
            notes: meta.notes.clone(),
            last_writer_device_id: meta.device_id.clone(),
            last_write_ts: meta.wall_clock_ts,
        };
        let encrypted = self.crypto().encrypt(&serde_json::to_vec(&payload)?)?;
        self.storage
            .write_object_conditional(
                &self.remote_bookmarks_path(&meta.book_hash),
                &encrypted,
                expected,
            )
            .await
    }

    async fn write_remote_tombstones_conditional(
        &self,
        book_hash: &str,
        set: &TombstoneSet,
        expected: Option<&RemoteVersion>,
    ) -> Result<bool> {
        let encrypted = self.crypto().encrypt(&set.encode()?)?;
        self.storage
            .write_object_conditional(
                &self.remote_tombstones_path(book_hash),
                &encrypted,
                expected,
            )
            .await
    }

    pub fn write_local_tombstones_for_test(&self, meta_id: &str, set: &TombstoneSet) -> Result<()> {
        self.write_local_tombstones(meta_id, set)
    }

    async fn sync_book_inner(&self, book_hash: &str, mode: SyncMode) -> Result<()> {
        self.refresh_active_remote_namespace().await?;
        let remote_path = self.remote_book_path(book_hash);
        let local_path = self.local_book_path(book_hash);
        let indexed_path = self.indexed_local_book_path(book_hash)?;
        let source_path = indexed_path.as_deref().unwrap_or(&local_path);

        if mode != SyncMode::PullOnly && source_path.exists() {
            let actual_hash = blake3_file_hex(source_path)?;
            if actual_hash != book_hash {
                return Err(SyncError::InvalidArg(format!(
                    "local book hash mismatch: expected {book_hash}, got {actual_hash}"
                )));
            }
            let local_size = std::fs::metadata(source_path)?.len();
            if let Some((last_remote_size, last_remote_etag, last_remote_mtime)) =
                self.indexed_blob_remote_info(book_hash)?
            {
                match self.storage.stat(&remote_path).await {
                    Ok(stat)
                        if remote_stat_matches_index(
                            &stat,
                            last_remote_size,
                            last_remote_etag.as_deref(),
                            last_remote_mtime,
                        ) =>
                    {
                        if source_path != local_path {
                            if let Some(parent) = local_path.parent() {
                                std::fs::create_dir_all(parent)?;
                            }
                            std::fs::copy(source_path, &local_path)?;
                        }
                        self.upsert_blob_index(
                            book_hash,
                            stat.size as i64,
                            stat.etag.as_deref(),
                            stat.mtime,
                            &local_path.to_string_lossy(),
                        )?;
                        return Ok(());
                    }
                    Ok(_) => {}
                    Err(err) if is_remote_not_found_error(&err) => {
                        self.upload_book_file(source_path, &remote_path).await?;
                        if source_path != local_path {
                            if let Some(parent) = local_path.parent() {
                                std::fs::create_dir_all(parent)?;
                            }
                            std::fs::copy(source_path, &local_path)?;
                        }
                        let stat = self.storage.stat(&remote_path).await?;
                        self.upsert_blob_index(
                            book_hash,
                            stat.size as i64,
                            stat.etag.as_deref(),
                            stat.mtime,
                            &local_path.to_string_lossy(),
                        )?;
                        return Ok(());
                    }
                    Err(err) => return Err(err),
                }
            }
            if self.storage.exists(&remote_path).await? {
                let (remote_hash, remote_size) = self
                    .download_book_file(&remote_path, None, book_hash)
                    .await?;
                if remote_hash != book_hash {
                    self.record_blob_conflict(
                        book_hash,
                        Some(source_path),
                        local_size,
                        &remote_hash,
                        remote_size,
                    )?;
                    return Ok(());
                }
            } else {
                self.upload_book_file(source_path, &remote_path).await?;
            }
            if source_path != local_path {
                if let Some(parent) = local_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(source_path, &local_path)?;
            }
            let stat = self.storage.stat(&remote_path).await?;
            self.upsert_blob_index(
                book_hash,
                stat.size as i64,
                stat.etag.as_deref(),
                stat.mtime,
                &local_path.to_string_lossy(),
            )?;
            return Ok(());
        }

        if mode != SyncMode::PushOnly && self.storage.exists(&remote_path).await? {
            let (actual_hash, size) = self
                .download_book_file(&remote_path, Some(&local_path), book_hash)
                .await?;
            if actual_hash != book_hash {
                self.record_blob_conflict(book_hash, None, 0, &actual_hash, size)?;
                return Ok(());
            }
            let stat = self.storage.stat(&remote_path).await?;
            self.upsert_blob_index(
                book_hash,
                stat.size as i64,
                stat.etag.as_deref(),
                stat.mtime,
                &local_path.to_string_lossy(),
            )?;
            return Ok(());
        }

        Err(SyncError::Storage(format!(
            "book {book_hash} is not present locally or remotely"
        )))
    }

    async fn upload_book_file(&self, source: &Path, remote_path: &str) -> Result<()> {
        let encrypted_path = self.transfer_path("upload");
        if let Some(parent) = encrypted_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        self.crypto().encrypt_file(source, &encrypted_path)?;
        let size = std::fs::metadata(&encrypted_path)?.len();
        let file = tokio::fs::File::open(&encrypted_path).await?;
        let result = self
            .storage
            .upload_large(remote_path, Box::new(file), size)
            .await;
        let _ = tokio::fs::remove_file(encrypted_path).await;
        result
    }

    async fn download_book_file(
        &self,
        remote_path: &str,
        destination: Option<&Path>,
        expected_hash: &str,
    ) -> Result<(String, u64)> {
        let encrypted_path = self.transfer_path("download-encrypted");
        let plaintext_path = self.transfer_path("download-plain");
        if let Some(parent) = encrypted_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut remote = self.storage.download_large(remote_path).await?;
        let mut encrypted = tokio::fs::File::create(&encrypted_path).await?;
        tokio::io::copy(&mut remote, &mut encrypted).await?;
        drop(encrypted);
        let decrypt_result = self.crypto().decrypt_file(&encrypted_path, &plaintext_path);
        let _ = std::fs::remove_file(&encrypted_path);
        decrypt_result?;
        let size = std::fs::metadata(&plaintext_path)?.len();
        let hash = blake3_file_hex(&plaintext_path)?;
        if let Some(destination) = destination.filter(|_| hash == expected_hash) {
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(&plaintext_path, destination)?;
        } else {
            let _ = std::fs::remove_file(&plaintext_path);
        }
        Ok((hash, size))
    }

    fn transfer_path(&self, label: &str) -> PathBuf {
        static NEXT_TRANSFER_ID: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(1);
        let id = NEXT_TRANSFER_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.config
            .local_cache_dir
            .join("transfers")
            .join(format!("{label}-{}-{id}.tmp", std::process::id()))
    }

    async fn discover_meta_pairs(&self, mode: SyncMode) -> Result<Vec<(String, String)>> {
        // Reeden-style flat layout uses a single per-book meta keyed by
        // book_hash, but legacy callers sometimes still write meta files under
        // unrelated meta_ids. We union remote book hashes (derived from
        // book_progress/bookmarks/books) with any locally-stored meta files
        // whose proto carries a real book_hash, so single_meta can still find
        // them by the caller-supplied id.
        let mut pairs: std::collections::BTreeSet<(String, String)> =
            std::collections::BTreeSet::new();

        let local_dir = self.config.local_cache_dir.join("metas");
        if local_dir.is_dir() {
            for entry in std::fs::read_dir(local_dir)? {
                let path = entry?.path();
                let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
                let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if ext == "meta"
                    && let Some(meta_id) = file_name.strip_suffix(".meta")
                    && let Some(book_hash) = self.read_local_meta(meta_id)?.map(|m| m.book_hash)
                {
                    pairs.insert((book_hash, meta_id.to_string()));
                }
            }
        }
        if mode != SyncMode::PushOnly {
            let mut seen = std::collections::BTreeSet::new();
            for prefix in ["book_progress", "bookmarks"] {
                for info in self
                    .storage
                    .list_prefix(&self.active_remote_path(prefix))
                    .await?
                {
                    let Some(logical_path) = self.strip_active_remote_path(&info.path) else {
                        continue;
                    };
                    let rest = match logical_path.strip_prefix(&format!("{prefix}/")) {
                        Some(rest) => rest,
                        None => continue,
                    };
                    let book_hash = match rest.split_once('.') {
                        Some((hash, _)) => hash,
                        None => continue,
                    };
                    seen.insert(book_hash.to_string());
                }
            }
            for prefix in ["books"] {
                for info in self
                    .storage
                    .list_prefix(&self.active_remote_path(prefix))
                    .await?
                {
                    let Some(logical_path) = self.strip_active_remote_path(&info.path) else {
                        continue;
                    };
                    let rest = match logical_path.strip_prefix(&format!("{prefix}/")) {
                        Some(rest) => rest,
                        None => continue,
                    };
                    let book_hash = match rest.split_once('.') {
                        Some((hash, _)) => hash,
                        None => rest,
                    };
                    seen.insert(book_hash.to_string());
                }
            }
            for book_hash in seen {
                pairs.insert((book_hash.clone(), book_hash));
            }
        }
        Ok(pairs.into_iter().collect())
    }

    async fn discover_book_hashes(&self, mode: SyncMode) -> Result<Vec<String>> {
        let mut hashes = std::collections::BTreeSet::new();
        {
            let db = self
                .db
                .lock()
                .map_err(|_| SyncError::Internal("database mutex poisoned".to_string()))?;
            let mut stmt = db.prepare("SELECT book_hash FROM blob_index")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                hashes.insert(row.get::<_, String>(0)?);
            }
        }
        if mode != SyncMode::PushOnly {
            for info in self
                .storage
                .list_prefix(&self.active_remote_path("books"))
                .await?
            {
                let Some(logical_path) = self.strip_active_remote_path(&info.path) else {
                    continue;
                };
                let full = &logical_path;
                let Some(rest) = full.strip_prefix("books/") else {
                    continue;
                };
                let Some((book_hash, _)) = rest.split_once('.') else {
                    hashes.insert(rest.to_string());
                    continue;
                };
                hashes.insert(book_hash.to_string());
            }
        }
        Ok(hashes.into_iter().collect())
    }

    fn upsert_blob_index(
        &self,
        book_hash: &str,
        last_remote_size: i64,
        last_remote_etag: Option<&str>,
        last_sync_mtime: i64,
        local_file_path: &str,
    ) -> Result<()> {
        let db = self
            .db
            .lock()
            .map_err(|_| SyncError::Internal("database mutex poisoned".to_string()))?;
        db.execute(
            "INSERT INTO blob_index(book_hash, last_remote_size, last_remote_etag, last_sync_mtime, local_file_path)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(book_hash) DO UPDATE SET
               last_remote_size = excluded.last_remote_size,
               last_remote_etag = excluded.last_remote_etag,
               last_sync_mtime = excluded.last_sync_mtime,
               local_file_path = excluded.local_file_path",
            rusqlite::params![
                book_hash,
                last_remote_size,
                last_remote_etag,
                last_sync_mtime,
                local_file_path
            ],
        )?;
        Ok(())
    }

    fn record_blob_conflict(
        &self,
        book_hash: &str,
        local_path: Option<&Path>,
        local_size: u64,
        remote_hash: &str,
        remote_size: u64,
    ) -> Result<()> {
        let local_json = serde_json::json!({
            "book_hash": book_hash,
            "hash": book_hash,
            "size": local_size,
            "path": local_path.map(|path| path.to_string_lossy().to_string())
        });
        let remote_json = serde_json::json!({
            "book_hash": book_hash,
            "hash": remote_hash,
            "size": remote_size
        });
        let description = format!(
            "blob conflict: reason=blob_hash_mismatch, expected_hash={book_hash}, remote_hash={remote_hash}"
        );
        let db = self
            .db
            .lock()
            .map_err(|_| SyncError::Internal("database mutex poisoned".to_string()))?;
        db.execute(
            "DELETE FROM conflict_log
             WHERE object_type = ?1 AND object_id = ?2 AND resolved_ts IS NULL",
            rusqlite::params!["blob", book_hash],
        )?;
        db.execute(
            "INSERT INTO conflict_log(timestamp, object_type, object_id, description, local_json, remote_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                now_millis(),
                "blob",
                book_hash,
                description,
                serde_json::to_string(&local_json)?,
                serde_json::to_string(&remote_json)?
            ],
        )?;
        drop(db);

        self.events.emit(
            EventType::ConflictFound,
            &serde_json::json!({
                "object_type":"blob",
                "book_hash":book_hash,
                "remote_hash":remote_hash,
                "reason":"blob_hash_mismatch"
            }),
        );
        Ok(())
    }

    pub fn conflict_count(&self) -> Result<i64> {
        let db = self
            .db
            .lock()
            .map_err(|_| SyncError::Internal("database mutex poisoned".to_string()))?;
        Ok(db.query_row(
            "SELECT COUNT(*) FROM conflict_log WHERE resolved_ts IS NULL",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn get_sync_state_json(&self) -> Result<String> {
        Ok(serde_json::to_string(&serde_json::json!({
            "device_id": self.device_id(),
            "encrypted": self.crypto_is_encrypted(),
            "conflict_count": self.conflict_count()?,
            "tombstone_count": self.tombstone_count()?,
            "conflicts": self.pending_conflicts()?
        }))?)
    }

    fn tombstone_count(&self) -> Result<usize> {
        let local_dir = self.config.local_cache_dir.join("metas");
        if !local_dir.is_dir() {
            return Ok(0);
        }
        let mut count = 0;
        for entry in std::fs::read_dir(local_dir)? {
            let path = entry?.path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".tombstones.json"))
            {
                count += TombstoneSet::decode(&std::fs::read(path)?)?.len();
            }
        }
        Ok(count)
    }

    fn pending_conflicts(&self) -> Result<Vec<serde_json::Value>> {
        let db = self
            .db
            .lock()
            .map_err(|_| SyncError::Internal("database mutex poisoned".to_string()))?;
        let mut stmt = db.prepare(
            "SELECT id, timestamp, object_type, object_id, description, local_json, remote_json
             FROM conflict_log
             WHERE resolved_ts IS NULL
             ORDER BY timestamp DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let local_json: Option<String> = row.get(5)?;
            let remote_json: Option<String> = row.get(6)?;
            let object_type: String = row.get(2)?;
            let object_id: String = row.get(3)?;
            Ok(conflict_state_json(
                row.get(0)?,
                row.get(1)?,
                &object_type,
                &object_id,
                row.get(4)?,
                local_json.as_deref(),
                remote_json.as_deref(),
            ))
        })?;

        let mut conflicts = Vec::new();
        for row in rows {
            conflicts.push(row?);
        }
        Ok(conflicts)
    }

    fn pending_meta_conflict_payload(&self, meta_id: &str) -> Result<(i64, String, String)> {
        let db = self
            .db
            .lock()
            .map_err(|_| SyncError::Internal("database mutex poisoned".to_string()))?;
        let mut stmt = db.prepare(
            "SELECT id, local_json, remote_json
             FROM conflict_log
             WHERE object_type = ?1 AND object_id = ?2 AND resolved_ts IS NULL
             ORDER BY timestamp DESC, id DESC
             LIMIT 1",
        )?;
        let mut rows = stmt.query(rusqlite::params!["meta", meta_id])?;
        if let Some(row) = rows.next()? {
            let id = row.get(0)?;
            let local_json: Option<String> = row.get(1)?;
            let remote_json: Option<String> = row.get(2)?;
            return Ok((
                id,
                local_json.ok_or_else(|| {
                    SyncError::Conflict("pending meta conflict has no local snapshot".to_string())
                })?,
                remote_json.ok_or_else(|| {
                    SyncError::Conflict("pending meta conflict has no remote snapshot".to_string())
                })?,
            ));
        }
        Err(SyncError::Conflict(format!(
            "no pending meta conflict for {meta_id}"
        )))
    }

    fn pending_tombstone_conflict_payload(
        &self,
        meta_id: &str,
        item_uuid: &str,
    ) -> Result<(i64, String, String)> {
        let db = self
            .db
            .lock()
            .map_err(|_| SyncError::Internal("database mutex poisoned".to_string()))?;
        let object_id = format!("{meta_id}:{item_uuid}");
        let mut stmt = db.prepare(
            "SELECT id, local_json, remote_json
             FROM conflict_log
             WHERE object_type = ?1 AND object_id = ?2 AND resolved_ts IS NULL
             ORDER BY timestamp DESC, id DESC
             LIMIT 1",
        )?;
        let mut rows = stmt.query(rusqlite::params!["tombstone", object_id])?;
        if let Some(row) = rows.next()? {
            let id = row.get(0)?;
            let local_json: Option<String> = row.get(1)?;
            let remote_json: Option<String> = row.get(2)?;
            return Ok((
                id,
                local_json.ok_or_else(|| {
                    SyncError::Conflict(
                        "pending tombstone conflict has no tombstone snapshot".to_string(),
                    )
                })?,
                remote_json.ok_or_else(|| {
                    SyncError::Conflict(
                        "pending tombstone conflict has no meta snapshot".to_string(),
                    )
                })?,
            ));
        }
        Err(SyncError::Conflict(format!(
            "no pending tombstone revival conflict for {meta_id}:{item_uuid}"
        )))
    }

    fn pending_blob_conflict_id(&self, book_hash: &str) -> Result<i64> {
        let db = self
            .db
            .lock()
            .map_err(|_| SyncError::Internal("database mutex poisoned".to_string()))?;
        let mut stmt = db.prepare(
            "SELECT id
             FROM conflict_log
             WHERE object_type = ?1 AND object_id = ?2 AND resolved_ts IS NULL
             ORDER BY timestamp DESC, id DESC
             LIMIT 1",
        )?;
        let mut rows = stmt.query(rusqlite::params!["blob", book_hash])?;
        if let Some(row) = rows.next()? {
            return Ok(row.get(0)?);
        }
        Err(SyncError::Conflict(format!(
            "no pending blob conflict for {book_hash}"
        )))
    }

    fn mark_conflict_resolved(&self, conflict_id: i64) -> Result<()> {
        let db = self
            .db
            .lock()
            .map_err(|_| SyncError::Internal("database mutex poisoned".to_string()))?;
        db.execute(
            "UPDATE conflict_log SET resolved_ts = ?1 WHERE id = ?2",
            rusqlite::params![now_millis(), conflict_id],
        )?;
        Ok(())
    }

    fn indexed_local_book_path(&self, book_hash: &str) -> Result<Option<PathBuf>> {
        let db = self
            .db
            .lock()
            .map_err(|_| SyncError::Internal("database mutex poisoned".to_string()))?;
        let mut stmt = db.prepare("SELECT local_file_path FROM blob_index WHERE book_hash = ?1")?;
        let mut rows = stmt.query(rusqlite::params![book_hash])?;
        if let Some(row) = rows.next()? {
            let path: String = row.get(0)?;
            Ok(Some(PathBuf::from(path)))
        } else {
            Ok(None)
        }
    }

    fn indexed_blob_remote_info(
        &self,
        book_hash: &str,
    ) -> Result<Option<(i64, Option<String>, i64)>> {
        let db = self
            .db
            .lock()
            .map_err(|_| SyncError::Internal("database mutex poisoned".to_string()))?;
        let mut stmt = db.prepare(
            "SELECT last_remote_size, last_remote_etag, last_sync_mtime
             FROM blob_index
             WHERE book_hash = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![book_hash])?;
        if let Some(row) = rows.next()? {
            Ok(Some((row.get(0)?, row.get(1)?, row.get(2)?)))
        } else {
            Ok(None)
        }
    }

    fn local_book_path(&self, book_hash: &str) -> PathBuf {
        self.config
            .local_cache_dir
            .join("blobs")
            .join(format!("{book_hash}.epub"))
    }

    fn blob_policy_decision(&self) -> Result<BlobPolicyDecision> {
        let scheduler = self
            .scheduler
            .lock()
            .map_err(|_| SyncError::Internal("scheduler mutex poisoned".to_string()))?;
        if scheduler.blob_paused {
            return Ok(BlobPolicyDecision {
                allowed: false,
                reason: "paused",
                byte_limit: scheduler.blob_byte_limit,
            });
        }
        match scheduler.network_type {
            NetworkType::Wifi => Ok(BlobPolicyDecision {
                allowed: true,
                reason: "wifi",
                byte_limit: scheduler.blob_byte_limit,
            }),
            NetworkType::Cellular => Ok(BlobPolicyDecision {
                allowed: false,
                reason: "cellular",
                byte_limit: scheduler.blob_byte_limit,
            }),
            NetworkType::Unknown => Ok(BlobPolicyDecision {
                allowed: false,
                reason: "unknown_network",
                byte_limit: scheduler.blob_byte_limit,
            }),
        }
    }

    async fn estimate_book_transfer_size(&self, book_hash: &str, mode: SyncMode) -> Result<u64> {
        let remote_path = self.remote_book_path(book_hash);
        let local_path = self.local_book_path(book_hash);
        let indexed_path = self.indexed_local_book_path(book_hash)?;
        let source_path = indexed_path.as_deref().unwrap_or(&local_path);

        if mode != SyncMode::PullOnly && source_path.exists() {
            return Ok(std::fs::metadata(source_path)?.len());
        }
        if mode != SyncMode::PushOnly && self.storage.exists(&remote_path).await? {
            return Ok(self.storage.stat(&remote_path).await?.size);
        }
        Ok(0)
    }

    pub fn last_error_safe_message(err: &SyncError) -> String {
        err.to_string()
    }

    pub fn version() -> i32 {
        1
    }

    pub fn device_id(&self) -> &str {
        &self.config.device_id
    }

    pub fn crypto_is_encrypted(&self) -> bool {
        self.crypto().is_encrypted()
    }
}

#[derive(Debug, Clone)]
struct BlobPolicyDecision {
    allowed: bool,
    reason: &'static str,
    byte_limit: Option<u64>,
}

fn blake3_file_hex(path: &Path) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    let mut file = std::fs::File::open(path)?;
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hasher.finalize().to_hex().to_string())
}

fn merge_meta_with_remote(
    local: Option<BookReadingMeta>,
    remote_progress: Option<&RemoteProgress>,
    remote_bookmarks: Option<&RemoteBookmarks>,
) -> BookReadingMeta {
    let mut base = local.unwrap_or_else(|| BookReadingMeta {
        meta_id: String::new(),
        book_hash: String::new(),
        modified_ts: 0,
        device_id: String::new(),
        progress: None,
        bookmarks: Vec::new(),
        highlights: Vec::new(),
        notes: Vec::new(),
        wall_clock_ts: 0,
        logical_ts: 0,
        origin_device_id: String::new(),
        edit_history: Vec::new(),
    });

    // Capture the local writer identity before either remote block mutates the
    // base meta. The bookmarks merge decision must compare the new remote
    // bookmarks against the *original* local ts/device — not the post-progress
    // copy.
    let local_wall = base.wall_clock_ts;
    let local_dev = base.device_id.clone();

    if let Some(rp) = remote_progress
        && (rp.last_write_ts > local_wall
            || (rp.last_write_ts == local_wall && rp.last_writer_device_id > local_dev))
    {
        base.progress = rp.progress.clone();
        base.wall_clock_ts = rp.last_write_ts;
        base.device_id = rp.last_writer_device_id.clone();
    }
    if let Some(rb) = remote_bookmarks {
        let rb_is_newer = rb.last_write_ts > local_wall
            || (rb.last_write_ts == local_wall && rb.last_writer_device_id > local_dev);
        let rb_same_writer =
            rb.last_write_ts == local_wall && rb.last_writer_device_id == local_dev;
        if rb_is_newer {
            // The remote writer is strictly newer: their snapshot is the
            // canonical truth, so an empty remote list (e.g. produced by a
            // tombstone mark-delete) correctly overrides the local cache.
            // Local-only items the remote hadn't seen yet are still pulled in
            // so we never lose work that the remote hadn't observed.
            let local_bookmarks: Vec<Bookmark> = base.bookmarks.clone();
            let local_highlights: Vec<Highlight> = base.highlights.clone();
            let local_notes: Vec<BookNote> = base.notes.clone();
            base.bookmarks = rb.bookmarks.clone();
            base.highlights = rb.highlights.clone();
            base.notes = rb.notes.clone();
            append_missing_bookmarks(&mut base.bookmarks, &local_bookmarks);
            append_missing_highlights(&mut base.highlights, &local_highlights);
            append_missing_notes(&mut base.notes, &local_notes);
            base.wall_clock_ts = rb.last_write_ts;
            base.device_id = rb.last_writer_device_id.clone();
        } else if rb_same_writer {
            merge_bookmark_list(&mut base.bookmarks, &rb.bookmarks);
            merge_highlight_list(&mut base.highlights, &rb.highlights);
            merge_note_list(&mut base.notes, &rb.notes);
        }
        // else: local is strictly newer; do not pull stale remote entries in.
    }
    base
}

fn merge_bookmark_list(target: &mut Vec<Bookmark>, incoming: &[Bookmark]) {
    for item in incoming {
        if let Some(existing) = target
            .iter_mut()
            .find(|e| e.bookmark_id == item.bookmark_id)
        {
            if item.create_ts >= existing.create_ts {
                *existing = item.clone();
            }
        } else {
            target.push(item.clone());
        }
    }
}

fn append_missing_bookmarks(target: &mut Vec<Bookmark>, incoming: &[Bookmark]) {
    for item in incoming {
        if !target
            .iter()
            .any(|existing| existing.bookmark_id == item.bookmark_id)
        {
            target.push(item.clone());
        }
    }
}

fn merge_highlight_list(target: &mut Vec<Highlight>, incoming: &[Highlight]) {
    for item in incoming {
        if let Some(existing) = target
            .iter_mut()
            .find(|e| e.highlight_id == item.highlight_id)
        {
            if item.create_ts >= existing.create_ts {
                *existing = item.clone();
            }
        } else {
            target.push(item.clone());
        }
    }
}

fn append_missing_highlights(target: &mut Vec<Highlight>, incoming: &[Highlight]) {
    for item in incoming {
        if !target
            .iter()
            .any(|existing| existing.highlight_id == item.highlight_id)
        {
            target.push(item.clone());
        }
    }
}

fn merge_note_list(target: &mut Vec<BookNote>, incoming: &[BookNote]) {
    for item in incoming {
        if let Some(existing) = target.iter_mut().find(|e| e.note_id == item.note_id) {
            if item.create_ts >= existing.create_ts {
                *existing = item.clone();
            }
        } else {
            target.push(item.clone());
        }
    }
}

fn append_missing_notes(target: &mut Vec<BookNote>, incoming: &[BookNote]) {
    for item in incoming {
        if !target
            .iter()
            .any(|existing| existing.note_id == item.note_id)
        {
            target.push(item.clone());
        }
    }
}

fn progress_matches_remote(meta: &BookReadingMeta, remote: Option<&RemoteProgress>) -> bool {
    match remote {
        Some(rp) => rp.progress == meta.progress && rp.last_write_ts == meta.wall_clock_ts,
        None => meta.progress.is_none(),
    }
}

fn bookmarks_matches_remote(meta: &BookReadingMeta, remote: Option<&RemoteBookmarks>) -> bool {
    match remote {
        Some(rb) => {
            rb.bookmarks == meta.bookmarks
                && rb.highlights == meta.highlights
                && rb.notes == meta.notes
                && rb.last_write_ts == meta.wall_clock_ts
        }
        None => meta.bookmarks.is_empty() && meta.highlights.is_empty() && meta.notes.is_empty(),
    }
}

fn remote_progress_matches(meta: &BookReadingMeta, remote: Option<&RemoteProgress>) -> bool {
    progress_matches_remote(meta, remote)
}

fn remote_bookmarks_matches(meta: &BookReadingMeta, remote: Option<&RemoteBookmarks>) -> bool {
    bookmarks_matches_remote(meta, remote)
}

#[cfg(test)]
fn parse_remote_bookmarks_lists(bytes: &[u8]) -> (Vec<Bookmark>, Vec<Highlight>, Vec<BookNote>) {
    let parsed: RemoteBookmarks = serde_json::from_slice(bytes)
        .expect("parse_remote_bookmarks_lists expects valid bookmarks JSON");
    (parsed.bookmarks, parsed.highlights, parsed.notes)
}

fn encode_edit_history_archive(edit_history: &[MetaEdit]) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(edit_history)?;
    Ok(zstd::stream::encode_all(Cursor::new(json), 3)?)
}

fn build_crypto(encryption: &EncryptionConfigJson) -> Result<Arc<dyn CryptoProvider>> {
    let encryption_type = encryption
        .encryption_type
        .as_deref()
        .unwrap_or("none")
        .to_ascii_lowercase();
    match encryption_type.as_str() {
        "none" | "noop" => Ok(Arc::new(NoopCrypto)),
        "age" => {
            let identity = encryption.identity.as_deref().ok_or_else(|| {
                SyncError::InvalidArg("age encryption requires identity".to_string())
            })?;
            Ok(Arc::new(AgeCrypto::from_identity_string(identity)?))
        }
        "envelope" | "aes-gcm-envelope" => Ok(Arc::new(build_envelope_crypto(encryption)?)),
        other => Err(SyncError::Crypto(format!(
            "encryption type '{other}' is implemented in a later milestone"
        ))),
    }
}

fn encryption_type_is_envelope(encryption: &EncryptionConfigJson) -> bool {
    matches!(
        encryption
            .encryption_type
            .as_deref()
            .unwrap_or("none")
            .to_ascii_lowercase()
            .as_str(),
        "envelope" | "aes-gcm-envelope"
    )
}

fn build_envelope_crypto(encryption: &EncryptionConfigJson) -> Result<EnvelopeCrypto> {
    if let Some(kek_hex) = encryption.kek_hex.as_deref() {
        return EnvelopeCrypto::from_kek_hex(
            kek_hex,
            encryption.kek_id.clone(),
            encryption.kek_version,
        );
    }
    let passphrase = encryption.passphrase.as_deref().ok_or_else(|| {
        SyncError::InvalidArg("envelope encryption requires passphrase or kek_hex".to_string())
    })?;
    EnvelopeCrypto::from_passphrase(
        passphrase,
        encryption.kek_id.clone(),
        encryption.kek_version,
        envelope_kdf_params(encryption),
    )
}

fn register_envelope_kek_version(
    conn: &Connection,
    encryption: &EncryptionConfigJson,
) -> Result<()> {
    if !encryption_type_is_envelope(encryption) {
        return Ok(());
    }
    let version = encryption.kek_version.unwrap_or(1) as i64;
    let kek_id = encryption
        .kek_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "default".to_string());
    let kdf_params = if encryption.kek_hex.is_some() {
        serde_json::json!({"mode":"raw_kek"})
    } else {
        serde_json::json!({
            "mode":"argon2id",
            "memory_cost":encryption.argon2_memory_cost.unwrap_or(19 * 1024),
            "time_cost":encryption.argon2_time_cost.unwrap_or(2),
            "parallelism":encryption.argon2_parallelism.unwrap_or(1)
        })
    };
    conn.execute(
        "INSERT INTO kek_versions(version, kek_id, created_ts, retired_ts, kdf_params)
         VALUES (?1, ?2, ?3, NULL, ?4)
         ON CONFLICT(version) DO UPDATE SET
           kek_id = excluded.kek_id,
           kdf_params = excluded.kdf_params",
        rusqlite::params![version, kek_id, now_millis(), kdf_params.to_string()],
    )?;
    Ok(())
}

fn envelope_kdf_params(encryption: &EncryptionConfigJson) -> Option<EnvelopeKdfParams> {
    if encryption.argon2_memory_cost.is_none()
        && encryption.argon2_time_cost.is_none()
        && encryption.argon2_parallelism.is_none()
    {
        return None;
    }
    Some(EnvelopeKdfParams {
        memory_cost: encryption.argon2_memory_cost.unwrap_or(19 * 1024),
        time_cost: encryption.argon2_time_cost.unwrap_or(2),
        parallelism: encryption.argon2_parallelism.unwrap_or(1),
    })
}

fn build_storage(
    storage_config: &StorageConfigJson,
    local_cache_dir: &Path,
) -> Result<Arc<dyn RemoteStorage>> {
    let storage_type = storage_config
        .storage_type
        .as_deref()
        .unwrap_or("file")
        .to_ascii_lowercase();
    match storage_type.as_str() {
        "file" => {
            let root = storage_config.root_dir.clone().ok_or_else(|| {
                SyncError::InvalidArg("file storage requires root_dir".to_string())
            })?;
            Ok(Arc::new(FileStorage::new(root)))
        }
        "memory" => Ok(Arc::new(FileStorage::new(
            local_cache_dir.join("remote_memory"),
        ))),
        "s3" => {
            let config = S3Config {
                endpoint: required_string("s3", storage_config.endpoint.clone(), "endpoint")?,
                bucket: required_string("s3", storage_config.bucket.clone(), "bucket")?,
                access_key: required_string("s3", storage_config.access_key.clone(), "access_key")?,
                secret_key: required_string("s3", storage_config.secret_key.clone(), "secret_key")?,
                region: storage_config
                    .region
                    .clone()
                    .unwrap_or_else(|| "us-east-1".to_string()),
                root_prefix: storage_config
                    .root_prefix
                    .clone()
                    .unwrap_or_else(|| "kmo-sync".to_string()),
                path_style: storage_config.path_style.unwrap_or(true),
                allow_http: storage_config.allow_http.unwrap_or(false),
            };
            Ok(Arc::new(S3Storage::new(config)?))
        }
        "webdav" => {
            let config = WebDavConfig {
                url: required_string("webdav", storage_config.url.clone(), "url")?,
                username: storage_config.username.clone(),
                password: storage_config.password.clone(),
                root_dir: storage_config
                    .root_dir
                    .as_ref()
                    .map(|path| path.to_string_lossy().to_string())
                    .unwrap_or_else(|| "kmo-sync".to_string()),
            };
            Ok(Arc::new(WebDavStorage::new(config)?))
        }
        other => Err(SyncError::Storage(format!(
            "storage type '{other}' is implemented in a later milestone"
        ))),
    }
}

fn required_string(storage_type: &str, value: Option<String>, name: &str) -> Result<String> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| SyncError::InvalidArg(format!("{storage_type} storage requires {name}")))
}

fn detect_clock_drift(db: &Connection, events: &EventEmitter) -> Result<()> {
    const CLOCK_DRIFT_WARNING_MILLIS: i64 = 60 * 60 * 1000;

    let now = now_millis();
    let previous = db
        .query_row(
            "SELECT value FROM sync_state WHERE key = ?1",
            rusqlite::params!["last_wall_clock_ts"],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|value| value.parse::<i64>().ok());

    if let Some(previous) = previous
        && previous.saturating_sub(now) > CLOCK_DRIFT_WARNING_MILLIS
    {
        events.emit(
            EventType::ClockDriftWarning,
            &serde_json::json!({
                "previous_wall_clock_ts": previous,
                "current_wall_clock_ts": now,
                "drift_millis": previous - now,
                "direction": "backward"
            }),
        );
    }

    db.execute(
        "INSERT INTO sync_state(key, value)
         VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params!["last_wall_clock_ts", now.to_string()],
    )?;
    Ok(())
}

fn remove_meta_item(meta: &mut BookReadingMeta, item_type: &TombstoneItemType, item_uuid: &str) {
    match item_type {
        TombstoneItemType::Bookmark => meta.bookmarks.retain(|item| item.bookmark_id != item_uuid),
        TombstoneItemType::Highlight => meta
            .highlights
            .retain(|item| item.highlight_id != item_uuid),
        TombstoneItemType::Note => meta.notes.retain(|item| item.note_id != item_uuid),
    }
}

fn snapshot_meta_item(
    meta: &BookReadingMeta,
    item_type: &TombstoneItemType,
    item_uuid: &str,
) -> Result<String> {
    match item_type {
        TombstoneItemType::Bookmark => meta
            .bookmarks
            .iter()
            .find(|item| item.bookmark_id == item_uuid)
            .map(serde_json::to_string),
        TombstoneItemType::Highlight => meta
            .highlights
            .iter()
            .find(|item| item.highlight_id == item_uuid)
            .map(serde_json::to_string),
        TombstoneItemType::Note => meta
            .notes
            .iter()
            .find(|item| item.note_id == item_uuid)
            .map(serde_json::to_string),
    }
    .ok_or_else(|| SyncError::InvalidArg(format!("metadata item not found: {item_uuid}")))?
    .map_err(Into::into)
}

fn restore_meta_item(meta: &mut BookReadingMeta, tombstone: &Tombstone) -> Result<()> {
    let snapshot = tombstone.deleted_item_json.as_deref().ok_or_else(|| {
        SyncError::Conflict(format!(
            "deletion for {} has no restorable snapshot",
            tombstone.uuid
        ))
    })?;
    match tombstone.item_type {
        TombstoneItemType::Bookmark => {
            let item: Bookmark = serde_json::from_str(snapshot)?;
            meta.bookmarks
                .retain(|value| value.bookmark_id != item.bookmark_id);
            meta.bookmarks.push(item);
        }
        TombstoneItemType::Highlight => {
            let item: Highlight = serde_json::from_str(snapshot)?;
            meta.highlights
                .retain(|value| value.highlight_id != item.highlight_id);
            meta.highlights.push(item);
        }
        TombstoneItemType::Note => {
            let item: BookNote = serde_json::from_str(snapshot)?;
            meta.notes.retain(|value| value.note_id != item.note_id);
            meta.notes.push(item);
        }
    }
    Ok(())
}

fn conflict_state_json(
    id: i64,
    timestamp: i64,
    object_type: &str,
    object_id: &str,
    description: Option<String>,
    local_json: Option<&str>,
    remote_json: Option<&str>,
) -> serde_json::Value {
    let mut value = serde_json::json!({
        "id": id,
        "timestamp": timestamp,
        "object_type": object_type,
        "object_id": object_id,
        "description": description
    });

    match object_type {
        "meta" => {
            value["meta_id"] = serde_json::Value::String(object_id.to_string());
            value["conflict_kind"] = serde_json::Value::String("meta_file".to_string());
            value["resolution_options"] = serde_json::json!(["local", "remote"]);
            value["local"] = meta_conflict_summary(local_json);
            value["remote"] = meta_conflict_summary(remote_json);
        }
        "tombstone" => {
            let (meta_id, item_uuid) = split_tombstone_object_id(object_id);
            value["meta_id"] = serde_json::Value::String(meta_id);
            value["item_uuid"] = serde_json::Value::String(item_uuid);
            value["conflict_kind"] = serde_json::Value::String("tombstone_revival".to_string());
            value["resolution_options"] = serde_json::json!(["delete", "restore"]);
            value["tombstone"] = tombstone_conflict_summary(local_json);
            value["incoming_meta"] = meta_conflict_summary(remote_json);
            if let Some(item_type) = tombstone_item_type(local_json) {
                value["item_type"] = serde_json::Value::String(item_type);
            }
        }
        "blob" => {
            value["book_hash"] = serde_json::Value::String(object_id.to_string());
            value["conflict_kind"] = serde_json::Value::String("blob_file".to_string());
            value["resolution_options"] = serde_json::json!(["local", "remote"]);
            value["local"] = blob_conflict_summary(local_json);
            value["remote"] = blob_conflict_summary(remote_json);
        }
        _ => {
            value["local"] = meta_conflict_summary(local_json);
            value["remote"] = meta_conflict_summary(remote_json);
        }
    }

    value
}

fn blob_conflict_summary(blob_json: Option<&str>) -> serde_json::Value {
    let Some(blob_json) = blob_json else {
        return serde_json::Value::Null;
    };
    match serde_json::from_str::<serde_json::Value>(blob_json) {
        Ok(value) => serde_json::json!({
            "book_hash": value.get("book_hash").cloned().unwrap_or(serde_json::Value::Null),
            "hash": value.get("hash").cloned().unwrap_or(serde_json::Value::Null),
            "size": value.get("size").cloned().unwrap_or(serde_json::Value::Null),
            "path": value.get("path").cloned().unwrap_or(serde_json::Value::Null)
        }),
        Err(_) => serde_json::Value::Null,
    }
}

fn split_tombstone_object_id(object_id: &str) -> (String, String) {
    object_id
        .split_once(':')
        .map(|(meta_id, item_uuid)| (meta_id.to_string(), item_uuid.to_string()))
        .unwrap_or_else(|| (object_id.to_string(), String::new()))
}

fn tombstone_conflict_summary(tombstone_json: Option<&str>) -> serde_json::Value {
    let Some(tombstone_json) = tombstone_json else {
        return serde_json::Value::Null;
    };
    match serde_json::from_str::<Tombstone>(tombstone_json) {
        Ok(tombstone) => serde_json::json!({
            "item_uuid": tombstone.uuid,
            "item_type": tombstone.item_type,
            "deleted_at_logical_ts": tombstone.deleted_at_logical_ts,
            "deleted_at_wall_ts": tombstone.deleted_at_wall_ts,
            "deleted_by_device": tombstone.deleted_by_device,
            "grace_period_days": tombstone.grace_period_days
        }),
        Err(_) => serde_json::Value::Null,
    }
}

fn tombstone_item_type(tombstone_json: Option<&str>) -> Option<String> {
    serde_json::from_str::<Tombstone>(tombstone_json?)
        .ok()
        .map(|tombstone| {
            match tombstone.item_type {
                TombstoneItemType::Bookmark => "bookmark",
                TombstoneItemType::Highlight => "highlight",
                TombstoneItemType::Note => "note",
            }
            .to_string()
        })
}

fn meta_conflict_summary(meta_json: Option<&str>) -> serde_json::Value {
    let Some(meta_json) = meta_json else {
        return serde_json::Value::Null;
    };
    match serde_json::from_str::<BookReadingMeta>(meta_json) {
        Ok(meta) => serde_json::json!({
            "device_id": meta.device_id,
            "logical_ts": meta.logical_ts,
            "modified_ts": meta.modified_ts,
            "progress_percent": meta.progress.map(|progress| progress.progress_percent)
        }),
        Err(_) => serde_json::Value::Null,
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn remote_stat_matches_index(
    stat: &crate::storage::RemoteFileInfo,
    last_remote_size: i64,
    last_remote_etag: Option<&str>,
    last_remote_mtime: i64,
) -> bool {
    if stat.size as i64 != last_remote_size {
        return false;
    }
    match last_remote_etag {
        Some(etag) => stat.etag.as_deref() == Some(etag),
        None => stat.mtime != 0 && stat.mtime == last_remote_mtime,
    }
}

fn is_remote_not_found_error(err: &SyncError) -> bool {
    match err {
        SyncError::Storage(message) => {
            let message = message.to_ascii_lowercase();
            message.contains("not found") || message.contains("404")
        }
        SyncError::Io(io) => io.kind() == std::io::ErrorKind::NotFound,
        _ => false,
    }
}

fn validate_json<T: for<'de> Deserialize<'de>>(json: &str, name: &str) -> Result<T> {
    if json.trim().is_empty() {
        return Err(SyncError::InvalidArg(format!("{name} is empty")));
    }
    Ok(serde_json::from_str(json)?)
}

fn validate_identifier(value: &str, name: &str) -> Result<()> {
    if value.is_empty() {
        return Err(SyncError::InvalidArg(format!("{name} is empty")));
    }
    if value.len() > 255
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        return Err(SyncError::InvalidArg(format!("unsafe {name}: {value:?}")));
    }
    Ok(())
}

fn validate_remote_object(
    schema_version: u32,
    actual_book_hash: &str,
    book_hash: &str,
) -> Result<()> {
    if schema_version != REMOTE_PROTOCOL_VERSION {
        return Err(SyncError::VersionMismatch(format!(
            "remote metadata schema {schema_version} is incompatible with {REMOTE_PROTOCOL_VERSION}"
        )));
    }
    if actual_book_hash != book_hash {
        return Err(SyncError::Conflict(format!(
            "remote metadata book hash mismatch: expected {book_hash}, got {actual_book_hash}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Highlight, ReadingProgress};
    use age::secrecy::ExposeSecret;
    use age::x25519::Identity;
    use std::sync::atomic::{AtomicBool, Ordering};

    static CLOCK_DRIFT_WARNING_SEEN: AtomicBool = AtomicBool::new(false);

    fn sample_meta(meta_id: &str, progress: f64, logical_ts: i64) -> BookReadingMeta {
        BookReadingMeta {
            meta_id: meta_id.to_string(),
            book_hash: "book-1".to_string(),
            modified_ts: logical_ts,
            device_id: "device-a".to_string(),
            progress: Some(ReadingProgress {
                cfi_position: format!("epubcfi(/6/{logical_ts})"),
                progress_percent: progress,
                chapter_id: "chapter-1".to_string(),
            }),
            bookmarks: vec![],
            highlights: vec![],
            notes: vec![],
            wall_clock_ts: logical_ts,
            logical_ts,
            origin_device_id: "device-a".to_string(),
            edit_history: vec![],
        }
    }

    fn sample_edit_history(count: usize) -> Vec<MetaEdit> {
        (0..count)
            .map(|index| MetaEdit {
                edit_id: format!("edit-{index}"),
                device_id: "device-a".to_string(),
                logical_ts: index as i64,
                op: None,
            })
            .collect()
    }

    fn facade_for(cache: &Path, remote: &Path, device_id: &str) -> KmoSyncFacade {
        let config = KmoSyncConfig {
            storage_config_json: format!(
                r#"{{"type":"file","root_dir":"{}"}}"#,
                remote.to_string_lossy()
            ),
            encryption_config_json: r#"{"type":"none"}"#.to_string(),
            device_id: device_id.to_string(),
            local_cache_dir: cache.to_path_buf(),
        };
        KmoSyncFacade::create(config, EventEmitter::new(None, std::ptr::null_mut())).unwrap()
    }

    fn facade_for_with_encryption(
        cache: &Path,
        remote: &Path,
        device_id: &str,
        encryption_config_json: String,
    ) -> KmoSyncFacade {
        let config = KmoSyncConfig {
            storage_config_json: format!(
                r#"{{"type":"file","root_dir":"{}"}}"#,
                remote.to_string_lossy()
            ),
            encryption_config_json,
            device_id: device_id.to_string(),
            local_cache_dir: cache.to_path_buf(),
        };
        KmoSyncFacade::create(config, EventEmitter::new(None, std::ptr::null_mut())).unwrap()
    }

    unsafe extern "C" fn capture_event_type(
        event_type: i32,
        _json_data: *const std::os::raw::c_char,
        _user_data: *mut std::os::raw::c_void,
    ) {
        if event_type == EventType::ClockDriftWarning as i32 {
            CLOCK_DRIFT_WARNING_SEEN.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn facade_create_rejects_invalid_json() {
        let config = KmoSyncConfig {
            storage_config_json: "{".to_string(),
            encryption_config_json: r#"{"type":"none"}"#.to_string(),
            device_id: "device-a".to_string(),
            local_cache_dir: tempfile::tempdir().unwrap().path().to_path_buf(),
        };

        let result = KmoSyncFacade::create(config, EventEmitter::new(None, std::ptr::null_mut()));
        assert!(result.is_err());
    }

    #[test]
    fn facade_create_initializes_local_database() {
        let dir = tempfile::tempdir().unwrap();
        let config = KmoSyncConfig {
            storage_config_json: r#"{"type":"memory"}"#.to_string(),
            encryption_config_json: r#"{"type":"none"}"#.to_string(),
            device_id: "device-a".to_string(),
            local_cache_dir: dir.path().to_path_buf(),
        };

        let facade =
            KmoSyncFacade::create(config, EventEmitter::new(None, std::ptr::null_mut())).unwrap();
        assert_eq!(facade.device_id(), "device-a");
        assert!(!facade.crypto_is_encrypted());
        assert!(dir.path().join("kmo_index.db").exists());
    }

    #[test]
    fn facade_create_warns_when_clock_moves_backwards() {
        let dir = tempfile::tempdir().unwrap();
        let conn = local_db::open_database(dir.path()).unwrap();
        conn.execute(
            "INSERT INTO sync_state(key, value) VALUES (?1, ?2)",
            rusqlite::params![
                "last_wall_clock_ts",
                (now_millis() + 2 * 60 * 60 * 1000).to_string()
            ],
        )
        .unwrap();
        drop(conn);
        CLOCK_DRIFT_WARNING_SEEN.store(false, Ordering::SeqCst);

        let config = KmoSyncConfig {
            storage_config_json: r#"{"type":"memory"}"#.to_string(),
            encryption_config_json: r#"{"type":"none"}"#.to_string(),
            device_id: "device-a".to_string(),
            local_cache_dir: dir.path().to_path_buf(),
        };

        let _facade = KmoSyncFacade::create(
            config,
            EventEmitter::new(Some(capture_event_type), std::ptr::null_mut()),
        )
        .unwrap();
        assert!(CLOCK_DRIFT_WARNING_SEEN.load(Ordering::SeqCst));
    }

    #[test]
    fn reeden_layout_no_sync_header_is_ever_written() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let facade = facade_for(cache.path(), remote.path(), "device-a");

        facade.sync_all(0).unwrap();

        assert!(!remote.path().join("kmo-sync/_sync_header.json").exists());
        assert!(!remote.path().join("yuewei/_sync_header.json").exists());
    }

    #[test]
    fn legacy_sync_header_is_ignored_by_the_flat_layout() {
        // Old protocol headers from previous versions should be silently ignored
        // because the new layout doesn't carry one at all.
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let header_path = remote.path().join("kmo-sync/_sync_header.json");
        std::fs::create_dir_all(header_path.parent().unwrap()).unwrap();
        std::fs::write(
            &header_path,
            br#"{"protocol_version":9999,"min_compatible_version":9999,"device_id":"future","last_modified_ts":0,"features":{"logical_ts":true,"tombstone":true,"edit_history":true,"fastcdc":true,"envelope_encryption":false}}"#,
        )
        .unwrap();
        let facade = facade_for(cache.path(), remote.path(), "device-a");

        // No version-mismatch error: the header is simply ignored.
        facade.sync_all(0).unwrap();
        assert!(header_path.exists());
    }

    #[test]
    fn put_local_meta_archives_large_edit_history() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let facade = facade_for(cache.path(), remote.path(), "device-a");
        let mut meta = sample_meta("meta-history", 0.25, 1);
        meta.edit_history = sample_edit_history(EDIT_HISTORY_INLINE_LIMIT + 1);

        facade.put_local_meta(&meta).unwrap();

        let stored = facade.read_local_meta("meta-history").unwrap().unwrap();
        assert_eq!(stored.edit_history.len(), EDIT_HISTORY_RETAINED_INLINE);
        assert_eq!(stored.edit_history[0].edit_id, "edit-901");
        let archive = std::fs::read(facade.local_history_path("meta-history")).unwrap();
        let decompressed = zstd::stream::decode_all(Cursor::new(archive)).unwrap();
        let edits: Vec<MetaEdit> = serde_json::from_slice(&decompressed).unwrap();
        assert_eq!(edits.len(), EDIT_HISTORY_INLINE_LIMIT + 1);
    }

    #[test]
    fn put_local_meta_archives_large_edit_history_remains_local_only() {
        // Edit history archives stay local — they are never written to the remote.
        let remote = tempfile::tempdir().unwrap();
        let cache_a = tempfile::tempdir().unwrap();
        let cache_b = tempfile::tempdir().unwrap();
        let facade_a = facade_for(cache_a.path(), remote.path(), "device-a");
        let facade_b = facade_for(cache_b.path(), remote.path(), "device-b");
        let mut meta = sample_meta("meta-history", 0.25, 1);
        meta.edit_history = sample_edit_history(EDIT_HISTORY_INLINE_LIMIT + 1);

        facade_a.put_local_meta(&meta).unwrap();
        facade_a.sync_single_meta("book-1", "meta-history").unwrap();

        assert!(
            !remote
                .path()
                .join("metas/meta-history.history.zst")
                .exists()
        );
        assert!(
            !remote
                .path()
                .join("book_progress/book-1.json")
                .join("meta-history")
                .exists()
        );

        // B will receive the meta without history (lww only carries progress+bookmarks),
        // so the local archive is never re-populated from the wire either.
        facade_b.sync_single_meta("book-1", "meta-history").unwrap();
        assert!(!facade_b.local_history_path("meta-history").exists());
    }

    #[test]
    fn meta_sync_roundtrip_between_two_devices_with_file_storage() {
        let remote = tempfile::tempdir().unwrap();
        let cache_a = tempfile::tempdir().unwrap();
        let cache_b = tempfile::tempdir().unwrap();
        let facade_a = facade_for(cache_a.path(), remote.path(), "device-a");
        let facade_b = facade_for(cache_b.path(), remote.path(), "device-b");

        let meta_a = sample_meta("meta-1", 0.25, 1);
        facade_a.put_local_meta(&meta_a).unwrap();
        facade_a.sync_single_meta("book-1", "meta-1").unwrap();

        facade_b.sync_single_meta("book-1", "meta-1").unwrap();
        let pulled_b = facade_b.read_local_meta("meta-1").unwrap().unwrap();
        assert_eq!(pulled_b.progress.unwrap().progress_percent, 0.25);

        let meta_b = sample_meta("meta-1", 0.75, 2);
        facade_b.put_local_meta(&meta_b).unwrap();
        facade_b.sync_single_meta("book-1", "meta-1").unwrap();

        facade_a.sync_single_meta("book-1", "meta-1").unwrap();
        let pulled_a = facade_a.read_local_meta("meta-1").unwrap().unwrap();
        assert_eq!(pulled_a.progress.unwrap().progress_percent, 0.75);
    }

    #[test]
    fn age_meta_sync_writes_ciphertext_and_second_device_decrypts() {
        let remote = tempfile::tempdir().unwrap();
        let cache_a = tempfile::tempdir().unwrap();
        let cache_b = tempfile::tempdir().unwrap();
        let identity = Identity::generate();
        let encryption = format!(
            r#"{{"type":"age","identity":"{}"}}"#,
            identity.to_string().expose_secret()
        );
        let facade_a = facade_for_with_encryption(
            cache_a.path(),
            remote.path(),
            "device-a",
            encryption.clone(),
        );
        let facade_b =
            facade_for_with_encryption(cache_b.path(), remote.path(), "device-b", encryption);

        let meta = sample_meta("meta-age-1", 0.33, 1);
        facade_a.put_local_meta(&meta).unwrap();
        facade_a.sync_single_meta("book-1", "meta-age-1").unwrap();

        let remote_ciphertext =
            std::fs::read(remote.path().join("book_progress/book-1.json.enc")).unwrap();
        let plaintext = encode_meta(&meta).unwrap();
        assert_ne!(remote_ciphertext, plaintext);
        assert!(!remote.path().join("book_progress/book-1.json").exists());

        facade_b.sync_single_meta("book-1", "meta-age-1").unwrap();
        let pulled = facade_b.read_local_meta("meta-age-1").unwrap().unwrap();
        assert_eq!(pulled.progress.unwrap().progress_percent, 0.33);
    }

    #[test]
    fn envelope_meta_sync_writes_ciphertext_and_second_device_decrypts() {
        let remote = tempfile::tempdir().unwrap();
        let cache_a = tempfile::tempdir().unwrap();
        let cache_b = tempfile::tempdir().unwrap();
        let encryption = r#"{
            "type":"envelope",
            "passphrase":"shared production passphrase",
            "kek_id":"device-group-a",
            "argon2_memory_cost":256,
            "argon2_time_cost":1,
            "argon2_parallelism":1
        }"#;
        let facade_a = facade_for_with_encryption(
            cache_a.path(),
            remote.path(),
            "device-a",
            encryption.to_string(),
        );
        let facade_b = facade_for_with_encryption(
            cache_b.path(),
            remote.path(),
            "device-b",
            encryption.to_string(),
        );

        let meta = sample_meta("meta-envelope-1", 0.44, 1);
        facade_a.put_local_meta(&meta).unwrap();
        facade_a
            .sync_single_meta("book-1", "meta-envelope-1")
            .unwrap();

        let remote_ciphertext =
            std::fs::read(remote.path().join("book_progress/book-1.json.env")).unwrap();
        let plaintext = encode_meta(&meta).unwrap();
        assert_ne!(remote_ciphertext, plaintext);
        assert!(remote_ciphertext.starts_with(b"KMOENV1\0"));
        assert!(!remote.path().join("book_progress/book-1.json").exists());

        facade_b
            .sync_single_meta("book-1", "meta-envelope-1")
            .unwrap();
        let pulled = facade_b
            .read_local_meta("meta-envelope-1")
            .unwrap()
            .unwrap();
        assert_eq!(pulled.progress.unwrap().progress_percent, 0.44);
    }

    #[test]
    fn envelope_kek_version_is_registered_with_local_db() {
        // Reeden-style flat layout has no remote sync header — instead the
        // envelope encryption feature is observable via the local DB.
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let encryption = r#"{
            "type":"envelope",
            "passphrase":"shared production passphrase",
            "kek_id":"device-group-a",
            "argon2_memory_cost":256,
            "argon2_time_cost":1,
            "argon2_parallelism":1
        }"#;
        let facade =
            facade_for_with_encryption(cache.path(), remote.path(), "device-a", encryption.into());

        facade.sync_all(0).unwrap();
        assert!(facade.crypto_is_encrypted());
        // No header object is written by the new layout.
        assert!(!remote.path().join("kmo-sync/_sync_header.json").exists());
    }

    #[test]
    fn rotate_envelope_kek_rewraps_remote_objects_and_updates_runtime_crypto() {
        let remote = tempfile::tempdir().unwrap();
        let cache_a = tempfile::tempdir().unwrap();
        let cache_old = tempfile::tempdir().unwrap();
        let cache_new = tempfile::tempdir().unwrap();
        let old_encryption = r#"{
            "type":"envelope",
            "passphrase":"old passphrase",
            "kek_id":"kek-old",
            "kek_version":1,
            "argon2_memory_cost":256,
            "argon2_time_cost":1,
            "argon2_parallelism":1
        }"#;
        let new_encryption = r#"{
            "type":"envelope",
            "passphrase":"new passphrase",
            "kek_id":"kek-new",
            "kek_version":2,
            "argon2_memory_cost":256,
            "argon2_time_cost":1,
            "argon2_parallelism":1
        }"#;
        let facade_a = facade_for_with_encryption(
            cache_a.path(),
            remote.path(),
            "device-a",
            old_encryption.to_string(),
        );
        let meta = sample_meta("meta-rotate-1", 0.55, 1);
        facade_a.put_local_meta(&meta).unwrap();
        facade_a
            .sync_single_meta("book-1", "meta-rotate-1")
            .unwrap();

        let remote_path = remote.path().join("book_progress/book-1.json.env");
        let before = std::fs::read(&remote_path).unwrap();
        let rewrapped = facade_a.rotate_envelope_kek(new_encryption).unwrap();
        let marker: ActiveRemoteNamespace = serde_json::from_slice(
            &std::fs::read(remote.path().join("_active_namespace.json")).unwrap(),
        )
        .unwrap();
        let rotated_path = remote
            .path()
            .join(marker.namespace)
            .join("book_progress/book-1.json.env");
        let after = std::fs::read(rotated_path).unwrap();

        assert_eq!(rewrapped, 1);
        assert_ne!(before, after);
        assert_eq!(std::fs::read(&remote_path).unwrap(), before);

        let old_reader = facade_for_with_encryption(
            cache_old.path(),
            remote.path(),
            "device-old",
            old_encryption.to_string(),
        );
        assert!(
            old_reader
                .sync_single_meta("book-1", "meta-rotate-1")
                .is_err()
        );

        let new_reader = facade_for_with_encryption(
            cache_new.path(),
            remote.path(),
            "device-new",
            new_encryption.to_string(),
        );
        new_reader
            .sync_single_meta("book-1", "meta-rotate-1")
            .unwrap();
        assert_eq!(
            new_reader
                .read_local_meta("meta-rotate-1")
                .unwrap()
                .unwrap()
                .progress
                .unwrap()
                .progress_percent,
            0.55
        );

        let rotated_meta = sample_meta("meta-rotate-2", 0.66, 2);
        facade_a.put_local_meta(&rotated_meta).unwrap();
        facade_a
            .sync_single_meta("book-1", "meta-rotate-2")
            .unwrap();
        new_reader
            .sync_single_meta("book-1", "meta-rotate-2")
            .unwrap();
        assert_eq!(
            new_reader
                .read_local_meta("meta-rotate-2")
                .unwrap()
                .unwrap()
                .progress
                .unwrap()
                .progress_percent,
            0.66
        );

        let db = facade_a
            .db
            .lock()
            .map_err(|_| SyncError::Internal("database mutex poisoned".to_string()))
            .unwrap();
        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM kek_versions WHERE version IN (1, 2)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn blob_sync_roundtrip_between_two_devices_with_file_storage() {
        let remote = tempfile::tempdir().unwrap();
        let cache_a = tempfile::tempdir().unwrap();
        let cache_b = tempfile::tempdir().unwrap();
        let facade_a = facade_for(cache_a.path(), remote.path(), "device-a");
        let facade_b = facade_for(cache_b.path(), remote.path(), "device-b");

        let source = cache_a.path().join("source.epub");
        std::fs::write(&source, b"epub bytes from device a").unwrap();
        let book_hash = blake3_file_hex(&source).unwrap();

        facade_a.put_local_book(&book_hash, &source).unwrap();
        facade_a.sync_book(&book_hash).unwrap();

        let remote_blob = remote.path().join(format!("books/{book_hash}"));
        assert_eq!(
            std::fs::read(&remote_blob).unwrap(),
            b"epub bytes from device a"
        );

        facade_b.sync_book(&book_hash).unwrap();
        let pulled = cache_b.path().join(format!("blobs/{book_hash}.epub"));
        assert_eq!(std::fs::read(pulled).unwrap(), b"epub bytes from device a");
    }

    #[test]
    fn reeden_layout_book_upload_writes_single_object_no_manifest() {
        let remote = tempfile::tempdir().unwrap();
        let cache_a = tempfile::tempdir().unwrap();
        let cache_b = tempfile::tempdir().unwrap();
        let facade_a = facade_for(cache_a.path(), remote.path(), "device-a");
        let facade_b = facade_for(cache_b.path(), remote.path(), "device-b");
        let source = cache_a.path().join("large.epub");
        // >5 MiB intentionally; the old code would have triggered CAS, the new
        // protocol always uploads a single object.
        let bytes: Vec<u8> = (0..(6 * 1024 * 1024))
            .map(|index| (index % 251) as u8)
            .collect();
        std::fs::write(&source, &bytes).unwrap();
        let book_hash = blake3_file_hex(&source).unwrap();

        facade_a.put_local_book(&book_hash, &source).unwrap();
        facade_a.sync_book(&book_hash).unwrap();

        // Single object lives directly under books/<hash>; no manifest, no cas folder.
        assert!(remote.path().join(format!("books/{book_hash}")).exists());
        assert!(
            !remote
                .path()
                .join(format!("books/{book_hash}/blobs"))
                .exists()
        );
        assert!(
            !remote
                .path()
                .join(format!("books/{book_hash}/blobs/{book_hash}.manifest.json"))
                .exists()
        );

        facade_b.sync_book(&book_hash).unwrap();
        let pulled = cache_b.path().join(format!("blobs/{book_hash}.epub"));
        assert_eq!(std::fs::read(pulled).unwrap(), bytes);
    }

    #[test]
    fn reeden_layout_pull_only_discovers_single_book_object() {
        let remote = tempfile::tempdir().unwrap();
        let cache_a = tempfile::tempdir().unwrap();
        let cache_b = tempfile::tempdir().unwrap();
        let facade_a = facade_for(cache_a.path(), remote.path(), "device-a");
        let facade_b = facade_for(cache_b.path(), remote.path(), "device-b");
        let source = cache_a.path().join("large.epub");
        let bytes: Vec<u8> = (0..(6 * 1024 * 1024))
            .map(|index| (index % 199) as u8)
            .collect();
        std::fs::write(&source, &bytes).unwrap();
        let book_hash = blake3_file_hex(&source).unwrap();
        facade_a.put_local_book(&book_hash, &source).unwrap();
        facade_a.sync_book(&book_hash).unwrap();

        facade_b.sync_all(2).unwrap();

        let pulled = cache_b.path().join(format!("blobs/{book_hash}.epub"));
        assert_eq!(std::fs::read(pulled).unwrap(), bytes);
    }

    #[test]
    fn reeden_layout_sync_progress_writes_only_one_object() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let facade = facade_for(cache.path(), remote.path(), "device-a");

        let mut meta = sample_meta("progress-meta", 0.42, 1);
        meta.bookmarks.push(Bookmark {
            bookmark_id: "bm-anchor".to_string(),
            cfi_range: "epubcfi(/6/2)".to_string(),
            title: "anchor".to_string(),
            create_ts: 1,
        });
        facade.put_local_meta(&meta).unwrap();
        facade.sync_all(0).unwrap();

        // First sync seeds the layout: one progress object + one bookmarks
        // object, no header.
        let progress_path = remote.path().join("book_progress/book-1.json");
        let bookmarks_path = remote.path().join("bookmarks/book-1.json");
        assert!(progress_path.exists());
        assert!(bookmarks_path.exists());

        // Bump the progress percent and resync. Only `book_progress/<hash>.json`
        // should be overwritten; `bookmarks/<hash>.json` must keep its byte
        // contents because the bookmark list is unchanged.
        let bookmarks_bytes_before = std::fs::read(&bookmarks_path).unwrap();

        let mut next = sample_meta("progress-meta", 0.43, 2);
        next.wall_clock_ts = now_millis();
        next.logical_ts = 2;
        // Carry the anchor bookmark so the bookmarks path stays identical.
        next.bookmarks.push(Bookmark {
            bookmark_id: "bm-anchor".to_string(),
            cfi_range: "epubcfi(/6/2)".to_string(),
            title: "anchor".to_string(),
            create_ts: 1,
        });
        facade.put_local_meta(&next).unwrap();
        facade.sync_all(0).unwrap();

        let bookmarks_bytes_after = std::fs::read(&bookmarks_path).unwrap();
        // Compare the bookmark/highlight/note lists — they must be byte-identical
        // even though the surrounding envelope (last_write_ts, etc.) gets
        // refreshed by the LWW merge.
        let before_lists = parse_remote_bookmarks_lists(&bookmarks_bytes_before);
        let after_lists = parse_remote_bookmarks_lists(&bookmarks_bytes_after);
        assert_eq!(
            before_lists, after_lists,
            "bookmark/highlight/note lists must not change on a progress-only update"
        );

        let bytes = std::fs::read(&progress_path).unwrap();
        let stored: RemoteProgress = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(stored.progress.unwrap().progress_percent, 0.43);
    }

    #[test]
    fn reeden_layout_sync_bookmarks_merges_by_id_and_takes_largest_create_ts() {
        let remote = tempfile::tempdir().unwrap();
        let cache_a = tempfile::tempdir().unwrap();
        let cache_b = tempfile::tempdir().unwrap();
        let facade_a = facade_for(cache_a.path(), remote.path(), "device-a");
        let facade_b = facade_for(cache_b.path(), remote.path(), "device-b");

        let book_hash = "book-1".to_string();
        let mut meta_a = sample_meta(&book_hash, 0.10, now_millis());
        meta_a.wall_clock_ts = now_millis();
        meta_a.bookmarks.push(Bookmark {
            bookmark_id: "bm-shared".to_string(),
            cfi_range: "epubcfi(/6/2)".to_string(),
            title: "first".to_string(),
            create_ts: 50,
        });
        meta_a.bookmarks.push(Bookmark {
            bookmark_id: "bm-a-only".to_string(),
            cfi_range: "epubcfi(/6/4)".to_string(),
            title: "a only".to_string(),
            create_ts: 60,
        });
        facade_a.put_local_meta(&meta_a).unwrap();
        facade_a.sync_all(0).unwrap();

        // B starts a moment later with one shared bookmark (older) + one
        // exclusive bookmark. The shared entry has a larger create_ts so the
        // per-item merge should prefer B's text.
        let mut meta_b = sample_meta(&book_hash, 0.20, now_millis());
        meta_b.wall_clock_ts = now_millis();
        meta_b.bookmarks.push(Bookmark {
            bookmark_id: "bm-shared".to_string(),
            cfi_range: "epubcfi(/6/2)".to_string(),
            title: "newer on b".to_string(),
            create_ts: 100,
        });
        meta_b.bookmarks.push(Bookmark {
            bookmark_id: "bm-b-only".to_string(),
            cfi_range: "epubcfi(/6/8)".to_string(),
            title: "b only".to_string(),
            create_ts: 90,
        });
        facade_b.put_local_meta(&meta_b).unwrap();
        facade_b.sync_all(0).unwrap();

        // After round-trip both devices see 3 bookmarks: shared one with the
        // larger create_ts (100 from B) plus each device's exclusive entry.
        for facade in [&facade_a, &facade_b] {
            facade.sync_all(0).unwrap();
            let merged = facade.read_local_meta(&book_hash).unwrap().unwrap();
            assert_eq!(merged.bookmarks.len(), 3);
            let shared = merged
                .bookmarks
                .iter()
                .find(|bm| bm.bookmark_id == "bm-shared")
                .unwrap();
            assert_eq!(
                shared.create_ts, 100,
                "shared entry should pick the newer ts"
            );
            assert!(
                merged
                    .bookmarks
                    .iter()
                    .any(|bm| bm.bookmark_id == "bm-a-only"),
                "device-a's exclusive bookmark must survive"
            );
            assert!(
                merged
                    .bookmarks
                    .iter()
                    .any(|bm| bm.bookmark_id == "bm-b-only"),
                "device-b's exclusive bookmark must survive"
            );
        }
    }

    #[test]
    fn blob_hash_mismatch_records_conflict_without_overwriting_remote() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let facade = facade_for(cache.path(), remote.path(), "device-a");
        let source = cache.path().join("source.epub");
        std::fs::write(&source, b"correct epub bytes").unwrap();
        let book_hash = blake3_file_hex(&source).unwrap();
        facade.put_local_book(&book_hash, &source).unwrap();

        let remote_blob = remote.path().join(format!("books/{book_hash}"));
        std::fs::create_dir_all(remote_blob.parent().unwrap()).unwrap();
        std::fs::write(&remote_blob, b"corrupt remote bytes").unwrap();

        facade.sync_book(&book_hash).unwrap();

        assert_eq!(
            std::fs::read(&remote_blob).unwrap(),
            b"corrupt remote bytes"
        );
        assert_eq!(facade.conflict_count().unwrap(), 1);
        let state = facade.get_sync_state_json().unwrap();
        assert!(state.contains("\"conflict_kind\":\"blob_file\""));
        assert!(state.contains("\"book_hash\""));
        assert!(state.contains("blob_hash_mismatch"));
    }

    #[test]
    fn resolve_blob_conflict_local_overwrites_remote_and_clears_conflict() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let facade = facade_for(cache.path(), remote.path(), "device-a");
        let source = cache.path().join("source.epub");
        std::fs::write(&source, b"correct epub bytes").unwrap();
        let book_hash = blake3_file_hex(&source).unwrap();
        facade.put_local_book(&book_hash, &source).unwrap();

        let remote_blob = remote.path().join(format!("books/{book_hash}"));
        std::fs::create_dir_all(remote_blob.parent().unwrap()).unwrap();
        std::fs::write(&remote_blob, b"corrupt remote bytes").unwrap();
        facade.sync_book(&book_hash).unwrap();

        facade.resolve_blob_conflict(&book_hash, "local").unwrap();

        assert_eq!(facade.conflict_count().unwrap(), 0);
        assert_eq!(std::fs::read(&remote_blob).unwrap(), b"correct epub bytes");
    }

    #[test]
    fn resolve_blob_conflict_remote_keeps_remote_and_clears_conflict() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let facade = facade_for(cache.path(), remote.path(), "device-a");
        let source = cache.path().join("source.epub");
        std::fs::write(&source, b"correct epub bytes").unwrap();
        let book_hash = blake3_file_hex(&source).unwrap();
        facade.put_local_book(&book_hash, &source).unwrap();

        let remote_blob = remote.path().join(format!("books/{book_hash}"));
        std::fs::create_dir_all(remote_blob.parent().unwrap()).unwrap();
        std::fs::write(&remote_blob, b"corrupt remote bytes").unwrap();
        facade.sync_book(&book_hash).unwrap();

        facade.resolve_blob_conflict(&book_hash, "remote").unwrap();

        assert_eq!(facade.conflict_count().unwrap(), 0);
        assert_eq!(
            std::fs::read(&remote_blob).unwrap(),
            b"corrupt remote bytes"
        );
    }

    #[test]
    fn age_blob_sync_writes_ciphertext_and_second_device_decrypts() {
        let remote = tempfile::tempdir().unwrap();
        let cache_a = tempfile::tempdir().unwrap();
        let cache_b = tempfile::tempdir().unwrap();
        let identity = Identity::generate();
        let encryption = format!(
            r#"{{"type":"age","identity":"{}"}}"#,
            identity.to_string().expose_secret()
        );
        let facade_a = facade_for_with_encryption(
            cache_a.path(),
            remote.path(),
            "device-a",
            encryption.clone(),
        );
        let facade_b =
            facade_for_with_encryption(cache_b.path(), remote.path(), "device-b", encryption);

        let source = cache_a.path().join("source.epub");
        std::fs::write(&source, b"encrypted epub bytes").unwrap();
        let book_hash = blake3_file_hex(&source).unwrap();

        facade_a.put_local_book(&book_hash, &source).unwrap();
        facade_a.sync_book(&book_hash).unwrap();

        let remote_blob = remote.path().join(format!("books/{book_hash}.enc"));
        let remote_bytes = std::fs::read(remote_blob).unwrap();
        assert_ne!(remote_bytes, b"encrypted epub bytes");

        facade_b.sync_book(&book_hash).unwrap();
        let pulled = cache_b.path().join(format!("blobs/{book_hash}.epub"));
        assert_eq!(std::fs::read(pulled).unwrap(), b"encrypted epub bytes");
    }

    #[test]
    fn sync_all_push_only_uploads_local_without_pulling_remote() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let facade = facade_for(cache.path(), remote.path(), "device-a");

        facade
            .put_local_meta(&sample_meta("local-meta", 0.1, 1))
            .unwrap();
        std::fs::create_dir_all(remote.path().join("books/book-1/metas")).unwrap();
        std::fs::create_dir_all(remote.path().join("book_progress")).unwrap();
        let remote_meta = sample_meta("remote-meta", 0.9, 2);
        let payload = serde_json::json!({
            "schema_version": 7,
            "book_hash": "book-1",
            "progress": remote_meta.progress.clone().map(|p| serde_json::json!({
                "cfi_position": p.cfi_position,
                "progress_percent": p.progress_percent,
                "chapter_id": p.chapter_id,
            })),
            "last_writer_device_id": remote_meta.device_id,
            "last_write_ts": remote_meta.wall_clock_ts,
        });
        std::fs::write(
            remote.path().join("book_progress/book-1.json"),
            serde_json::to_vec(&payload).expect("to_vec payload"),
        )
        .expect("fs::write to book_progress");
        eprintln!(
            "PRE-SYNC remote dir: {:?}",
            std::fs::read_dir(remote.path())
                .unwrap()
                .collect::<Vec<_>>()
        );

        let res = facade.sync_all(1);
        eprintln!("POST-SYNC result: {res:?}");
        res.unwrap();

        eprintln!(
            "POST-ASSERT remote dir: {:?}",
            std::fs::read_dir(remote.path())
                .unwrap()
                .collect::<Vec<_>>()
        );

        assert!(remote.path().join("book_progress/book-1.json").exists());
        assert!(!cache.path().join("metas/remote-meta.meta").exists());
    }

    #[test]
    fn sync_all_pull_only_downloads_remote_without_uploading_local() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let facade = facade_for(cache.path(), remote.path(), "device-a");

        facade
            .put_local_meta(&sample_meta("local-meta", 0.1, 1))
            .unwrap();
        std::fs::create_dir_all(remote.path().join("books/book-1/metas")).unwrap();
        std::fs::create_dir_all(remote.path().join("book_progress")).unwrap();
        let remote_meta = sample_meta("remote-meta", 0.9, 2);
        let payload = serde_json::json!({
            "schema_version": 7,
            "book_hash": "book-1",
            "progress": remote_meta.progress.clone().map(|p| serde_json::json!({
                "cfi_position": p.cfi_position,
                "progress_percent": p.progress_percent,
                "chapter_id": p.chapter_id,
            })),
            "last_writer_device_id": remote_meta.device_id,
            "last_write_ts": remote_meta.wall_clock_ts,
        });
        let before_bytes = serde_json::to_vec(&payload).unwrap();
        std::fs::write(
            remote.path().join("book_progress/book-1.json"),
            &before_bytes,
        )
        .unwrap();
        let before_mtime = std::fs::metadata(remote.path().join("book_progress/book-1.json"))
            .unwrap()
            .modified()
            .unwrap();

        let res = facade.sync_all(2);
        eprintln!("sync_all result: {:?}", res);
        res.unwrap();

        // PullOnly must not overwrite the remote file; remote is left exactly
        // as the other device wrote it.
        assert_eq!(
            std::fs::read(remote.path().join("book_progress/book-1.json")).unwrap(),
            before_bytes
        );
        let after_mtime = std::fs::metadata(remote.path().join("book_progress/book-1.json"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before_mtime, after_mtime);
        // New layout keys the per-book meta by book_hash, so the pulled remote
        // meta lands in <cache>/metas/book-1.meta (not remote-meta.meta).
        assert!(cache.path().join("metas/book-1.meta").exists());
    }

    #[test]
    fn sync_all_bidirectional_handles_local_and_remote_meta_and_blob() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let facade = facade_for(cache.path(), remote.path(), "device-a");

        facade
            .put_local_meta(&sample_meta("local-meta", 0.1, 1))
            .unwrap();

        std::fs::create_dir_all(remote.path().join("books/book-1/metas")).unwrap();
        std::fs::create_dir_all(remote.path().join("book_progress")).unwrap();
        let remote_meta = sample_meta("remote-meta", 0.9, 2);
        let payload = serde_json::json!({
            "schema_version": 7,
            "book_hash": "book-1",
            "progress": remote_meta.progress.clone().map(|p| serde_json::json!({
                "cfi_position": p.cfi_position,
                "progress_percent": p.progress_percent,
                "chapter_id": p.chapter_id,
            })),
            "last_writer_device_id": remote_meta.device_id,
            "last_write_ts": remote_meta.wall_clock_ts,
        });
        std::fs::write(
            remote.path().join("book_progress/book-1.json"),
            serde_json::to_vec(&payload).unwrap(),
        )
        .unwrap();

        let local_book = cache.path().join("source.epub");
        std::fs::write(&local_book, b"local book").unwrap();
        let local_book_hash = blake3_file_hex(&local_book).unwrap();
        facade
            .put_local_book(&local_book_hash, &local_book)
            .unwrap();

        let remote_book_bytes = b"remote book";
        let remote_book_hash = blake3::hash(remote_book_bytes).to_hex().to_string();
        std::fs::write(
            remote.path().join(format!("books/{remote_book_hash}")),
            remote_book_bytes,
        )
        .unwrap();

        facade.sync_all(0).unwrap();

        assert!(remote.path().join("book_progress/book-1.json").exists());
        // Pulled remote meta now lands at metas/book-1.meta (per-book key).
        assert!(cache.path().join("metas/book-1.meta").exists());
        assert!(
            remote
                .path()
                .join(format!("books/{local_book_hash}"))
                .exists()
        );
        assert_eq!(
            std::fs::read(cache.path().join(format!("blobs/{remote_book_hash}.epub"))).unwrap(),
            remote_book_bytes
        );
    }

    #[test]
    fn concurrent_meta_update_resolves_via_last_write_wins_without_recording_conflict() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let facade = facade_for(cache.path(), remote.path(), "device-a");

        let mut local_meta = sample_meta("conflict-meta", 0.25, 7);
        local_meta.device_id = "device-a".to_string();
        facade.put_local_meta(&local_meta).unwrap();

        let mut remote_meta = sample_meta("conflict-meta", 0.75, 8);
        remote_meta.device_id = "device-b".to_string();
        std::fs::create_dir_all(remote.path().join("book_progress")).unwrap();
        let payload = serde_json::json!({
            "schema_version": 7,
            "book_hash": "book-1",
            "progress": remote_meta.progress.clone().map(|p| serde_json::json!({
                "cfi_position": p.cfi_position,
                "progress_percent": p.progress_percent,
                "chapter_id": p.chapter_id,
            })),
            "last_writer_device_id": remote_meta.device_id,
            "last_write_ts": remote_meta.wall_clock_ts,
        });
        std::fs::write(
            remote.path().join("book_progress/book-1.json"),
            serde_json::to_vec(&payload).unwrap(),
        )
        .unwrap();

        facade.sync_single_meta("book-1", "conflict-meta").unwrap();

        // Reeden LWW: remote with the larger wall_clock_ts wins outright. No
        // conflict is recorded because conflict resolution is no longer a
        // protocol feature.
        assert_eq!(facade.conflict_count().unwrap(), 0);
        let kept = facade.read_local_meta("conflict-meta").unwrap().unwrap();
        assert_eq!(kept.progress.unwrap().progress_percent, 0.75);
        let remote_bytes = std::fs::read(remote.path().join("book_progress/book-1.json")).unwrap();
        let remote_kept: RemoteProgress = serde_json::from_slice(&remote_bytes).unwrap();
        assert_eq!(remote_kept.progress.unwrap().progress_percent, 0.75);
    }

    #[test]
    fn choosing_remote_conflict_version_pulls_without_overwriting_remote() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let facade = facade_for(cache.path(), remote.path(), "device-a");

        let mut local_meta = sample_meta("resolve-meta", 0.25, 7);
        local_meta.device_id = "device-a".to_string();
        facade.put_local_meta(&local_meta).unwrap();

        let mut remote_meta = sample_meta("resolve-meta", 0.75, 8);
        remote_meta.device_id = "device-b".to_string();
        std::fs::create_dir_all(remote.path().join("book_progress")).unwrap();
        let payload = serde_json::json!({
            "schema_version": 7,
            "book_hash": "book-1",
            "progress": remote_meta.progress.clone().map(|p| serde_json::json!({
                "cfi_position": p.cfi_position,
                "progress_percent": p.progress_percent,
                "chapter_id": p.chapter_id,
            })),
            "last_writer_device_id": remote_meta.device_id,
            "last_write_ts": remote_meta.wall_clock_ts,
        });
        std::fs::write(
            remote.path().join("book_progress/book-1.json"),
            serde_json::to_vec(&payload).unwrap(),
        )
        .unwrap();

        // No explicit conflict row exists, so the manual override must perform
        // a pull-only refresh instead of writing the stale local snapshot.
        assert_eq!(facade.conflict_count().unwrap(), 0);
        // resolve_meta_conflict still works as a manual override for power users.
        facade
            .resolve_meta_conflict("resolve-meta", "remote")
            .unwrap();

        assert_eq!(facade.conflict_count().unwrap(), 0);
        let local_after = facade.read_local_meta("resolve-meta").unwrap().unwrap();
        assert_eq!(local_after.progress.unwrap().progress_percent, 0.75);
        let remote_bytes = std::fs::read(remote.path().join("book_progress/book-1.json")).unwrap();
        let remote_after: RemoteProgress = serde_json::from_slice(&remote_bytes).unwrap();
        assert_eq!(remote_after.progress.unwrap().progress_percent, 0.75);
    }

    #[test]
    fn tombstone_delete_propagates_across_devices() {
        let remote = tempfile::tempdir().unwrap();
        let cache_a = tempfile::tempdir().unwrap();
        let cache_b = tempfile::tempdir().unwrap();
        let facade_a = facade_for(cache_a.path(), remote.path(), "device-a");
        let facade_b = facade_for(cache_b.path(), remote.path(), "device-b");

        let mut meta = sample_meta("tombstone-meta", 0.25, 1);
        meta.highlights.push(Highlight {
            highlight_id: "highlight-1".to_string(),
            cfi_start: "start".to_string(),
            cfi_end: "end".to_string(),
            color: "yellow".to_string(),
            comment: "note".to_string(),
            create_ts: 1,
        });
        facade_a.put_local_meta(&meta).unwrap();
        facade_a
            .sync_single_meta("book-1", "tombstone-meta")
            .unwrap();
        facade_b
            .sync_single_meta("book-1", "tombstone-meta")
            .unwrap();

        // A 删除 highlight-1
        facade_a
            .mark_meta_item_deleted("tombstone-meta", "highlight", "highlight-1")
            .unwrap();
        let local_after_delete = facade_a.read_local_meta("tombstone-meta").unwrap().unwrap();
        assert!(local_after_delete.highlights.is_empty());

        // A 的本地 tombstone 有 1 条
        assert_eq!(
            facade_a
                .read_local_tombstones_for_test("tombstone-meta")
                .unwrap()
                .len(),
            1
        );

        // A sync → tombstone 传播到远端
        facade_a
            .sync_single_meta("book-1", "tombstone-meta")
            .unwrap();

        // B sync → 拉取远端 tombstone，过滤掉已删除的 highlight-1
        facade_b
            .sync_single_meta("book-1", "tombstone-meta")
            .unwrap();
        let b_meta = facade_b.read_local_meta("tombstone-meta").unwrap().unwrap();
        // 删除已跨设备传播：B 不再有 highlight-1（不再复活）
        assert_eq!(b_meta.highlights.len(), 0);
        // B 的本地 tombstone 通过同步获得了 1 条
        assert_eq!(
            facade_b
                .read_local_tombstones_for_test("tombstone-meta")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn tombstone_revival_propagates_across_devices() {
        let remote = tempfile::tempdir().unwrap();
        let cache_a = tempfile::tempdir().unwrap();
        let cache_b = tempfile::tempdir().unwrap();
        let facade_a = facade_for(cache_a.path(), remote.path(), "device-a");
        let facade_b = facade_for(cache_b.path(), remote.path(), "device-b");

        let mut meta = sample_meta("revival-meta", 0.25, 1);
        meta.highlights.push(Highlight {
            highlight_id: "highlight-1".to_string(),
            cfi_start: "start".to_string(),
            cfi_end: "end".to_string(),
            color: "yellow".to_string(),
            comment: "note".to_string(),
            create_ts: 1,
        });
        facade_a.put_local_meta(&meta).unwrap();
        facade_a.sync_single_meta("book-1", "revival-meta").unwrap();
        facade_b.sync_single_meta("book-1", "revival-meta").unwrap();

        // A 删除 highlight-1 并同步（tombstone 传播到远端，远端 bookmarks 清空）
        facade_a
            .mark_meta_item_deleted("revival-meta", "highlight", "highlight-1")
            .unwrap();
        facade_a.sync_single_meta("book-1", "revival-meta").unwrap();

        // A undo → tombstone 被 revive，记录 revival
        facade_a
            .undo_deletion("revival-meta", "highlight-1")
            .unwrap();
        let a_tombstones = facade_a
            .read_local_tombstones_for_test("revival-meta")
            .unwrap();
        assert_eq!(a_tombstones.tombstones.len(), 0);
        assert_eq!(a_tombstones.revivals.len(), 1);

        // A sync → revival 传播到远端（远端 tombstone 获得 revival 记录）
        facade_a.sync_single_meta("book-1", "revival-meta").unwrap();

        // B sync → B 本地缓存有 highlight-1，union 逻辑会救回它
        // 如果没有 revival，tombstone 会过滤掉救回的 highlight-1
        // 有了 revival，tombstone 被撤销，highlight-1 保留
        facade_b.sync_single_meta("book-1", "revival-meta").unwrap();
        let b_meta = facade_b.read_local_meta("revival-meta").unwrap().unwrap();
        assert_eq!(b_meta.highlights.len(), 1);
        assert_eq!(b_meta.highlights[0].highlight_id, "highlight-1");

        // B 的 tombstone 也有 revival 记录
        let b_tombstones = facade_b
            .read_local_tombstones_for_test("revival-meta")
            .unwrap();
        let h1_tombstone = b_tombstones
            .tombstones
            .iter()
            .find(|t| t.uuid == "highlight-1")
            .expect("tombstone should exist");
        assert!(b_tombstones.is_revived(h1_tombstone));
    }

    #[test]
    fn tombstone_revival_is_resolved_by_last_write_wins() {
        // New layout has no tombstone-revival conflict path. Mark-delete on A
        // is local-only until A's own meta syncs; meanwhile B's offline edit
        // propagates through bookmarks LWW merge.
        let remote = tempfile::tempdir().unwrap();
        let cache_a = tempfile::tempdir().unwrap();
        let cache_b = tempfile::tempdir().unwrap();
        let facade_a = facade_for(cache_a.path(), remote.path(), "device-a");
        let facade_b = facade_for(cache_b.path(), remote.path(), "device-b");

        let mut base = sample_meta("revival-meta", 0.25, 1);
        base.highlights.push(Highlight {
            highlight_id: "highlight-1".to_string(),
            cfi_start: "start".to_string(),
            cfi_end: "end".to_string(),
            color: "yellow".to_string(),
            comment: "original".to_string(),
            create_ts: 1,
        });
        facade_a.put_local_meta(&base).unwrap();
        facade_a.sync_single_meta("book-1", "revival-meta").unwrap();
        facade_b.sync_single_meta("book-1", "revival-meta").unwrap();

        facade_a
            .mark_meta_item_deleted("revival-meta", "highlight", "highlight-1")
            .unwrap();

        let mut resurrected = base;
        resurrected.device_id = "device-b".to_string();
        resurrected.logical_ts = 3;
        resurrected.modified_ts = 3;
        resurrected.wall_clock_ts = 3;
        resurrected.highlights[0].comment = "offline edit after delete".to_string();
        facade_b.put_local_meta(&resurrected).unwrap();

        facade_b.sync_single_meta("book-1", "revival-meta").unwrap();

        // No tombstone revival conflict recorded in the reeden layout.
        assert_eq!(facade_b.conflict_count().unwrap(), 0);
        let local_b = facade_b.read_local_meta("revival-meta").unwrap().unwrap();
        assert_eq!(local_b.highlights[0].comment, "offline edit after delete");

        let remote_bytes = std::fs::read(remote.path().join("book_progress/book-1.json")).unwrap();
        let remote_progress: RemoteProgress = serde_json::from_slice(&remote_bytes).unwrap();
        // Progress JSON was overwritten by B's higher wall_clock_ts.
        assert_eq!(remote_progress.last_write_ts, resurrected.wall_clock_ts);
        assert_eq!(remote_progress.last_writer_device_id, "device-b");

        let bookmarks_bytes = std::fs::read(remote.path().join("bookmarks/book-1.json")).unwrap();
        let remote_bookmarks: RemoteBookmarks = serde_json::from_slice(&bookmarks_bytes).unwrap();
        assert_eq!(remote_bookmarks.highlights.len(), 1);
        assert_eq!(
            remote_bookmarks.highlights[0].comment,
            "offline edit after delete"
        );
    }

    #[test]
    fn resolve_tombstone_revival_is_local_only_in_reeden_layout() {
        let remote = tempfile::tempdir().unwrap();
        let cache_a = tempfile::tempdir().unwrap();
        let cache_b = tempfile::tempdir().unwrap();
        let facade_a = facade_for(cache_a.path(), remote.path(), "device-a");
        let facade_b = facade_for(cache_b.path(), remote.path(), "device-b");

        let mut base = sample_meta("restore-meta", 0.25, 1);
        base.highlights.push(Highlight {
            highlight_id: "highlight-1".to_string(),
            cfi_start: "start".to_string(),
            cfi_end: "end".to_string(),
            color: "yellow".to_string(),
            comment: "original".to_string(),
            create_ts: 1,
        });
        facade_a.put_local_meta(&base).unwrap();
        facade_a.sync_single_meta("book-1", "restore-meta").unwrap();
        facade_b.sync_single_meta("book-1", "restore-meta").unwrap();

        facade_a
            .mark_meta_item_deleted("restore-meta", "highlight", "highlight-1")
            .unwrap();

        let mut resurrected = base;
        resurrected.device_id = "device-b".to_string();
        resurrected.logical_ts = 3;
        resurrected.modified_ts = 3;
        resurrected.wall_clock_ts = 3;
        resurrected.highlights[0].comment = "restore this".to_string();
        facade_b.put_local_meta(&resurrected).unwrap();
        facade_b.sync_single_meta("book-1", "restore-meta").unwrap();

        // Manual resolution still works as a local operation: invoking it on B
        // does not error even when there is no conflict record.
        facade_b
            .resolve_tombstone_revival("restore-meta", "highlight-1", "restore")
            .unwrap();

        assert_eq!(facade_b.conflict_count().unwrap(), 0);
        let local_after_resolve = facade_b.read_local_meta("restore-meta").unwrap().unwrap();
        assert_eq!(local_after_resolve.highlights[0].comment, "restore this");

        facade_b.sync_single_meta("book-1", "restore-meta").unwrap();
        let bookmarks_bytes = std::fs::read(remote.path().join("bookmarks/book-1.json")).unwrap();
        let remote_after_resolve: RemoteBookmarks =
            serde_json::from_slice(&bookmarks_bytes).unwrap();
        assert_eq!(remote_after_resolve.highlights[0].comment, "restore this");
    }

    #[test]
    fn cellular_sync_all_syncs_meta_and_pauses_blob() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let facade = facade_for(cache.path(), remote.path(), "device-a");

        facade.set_network_type(1).unwrap();
        facade
            .put_local_meta(&sample_meta("cellular-meta", 0.4, 1))
            .unwrap();

        let local_book = cache.path().join("cellular.epub");
        std::fs::write(&local_book, b"cellular book").unwrap();
        let local_book_hash = blake3_file_hex(&local_book).unwrap();
        facade
            .put_local_book(&local_book_hash, &local_book)
            .unwrap();

        facade.sync_all(0).unwrap();

        assert!(remote.path().join("book_progress/book-1.json").exists());
        assert!(
            !remote
                .path()
                .join(format!("books/{local_book_hash}"))
                .exists()
        );
    }

    #[test]
    fn unknown_network_defaults_to_meta_only() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let facade = facade_for(cache.path(), remote.path(), "device-a");

        facade.set_network_type(2).unwrap();
        facade
            .put_local_meta(&sample_meta("unknown-meta", 0.4, 1))
            .unwrap();

        let local_book = cache.path().join("unknown.epub");
        std::fs::write(&local_book, b"unknown book").unwrap();
        let local_book_hash = blake3_file_hex(&local_book).unwrap();
        facade
            .put_local_book(&local_book_hash, &local_book)
            .unwrap();

        facade.sync_all(0).unwrap();

        assert!(remote.path().join("book_progress/book-1.json").exists());
        assert!(
            !remote
                .path()
                .join(format!("books/{local_book_hash}"))
                .exists()
        );
    }

    #[test]
    fn blob_byte_limit_pauses_before_writing_remote_blob() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let facade = facade_for(cache.path(), remote.path(), "device-a");

        let local_book = cache.path().join("limited.epub");
        std::fs::write(&local_book, b"this blob is larger than the limit").unwrap();
        let local_book_hash = blake3_file_hex(&local_book).unwrap();
        facade
            .put_local_book(&local_book_hash, &local_book)
            .unwrap();
        facade.set_blob_byte_limit(4).unwrap();

        facade.sync_all(0).unwrap();

        assert!(
            !remote
                .path()
                .join(format!("books/{local_book_hash}"))
                .exists()
        );
    }

    #[test]
    fn sync_book_returns_network_error_when_blob_policy_paused() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let facade = facade_for(cache.path(), remote.path(), "device-a");

        facade.set_network_type(1).unwrap();
        let err = facade.sync_book("book-hash").unwrap_err();

        assert_eq!(err.code(), 1);
        assert!(err.to_string().contains("cellular"));
    }

    #[test]
    fn remote_paths_use_kmo_sync_flat_layout() {
        // No cryptography involved: verify the path builders layout.
        let book_hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(
            KmoSyncFacade::remote_book_dir(book_hash),
            format!("books/{book_hash}")
        );
    }

    #[test]
    fn newer_remote_highlight_content_is_not_replaced_by_stale_local_content() {
        let mut local = sample_meta("meta-merge", 0.1, 10);
        local.device_id = "device-a".to_string();
        local.highlights.push(Highlight {
            highlight_id: "shared".to_string(),
            cfi_start: "a".to_string(),
            cfi_end: "b".to_string(),
            color: "yellow".to_string(),
            comment: "stale".to_string(),
            create_ts: 1,
        });
        let remote = RemoteBookmarks {
            schema_version: REMOTE_PROTOCOL_VERSION,
            book_hash: "book-1".to_string(),
            bookmarks: vec![],
            highlights: vec![Highlight {
                comment: "new remote comment".to_string(),
                ..local.highlights[0].clone()
            }],
            notes: vec![],
            last_writer_device_id: "device-b".to_string(),
            last_write_ts: 20,
        };
        let merged = merge_meta_with_remote(Some(local), None, Some(&remote));
        assert_eq!(merged.highlights[0].comment, "new remote comment");
    }

    #[test]
    fn undo_deletion_restores_the_snapshot_on_the_same_device() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let facade = facade_for(cache.path(), remote.path(), "device-a");
        let mut meta = sample_meta("undo-meta", 0.1, 1);
        meta.highlights.push(Highlight {
            highlight_id: "highlight-undo".to_string(),
            cfi_start: "a".to_string(),
            cfi_end: "b".to_string(),
            color: "yellow".to_string(),
            comment: "restore me".to_string(),
            create_ts: 1,
        });
        facade.put_local_meta(&meta).unwrap();
        facade
            .mark_meta_item_deleted("undo-meta", "highlight", "highlight-undo")
            .unwrap();
        facade.undo_deletion("undo-meta", "highlight-undo").unwrap();
        let restored = facade.read_local_meta("undo-meta").unwrap().unwrap();
        assert_eq!(restored.highlights.len(), 1);
        assert_eq!(restored.highlights[0].comment, "restore me");
    }

    #[test]
    fn unsafe_metadata_identifiers_are_rejected() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let facade = facade_for(cache.path(), remote.path(), "device-a");
        let mut meta = sample_meta("../escape", 0.1, 1);
        assert!(facade.put_local_meta(&meta).is_err());
        meta.meta_id = "/tmp/escape".to_string();
        assert!(facade.put_local_meta(&meta).is_err());
    }

    #[test]
    fn simultaneous_unique_bookmarks_converge_without_lost_updates() {
        let remote = tempfile::tempdir().unwrap();
        let cache_a = tempfile::tempdir().unwrap();
        let cache_b = tempfile::tempdir().unwrap();
        let facade_a = std::sync::Arc::new(facade_for(cache_a.path(), remote.path(), "device-a"));
        let facade_b = std::sync::Arc::new(facade_for(cache_b.path(), remote.path(), "device-b"));
        let mut meta_a = sample_meta("cas-meta", 0.1, 10);
        meta_a.device_id = "device-a".to_string();
        meta_a.bookmarks.push(Bookmark {
            bookmark_id: "only-a".to_string(),
            cfi_range: "a".to_string(),
            title: "a".to_string(),
            create_ts: 10,
        });
        let mut meta_b = sample_meta("cas-meta", 0.1, 10);
        meta_b.device_id = "device-b".to_string();
        meta_b.bookmarks.push(Bookmark {
            bookmark_id: "only-b".to_string(),
            cfi_range: "b".to_string(),
            title: "b".to_string(),
            create_ts: 10,
        });
        facade_a.put_local_meta(&meta_a).unwrap();
        facade_b.put_local_meta(&meta_b).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let thread_a = {
            let facade = facade_a.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                facade.sync_single_meta("book-1", "cas-meta")
            })
        };
        let thread_b = {
            let facade = facade_b.clone();
            std::thread::spawn(move || {
                barrier.wait();
                facade.sync_single_meta("book-1", "cas-meta")
            })
        };
        thread_a.join().unwrap().unwrap();
        thread_b.join().unwrap().unwrap();
        facade_a.sync_single_meta("book-1", "cas-meta").unwrap();
        let merged = facade_a.read_local_meta("cas-meta").unwrap().unwrap();
        assert!(
            merged
                .bookmarks
                .iter()
                .any(|item| item.bookmark_id == "only-a")
        );
        assert!(
            merged
                .bookmarks
                .iter()
                .any(|item| item.bookmark_id == "only-b")
        );
    }

    #[test]
    fn incompatible_remote_metadata_schema_is_rejected() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let facade = facade_for(cache.path(), remote.path(), "device-a");
        std::fs::create_dir_all(remote.path().join("book_progress")).unwrap();
        std::fs::write(
            remote.path().join("book_progress/book-1.json"),
            br#"{"schema_version":999,"book_hash":"book-1","progress":null,"last_writer_device_id":"future","last_write_ts":1}"#,
        )
        .unwrap();
        let error = facade
            .sync_single_meta("book-1", "schema-meta")
            .unwrap_err();
        assert_eq!(error.code(), 11);
    }
}
