package com.kmosync.sample

import android.app.Activity
import android.os.Bundle
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import com.kmosync.KmoSync
import com.kmosync.KmoSyncConfig
import com.kmosync.SyncMode
import com.kmosync.SyncResult
import com.kmosync.TombstoneRevivalResolution
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

class MainActivity : Activity() {
    private lateinit var status: TextView

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        status = TextView(this).apply {
            textSize = 16f
            text = "Book Sync Android sample ready"
        }
        val push = Button(this).apply {
            text = "Push Android Meta"
            setOnClickListener {
                runAction { pushAndroidMeta() }
            }
        }
        val pull = Button(this).apply {
            text = "Pull Shared Meta"
            setOnClickListener {
                runAction { pullSharedMeta() }
            }
        }
        val state = Button(this).apply {
            text = "Show Sync State"
            setOnClickListener {
                runAction { showSyncState() }
            }
        }
        val resolve = Button(this).apply {
            text = "Resolve First Conflict"
            setOnClickListener {
                runAction { resolveFirstConflict() }
            }
        }
        val layout = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(48, 64, 48, 48)
            addView(status)
            addView(push)
            addView(pull)
            addView(state)
            addView(resolve)
        }
        setContentView(layout)
    }

    private fun runAction(action: suspend () -> String) {
        status.text = "Running..."
        CoroutineScope(Dispatchers.IO).launch {
            val result = action()
            withContext(Dispatchers.Main) {
                status.text = result
            }
        }
    }

    private suspend fun pushAndroidMeta(): String =
        createSync().use { sync ->
            val put = sync.putLocalMetaJson(sampleMetaJson("shared-meta", 0.42, 10))
            if (put is SyncResult.Failure) {
                return@use "Put failed: ${put.code} ${put.message}"
            }
            when (val result = sync.syncAll(SyncMode.PushOnly)) {
                SyncResult.Success -> "Android pushed shared-meta progress 0.42"
                is SyncResult.Failure -> "Push failed: ${result.code} ${result.message}"
            }
        }

    private suspend fun pullSharedMeta(): String =
        createSync().use { sync ->
            when (val result = sync.syncAll(SyncMode.PullOnly)) {
                SyncResult.Success -> {
                    val meta = sync.getLocalMeta("shared-meta") ?: "null"
                    "Pulled shared-meta:\n$meta"
                }
                is SyncResult.Failure -> "Pull failed: ${result.code} ${result.message}"
            }
        }

    private suspend fun showSyncState(): String =
        createSync().use { sync ->
            sync.getSyncState() ?: "Sync state unavailable"
        }

    private suspend fun resolveFirstConflict(): String =
        createSync().use { sync ->
            val state = sync.getSyncState() ?: return@use "Sync state unavailable"
            val conflicts = JSONObject(state).optJSONArray("conflicts")
            if (conflicts == null || conflicts.length() == 0) {
                return@use "No pending conflicts"
            }
            val conflict = conflicts.getJSONObject(0)
            when (conflict.optString("conflict_kind")) {
                "meta_file" -> {
                    val metaId = conflict.getString("meta_id")
                    when (val result = sync.resolveMetaConflict(metaId, "remote")) {
                        SyncResult.Success -> "Resolved meta conflict for $metaId with remote"
                        is SyncResult.Failure -> "Resolve failed: ${result.code} ${result.message}"
                    }
                }
                "tombstone_revival" -> {
                    val metaId = conflict.getString("meta_id")
                    val itemUuid = conflict.getString("item_uuid")
                    when (val result = sync.resolveTombstoneRevival(
                        metaId,
                        itemUuid,
                        TombstoneRevivalResolution.Restore,
                    )) {
                        SyncResult.Success -> "Restored tombstone conflict $itemUuid"
                        is SyncResult.Failure -> "Resolve failed: ${result.code} ${result.message}"
                    }
                }
                "blob_file" -> {
                    val bookHash = conflict.getString("book_hash")
                    when (val result = sync.resolveBlobConflict(bookHash, "remote")) {
                        SyncResult.Success -> "Resolved blob conflict for $bookHash with remote"
                        is SyncResult.Failure -> "Resolve failed: ${result.code} ${result.message}"
                    }
                }
                else -> "Unsupported conflict: ${conflict.optString("conflict_kind")}"
            }
        }

    private fun createSync(): KmoSync =
        KmoSync(
            KmoSyncConfig(
                storageConfigJson = storageConfigJson(),
                encryptionConfigJson = """{"type":"none"}""",
                deviceId = "android-sample",
                localCacheDir = filesDir.resolve("kmo-sync").absolutePath,
            ),
        )

    private fun storageConfigJson(): String =
        runCatching {
            assets.open("kmo_sync_sample_config.json").bufferedReader().use { it.readText() }
        }.getOrDefault("""{"type":"memory"}""")

    private fun sampleMetaJson(metaId: String, progress: Double, logicalTs: Long): String =
        """
        {
          "meta_id":"$metaId",
          "book_hash":"shared-sample-book",
          "modified_ts":$logicalTs,
          "device_id":"android-sample",
          "progress":{
            "cfi_position":"epubcfi(/6/2)",
            "progress_percent":$progress,
            "chapter_id":"chapter-1"
          },
          "bookmarks":[],
          "highlights":[],
          "notes":[],
          "wall_clock_ts":$logicalTs,
          "logical_ts":$logicalTs,
          "origin_device_id":"android-sample",
          "edit_history":[]
        }
        """.trimIndent()
}
