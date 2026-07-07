package com.kmosync

import kotlinx.coroutines.runBlocking
import java.nio.file.Files
import kotlin.test.Test
import kotlin.test.assertEquals

class KmoSyncJvmNativeSmokeTest {
    @Test
    fun loadsRustLibraryAndRunsSyncAllWhenNativePathIsConfigured() {
        if (System.getenv("KMO_SYNC_NATIVE_LIB_DIR").isNullOrBlank() &&
            System.getProperty("kmo.sync.native.lib.dir").isNullOrBlank()
        ) {
            return
        }

        val cacheDir = Files.createTempDirectory("kmo-sync-kmp-jvm").toString()
        KmoSync(
            KmoSyncConfig(
                storageConfigJson = """{"type":"memory"}""",
                encryptionConfigJson = """{"type":"none"}""",
                deviceId = "kmp-jvm",
                localCacheDir = cacheDir,
            ),
        ).use { sync ->
            assertEquals(SyncResult.Success, runBlocking { sync.syncAll(SyncMode.Bidirectional) })
            assertEquals(SyncResult.Success, sync.setNetworkType(NetworkType.Cellular))
            assertEquals(SyncResult.Success, runBlocking { sync.syncAll(SyncMode.Bidirectional) })
            assertEquals(SyncResult.Success, sync.setNetworkType(NetworkType.Wifi))
        }
    }
}
