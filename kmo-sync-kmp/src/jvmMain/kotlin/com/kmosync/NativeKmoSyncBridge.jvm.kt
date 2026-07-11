package com.kmosync

import com.sun.jna.Callback
import com.sun.jna.Library
import com.sun.jna.Native
import com.sun.jna.Pointer
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.util.Locale
import java.util.concurrent.atomic.AtomicBoolean

internal actual class NativeKmoSyncBridge actual constructor(
    config: KmoSyncConfig,
    emitEvent: (SyncEvent) -> Unit,
) : AutoCloseable {
    private val closed = AtomicBoolean(false)
    private val callback = NativeCallback { eventType, jsonPointer, _ ->
        val json = jsonPointer?.getString(0) ?: ""
        emitEvent(SyncEvent(SyncEventType.fromWireValue(eventType), eventType, json))
    }
    private val handle: Pointer

    init {
        handle = native.kmo_sync_create(
            config.storageConfigJson,
            config.encryptionConfigJson,
            config.deviceId,
            config.localCacheDir,
            callback,
            null,
        ) ?: error("kmo_sync_create returned null")
    }

    actual fun syncAll(mode: Int): Int = native.kmo_sync_all(handle, mode)

    actual fun syncSingleMeta(bookHash: String, metaId: String): Int =
        native.kmo_sync_single_meta(handle, bookHash, metaId)

    actual fun syncBook(bookHash: String): Int = native.kmo_sync_book(handle, bookHash)

    actual fun putLocalBook(bookHash: String, localFilePath: String): Int =
        native.kmo_sync_put_local_book(handle, bookHash, localFilePath)

    actual fun putLocalMetaJson(metaJson: String): Int =
        native.kmo_sync_put_local_meta_json(handle, metaJson)

    actual fun resolveMetaConflict(metaId: String, chosenVersion: String): Int =
        native.kmo_sync_resolve_meta_file_conflict(handle, metaId, chosenVersion)

    actual fun resolveBlobConflict(bookHash: String, chosenVersion: String): Int =
        native.kmo_sync_resolve_blob_conflict(handle, bookHash, chosenVersion)

    actual fun rotateEnvelopeKek(newEncryptionConfigJson: String): Int =
        native.kmo_sync_rotate_envelope_kek(handle, newEncryptionConfigJson)

    actual fun markMetaItemDeleted(metaId: String, itemType: String, itemUuid: String): Int =
        native.kmo_sync_mark_meta_item_deleted(handle, metaId, itemType, itemUuid)

    actual fun undoDeletion(metaId: String, itemUuid: String): Int =
        native.kmo_sync_undo_deletion(handle, metaId, itemUuid)

    actual fun resolveTombstoneRevival(
        metaId: String,
        itemUuid: String,
        resolution: String,
    ): Int = native.kmo_sync_resolve_tombstone_revival(handle, metaId, itemUuid, resolution)

    actual fun getLocalMeta(metaId: String): String? {
        val pointer = native.kmo_sync_get_local_meta(handle, metaId) ?: return null
        return try {
            pointer.getString(0)
        } finally {
            native.kmo_sync_free_string(pointer)
        }
    }

    actual fun getSyncState(): String? {
        val pointer = native.kmo_sync_get_sync_state(handle) ?: return null
        return try {
            pointer.getString(0)
        } finally {
            native.kmo_sync_free_string(pointer)
        }
    }

    actual fun setNetworkType(networkType: Int): Int =
        native.kmo_sync_set_network_type(handle, networkType)

    actual fun setBlobByteLimit(byteLimit: Long): Int =
        native.kmo_sync_set_blob_byte_limit(handle, byteLimit)

    actual fun pause(): Int = native.kmo_sync_pause(handle)

    actual fun resume(): Int = native.kmo_sync_resume(handle)

    actual fun lastError(): String {
        val pointer = native.kmo_sync_last_error(handle) ?: return ""
        return try {
            pointer.getString(0)
        } finally {
            native.kmo_sync_free_string(pointer)
        }
    }

    actual override fun close() {
        if (closed.compareAndSet(false, true)) {
            native.kmo_sync_destroy(handle)
        }
    }

    private fun interface NativeCallback : Callback {
        fun invoke(eventType: Int, jsonData: Pointer?, userData: Pointer?)
    }

    private interface KmoSyncNative : Library {
        fun kmo_sync_create(
            storageConfigJson: String,
            encryptionConfigJson: String,
            deviceId: String,
            localCacheDir: String,
            callback: NativeCallback?,
            userData: Pointer?,
        ): Pointer?

        fun kmo_sync_destroy(sync: Pointer?)
        fun kmo_sync_all(sync: Pointer?, mode: Int): Int
        fun kmo_sync_single_meta(sync: Pointer?, bookHash: String, metaId: String): Int
        fun kmo_sync_book(sync: Pointer?, bookHash: String): Int
        fun kmo_sync_put_local_book(sync: Pointer?, bookHash: String, localFilePath: String): Int
        fun kmo_sync_put_local_meta_json(sync: Pointer?, metaJson: String): Int
        fun kmo_sync_resolve_meta_file_conflict(
            sync: Pointer?,
            metaId: String,
            chosenVersion: String,
        ): Int
        fun kmo_sync_resolve_blob_conflict(
            sync: Pointer?,
            bookHash: String,
            chosenVersion: String,
        ): Int
        fun kmo_sync_rotate_envelope_kek(
            sync: Pointer?,
            newEncryptionConfigJson: String,
        ): Int
        fun kmo_sync_mark_meta_item_deleted(
            sync: Pointer?,
            metaId: String,
            itemType: String,
            itemUuid: String,
        ): Int
        fun kmo_sync_undo_deletion(sync: Pointer?, metaId: String, itemUuid: String): Int
        fun kmo_sync_resolve_tombstone_revival(
            sync: Pointer?,
            metaId: String,
            itemUuid: String,
            resolution: String,
        ): Int
        fun kmo_sync_set_network_type(sync: Pointer?, networkType: Int): Int
        fun kmo_sync_set_blob_byte_limit(sync: Pointer?, byteLimit: Long): Int
        fun kmo_sync_pause(sync: Pointer?): Int
        fun kmo_sync_resume(sync: Pointer?): Int
        fun kmo_sync_get_local_meta(sync: Pointer?, metaId: String): Pointer?
        fun kmo_sync_get_sync_state(sync: Pointer?): Pointer?
        fun kmo_sync_last_error(sync: Pointer?): Pointer?
        fun kmo_sync_free_string(value: Pointer?)
    }

    private companion object {
        val native: KmoSyncNative by lazy {
            val nativeDir = System.getenv("KMO_SYNC_NATIVE_LIB_DIR")
                ?: System.getProperty("kmo.sync.native.lib.dir")
            if (!nativeDir.isNullOrBlank()) {
                val property = "jna.library.path"
                val current = System.getProperty(property).orEmpty()
                val separator = System.getProperty("path.separator")
                System.setProperty(
                    property,
                    if (current.isBlank()) nativeDir else "$nativeDir$separator$current",
                )
                Native.load("kmo_sync", KmoSyncNative::class.java)
            } else {
                Native.load(extractBundledLibrary(), KmoSyncNative::class.java)
            }
        }

        private fun extractBundledLibrary(): String {
            val osName = System.getProperty("os.name").lowercase(Locale.ROOT)
            val os = when {
                osName.contains("mac") -> "macos"
                osName.contains("win") -> "windows"
                else -> "linux"
            }
            val archName = System.getProperty("os.arch").lowercase(Locale.ROOT)
            val arch = if (archName == "aarch64" || archName == "arm64") "aarch64" else "x86_64"
            val libraryName = when (os) {
                "macos" -> "libkmo_sync.dylib"
                "windows" -> "kmo_sync.dll"
                else -> "libkmo_sync.so"
            }
            val resourcePath = "native/$os-$arch/$libraryName"
            val input = NativeKmoSyncBridge::class.java.classLoader
                .getResourceAsStream(resourcePath)
                ?: error("Bundled native library is missing: $resourcePath")
            val directory = Files.createTempDirectory("kmo-sync-native")
            val target = directory.resolve(libraryName)
            input.use { Files.copy(it, target, StandardCopyOption.REPLACE_EXISTING) }
            target.toFile().deleteOnExit()
            directory.toFile().deleteOnExit()
            return target.toAbsolutePath().toString()
        }
    }
}
