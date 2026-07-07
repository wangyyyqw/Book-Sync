package com.kmosync

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.withContext

class KmoSync(config: KmoSyncConfig) : AutoCloseable {
    private val eventFlow = MutableSharedFlow<SyncEvent>(extraBufferCapacity = 64)
    private val native = NativeKmoSyncBridge(config) { event ->
        eventFlow.tryEmit(event)
    }

    val events: Flow<SyncEvent> = eventFlow

    suspend fun syncAll(mode: SyncMode): SyncResult = nativeCall {
        native.syncAll(mode.wireValue)
    }

    suspend fun syncSingleMeta(bookHash: String, metaId: String): SyncResult = nativeCall {
        native.syncSingleMeta(bookHash, metaId)
    }

    suspend fun syncBook(bookHash: String): SyncResult = nativeCall {
        native.syncBook(bookHash)
    }

    suspend fun putLocalBook(bookHash: String, localFilePath: String): SyncResult = nativeCall {
        native.putLocalBook(bookHash, localFilePath)
    }

    suspend fun putLocalMetaJson(metaJson: String): SyncResult = nativeCall {
        native.putLocalMetaJson(metaJson)
    }

    suspend fun resolveMetaConflict(metaId: String, chosenVersion: String): SyncResult = nativeCall {
        native.resolveMetaConflict(metaId, chosenVersion)
    }

    suspend fun resolveBlobConflict(bookHash: String, chosenVersion: String): SyncResult = nativeCall {
        native.resolveBlobConflict(bookHash, chosenVersion)
    }

    suspend fun rotateEnvelopeKek(newEncryptionConfigJson: String): SyncResult = nativeCall {
        native.rotateEnvelopeKek(newEncryptionConfigJson)
    }

    suspend fun markMetaItemDeleted(
        metaId: String,
        itemType: TombstoneItemType,
        itemUuid: String,
    ): SyncResult = nativeCall {
        native.markMetaItemDeleted(metaId, itemType.wireValue, itemUuid)
    }

    suspend fun undoDeletion(metaId: String, itemUuid: String): SyncResult = nativeCall {
        native.undoDeletion(metaId, itemUuid)
    }

    suspend fun resolveTombstoneRevival(
        metaId: String,
        itemUuid: String,
        resolution: TombstoneRevivalResolution,
    ): SyncResult = nativeCall {
        native.resolveTombstoneRevival(metaId, itemUuid, resolution.wireValue)
    }

    fun getLocalMeta(metaId: String): String? = native.getLocalMeta(metaId)

    fun getSyncState(): String? = native.getSyncState()

    fun pause(): SyncResult = resultFromCode(native.pause())

    fun resume(): SyncResult = resultFromCode(native.resume())

    fun setNetworkType(type: NetworkType): SyncResult =
        resultFromCode(native.setNetworkType(type.wireValue))

    fun setBlobByteLimit(byteLimit: Long?): SyncResult =
        resultFromCode(native.setBlobByteLimit(byteLimit ?: -1L))

    override fun close() {
        native.close()
    }

    private suspend fun nativeCall(call: () -> Int): SyncResult =
        withContext(Dispatchers.Default) {
            resultFromCode(call())
        }

    private fun resultFromCode(code: Int): SyncResult =
        if (code == ErrorCode.OK.code) {
            SyncResult.Success
        } else {
            SyncResult.Failure(code, native.lastError())
        }
}

data class KmoSyncConfig(
    val storageConfigJson: String,
    val encryptionConfigJson: String,
    val deviceId: String,
    val localCacheDir: String,
)

class SyncIntervalOption private constructor(
    val label: String,
    val seconds: Long,
) {
    val milliseconds: Long = seconds * 1_000L

    override fun equals(other: Any?): Boolean =
        other is SyncIntervalOption &&
            label == other.label &&
            seconds == other.seconds

    override fun hashCode(): Int =
        31 * label.hashCode() + seconds.hashCode()

    override fun toString(): String = label

    companion object {
        private const val CUSTOM_BASE_SECONDS = 5L

        val TenSeconds = SyncIntervalOption("10 秒", 10L)
        val TwentySeconds = SyncIntervalOption("20 秒", 20L)
        val ThirtySeconds = SyncIntervalOption("30 秒", 30L)
        val OneMinute = SyncIntervalOption("1 分钟", 60L)
        val TwoMinutes = SyncIntervalOption("2 分钟", 120L)
        val ThreeMinutes = SyncIntervalOption("3 分钟", 180L)
        val FiveMinutes = SyncIntervalOption("5 分钟", 300L)

        val presets: List<SyncIntervalOption> = listOf(
            TenSeconds,
            TwentySeconds,
            ThirtySeconds,
            OneMinute,
            TwoMinutes,
            ThreeMinutes,
            FiveMinutes,
        )

        fun custom(multiplier: Long): SyncIntervalOption {
            require(multiplier > 0L) { "custom sync interval multiplier must be positive" }
            val seconds = CUSTOM_BASE_SECONDS * multiplier
            return SyncIntervalOption("自定义 ${seconds} 秒", seconds)
        }
    }
}

enum class SyncMode(val wireValue: Int) {
    Bidirectional(0),
    PushOnly(1),
    PullOnly(2),
}

enum class NetworkType(val wireValue: Int) {
    Wifi(0),
    Cellular(1),
    Unknown(2),
}

enum class TombstoneItemType(val wireValue: String) {
    Bookmark("bookmark"),
    Highlight("highlight"),
    Note("note"),
}

enum class TombstoneRevivalResolution(val wireValue: String) {
    Delete("delete"),
    Restore("restore"),
}

sealed interface SyncResult {
    data object Success : SyncResult
    data class Failure(val code: Int, val message: String) : SyncResult
}

data class SyncEvent(
    val type: SyncEventType,
    val rawType: Int,
    val json: String,
)

enum class SyncEventType(val wireValue: Int) {
    SyncStart(1),
    SyncProgress(2),
    BookChanged(3),
    ConflictFound(4),
    SecurityWarning(5),
    SyncComplete(6),
    Error(7),
    BlobConflict(8),
    DataConflict(9),
    TombstoneRevival(10),
    MergeProgress(11),
    ClockDriftWarning(12),
    Unknown(-1);

    companion object {
        fun fromWireValue(value: Int): SyncEventType =
            entries.firstOrNull { it.wireValue == value } ?: Unknown
    }
}

enum class ErrorCode(val code: Int) {
    OK(0),
    Network(1),
    Storage(2),
    Crypto(3),
    Conflict(4),
    InvalidArg(5),
    Internal(6),
    VersionMismatch(11),
}
