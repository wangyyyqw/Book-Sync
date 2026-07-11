@file:OptIn(kotlinx.cinterop.ExperimentalForeignApi::class)

package com.kmosync

import com.kmosync.native.*
import kotlinx.cinterop.CPointer
import kotlinx.cinterop.StableRef
import kotlinx.cinterop.asStableRef
import kotlinx.cinterop.pointed
import kotlinx.cinterop.staticCFunction
import kotlinx.cinterop.toKString
import platform.Foundation.NSLock

internal actual class NativeKmoSyncBridge actual constructor(
    config: KmoSyncConfig,
    emitEvent: (SyncEvent) -> Unit,
) : AutoCloseable {
    private val callbackRef = StableRef.create(emitEvent)
    private val lock = NSLock()
    private var handle: CPointer<kmo_sync_t>? = kmo_sync_create(
        config.storageConfigJson,
        config.encryptionConfigJson,
        config.deviceId,
        config.localCacheDir,
        staticCFunction { eventType, json, userData ->
            if (json != null && userData != null) {
                val emit = userData.asStableRef<(SyncEvent) -> Unit>().get()
                emit(SyncEvent(SyncEventType.fromWireValue(eventType), eventType, json.toKString()))
            }
        },
        callbackRef.asCPointer(),
    ) ?: error("kmo_sync_create returned null")

    actual fun syncAll(mode: Int): Int = withHandleCode { kmo_sync_all(it, mode) }
    actual fun syncSingleMeta(bookHash: String, metaId: String): Int =
        withHandleCode { kmo_sync_single_meta(it, bookHash, metaId) }
    actual fun syncBook(bookHash: String): Int = withHandleCode { kmo_sync_book(it, bookHash) }
    actual fun putLocalBook(bookHash: String, localFilePath: String): Int =
        withHandleCode { kmo_sync_put_local_book(it, bookHash, localFilePath) }
    actual fun putLocalMetaJson(metaJson: String): Int =
        withHandleCode { kmo_sync_put_local_meta_json(it, metaJson) }
    actual fun resolveMetaConflict(metaId: String, chosenVersion: String): Int =
        withHandleCode { kmo_sync_resolve_meta_file_conflict(it, metaId, chosenVersion) }
    actual fun resolveBlobConflict(bookHash: String, chosenVersion: String): Int =
        withHandleCode { kmo_sync_resolve_blob_conflict(it, bookHash, chosenVersion) }
    actual fun rotateEnvelopeKek(newEncryptionConfigJson: String): Int =
        withHandleCode { kmo_sync_rotate_envelope_kek(it, newEncryptionConfigJson) }
    actual fun markMetaItemDeleted(metaId: String, itemType: String, itemUuid: String): Int =
        withHandleCode { kmo_sync_mark_meta_item_deleted(it, metaId, itemType, itemUuid) }
    actual fun undoDeletion(metaId: String, itemUuid: String): Int =
        withHandleCode { kmo_sync_undo_deletion(it, metaId, itemUuid) }
    actual fun resolveTombstoneRevival(metaId: String, itemUuid: String, resolution: String): Int =
        withHandleCode { kmo_sync_resolve_tombstone_revival(it, metaId, itemUuid, resolution) }
    actual fun setNetworkType(networkType: Int): Int =
        withHandleCode { kmo_sync_set_network_type(it, networkType) }
    actual fun setBlobByteLimit(byteLimit: Long): Int =
        withHandleCode { kmo_sync_set_blob_byte_limit(it, byteLimit) }
    actual fun pause(): Int = withHandleCode { kmo_sync_pause(it) }
    actual fun resume(): Int = withHandleCode { kmo_sync_resume(it) }

    actual fun getLocalMeta(metaId: String): String? =
        withHandleOrNull { consumeNativeString(kmo_sync_get_local_meta(it, metaId)) }
    actual fun getSyncState(): String? =
        withHandleOrNull { consumeNativeString(kmo_sync_get_sync_state(it)) }
    actual fun lastError(): String =
        withHandleOrNull { consumeNativeString(kmo_sync_last_error(it)).orEmpty() }
            ?: "KmoSync is closed"

    actual override fun close() {
        lock.lock()
        try {
            val current = handle ?: return
            handle = null
            kmo_sync_destroy(current)
            callbackRef.dispose()
        } finally {
            lock.unlock()
        }
    }

    private inline fun withHandleCode(block: (CPointer<kmo_sync_t>) -> Int): Int {
        lock.lock()
        return try {
            handle?.let(block) ?: ErrorCode.InvalidArg.code
        } finally {
            lock.unlock()
        }
    }

    private inline fun <T> withHandleOrNull(block: (CPointer<kmo_sync_t>) -> T): T? {
        lock.lock()
        return try {
            handle?.let(block)
        } finally {
            lock.unlock()
        }
    }

    private fun consumeNativeString(value: kotlinx.cinterop.CPointer<kotlinx.cinterop.ByteVar>?): String? {
        if (value == null) return null
        return try {
            value.toKString()
        } finally {
            kmo_sync_free_string(value)
        }
    }
}
