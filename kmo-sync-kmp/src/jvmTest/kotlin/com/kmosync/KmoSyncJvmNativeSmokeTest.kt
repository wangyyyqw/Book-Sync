package com.kmosync

import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.flow.first
import java.nio.file.Files
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs

class KmoSyncJvmNativeSmokeTest {
    @Test
    fun loadsBundledRustLibraryAndRunsSyncAll() {
        val cacheDir = Files.createTempDirectory("kmo-sync-kmp-jvm").toString()
        val sync = KmoSync(
            KmoSyncConfig(
                storageConfigJson = """{"type":"memory"}""",
                encryptionConfigJson = """{"type":"none"}""",
                deviceId = "kmp-jvm",
                localCacheDir = cacheDir,
            ),
        )
        sync.use {
            assertEquals(SyncResult.Success, runBlocking { sync.syncAll(SyncMode.Bidirectional) })
            // Emitted during construction, before the first subscriber: it must remain queued.
            assertEquals(SyncEventType.SecurityWarning, runBlocking { sync.events.first() }.type)
            assertEquals(SyncResult.Success, sync.setNetworkType(NetworkType.Cellular))
            assertEquals(SyncResult.Success, runBlocking { sync.syncAll(SyncMode.Bidirectional) })
            assertEquals(SyncResult.Success, sync.setNetworkType(NetworkType.Wifi))
        }
        val closedResult = runBlocking { sync.syncAll(SyncMode.Bidirectional) }
        assertIs<SyncResult.Failure>(closedResult)
        assertEquals(ErrorCode.InvalidArg.code, closedResult.code)
    }
}
