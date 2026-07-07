package com.kmosync

internal expect class NativeKmoSyncBridge(
    config: KmoSyncConfig,
    emitEvent: (SyncEvent) -> Unit,
) : AutoCloseable {
    fun syncAll(mode: Int): Int
    fun syncSingleMeta(bookHash: String, metaId: String): Int
    fun syncBook(bookHash: String): Int
    fun putLocalBook(bookHash: String, localFilePath: String): Int
    fun putLocalMetaJson(metaJson: String): Int
    fun resolveMetaConflict(metaId: String, chosenVersion: String): Int
    fun resolveBlobConflict(bookHash: String, chosenVersion: String): Int
    fun rotateEnvelopeKek(newEncryptionConfigJson: String): Int
    fun markMetaItemDeleted(metaId: String, itemType: String, itemUuid: String): Int
    fun undoDeletion(metaId: String, itemUuid: String): Int
    fun resolveTombstoneRevival(metaId: String, itemUuid: String, resolution: String): Int
    fun getLocalMeta(metaId: String): String?
    fun getSyncState(): String?
    fun setNetworkType(networkType: Int): Int
    fun setBlobByteLimit(byteLimit: Long): Int
    fun pause(): Int
    fun resume(): Int
    fun lastError(): String
    override fun close()
}
