package com.kmosync

import java.util.concurrent.atomic.AtomicBoolean

internal actual class NativeKmoSyncBridge actual constructor(
    config: KmoSyncConfig,
    emitEvent: (SyncEvent) -> Unit,
) : AutoCloseable {
    private val closed = AtomicBoolean(false)
    private val nativeEventCallback = NativeEventCallback { eventType, json ->
        emitEvent(SyncEvent(SyncEventType.fromWireValue(eventType), eventType, json))
    }
    private val handle: Long = KmoSyncJni.create(
        config.storageConfigJson,
        config.encryptionConfigJson,
        config.deviceId,
        config.localCacheDir,
        nativeEventCallback,
    )

    init {
        require(handle != 0L) { "kmo_sync_create returned null" }
    }

    actual fun syncAll(mode: Int): Int = KmoSyncJni.syncAll(handle, mode)

    actual fun syncSingleMeta(bookHash: String, metaId: String): Int =
        KmoSyncJni.syncSingleMeta(handle, bookHash, metaId)

    actual fun syncBook(bookHash: String): Int = KmoSyncJni.syncBook(handle, bookHash)

    actual fun putLocalBook(bookHash: String, localFilePath: String): Int =
        KmoSyncJni.putLocalBook(handle, bookHash, localFilePath)

    actual fun putLocalMetaJson(metaJson: String): Int = KmoSyncJni.putLocalMetaJson(handle, metaJson)

    actual fun resolveMetaConflict(metaId: String, chosenVersion: String): Int =
        KmoSyncJni.resolveMetaConflict(handle, metaId, chosenVersion)

    actual fun resolveBlobConflict(bookHash: String, chosenVersion: String): Int =
        KmoSyncJni.resolveBlobConflict(handle, bookHash, chosenVersion)

    actual fun rotateEnvelopeKek(newEncryptionConfigJson: String): Int =
        KmoSyncJni.rotateEnvelopeKek(handle, newEncryptionConfigJson)

    actual fun markMetaItemDeleted(metaId: String, itemType: String, itemUuid: String): Int =
        KmoSyncJni.markMetaItemDeleted(handle, metaId, itemType, itemUuid)

    actual fun undoDeletion(metaId: String, itemUuid: String): Int =
        KmoSyncJni.undoDeletion(handle, metaId, itemUuid)

    actual fun resolveTombstoneRevival(
        metaId: String,
        itemUuid: String,
        resolution: String,
    ): Int = KmoSyncJni.resolveTombstoneRevival(handle, metaId, itemUuid, resolution)

    actual fun getLocalMeta(metaId: String): String? = KmoSyncJni.getLocalMeta(handle, metaId)

    actual fun getSyncState(): String? = KmoSyncJni.getSyncState(handle)

    actual fun setNetworkType(networkType: Int): Int =
        KmoSyncJni.setNetworkType(handle, networkType)

    actual fun setBlobByteLimit(byteLimit: Long): Int =
        KmoSyncJni.setBlobByteLimit(handle, byteLimit)

    actual fun pause(): Int = KmoSyncJni.pause(handle)

    actual fun resume(): Int = KmoSyncJni.resume(handle)

    actual fun lastError(): String = KmoSyncJni.lastError(handle)

    actual override fun close() {
        if (closed.compareAndSet(false, true)) {
            KmoSyncJni.destroy(handle)
        }
    }
}

private object KmoSyncJni {
    init {
        System.loadLibrary("kmo_sync")
    }

    external fun create(
        storageConfigJson: String,
        encryptionConfigJson: String,
        deviceId: String,
        localCacheDir: String,
        callback: NativeEventCallback,
    ): Long

    external fun destroy(handle: Long)
    external fun syncAll(handle: Long, mode: Int): Int
    external fun syncSingleMeta(handle: Long, bookHash: String, metaId: String): Int
    external fun syncBook(handle: Long, bookHash: String): Int
    external fun putLocalBook(handle: Long, bookHash: String, localFilePath: String): Int
    external fun putLocalMetaJson(handle: Long, metaJson: String): Int
    external fun resolveMetaConflict(handle: Long, metaId: String, chosenVersion: String): Int
    external fun resolveBlobConflict(handle: Long, bookHash: String, chosenVersion: String): Int
    external fun rotateEnvelopeKek(handle: Long, newEncryptionConfigJson: String): Int
    external fun markMetaItemDeleted(
        handle: Long,
        metaId: String,
        itemType: String,
        itemUuid: String,
    ): Int
    external fun undoDeletion(handle: Long, metaId: String, itemUuid: String): Int
    external fun resolveTombstoneRevival(
        handle: Long,
        metaId: String,
        itemUuid: String,
        resolution: String,
    ): Int
    external fun getLocalMeta(handle: Long, metaId: String): String?
    external fun getSyncState(handle: Long): String?
    external fun setNetworkType(handle: Long, networkType: Int): Int
    external fun setBlobByteLimit(handle: Long, byteLimit: Long): Int
    external fun pause(handle: Long): Int
    external fun resume(handle: Long): Int
    external fun lastError(handle: Long): String
}

private fun interface NativeEventCallback {
    fun onEvent(eventType: Int, json: String)
}
