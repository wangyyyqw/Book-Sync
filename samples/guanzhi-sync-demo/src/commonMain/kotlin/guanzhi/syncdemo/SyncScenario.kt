package guanzhi.syncdemo

import com.kmosync.KmoSync
import com.kmosync.SyncMode
import com.kmosync.SyncResult
import com.kmosync.TombstoneItemType

/**
 * 双端同步场景引擎（common 代码，JVM 命令行与 Android APK 共用）。
 *
 * 用两台设备（手机 phone-1 / 平板 pad-1）驱动 Book Sync，覆盖：
 * 进度 LWW、书签 union 合并、划线 / 笔记传播、删除 tombstone、
 * 撤销删除 revival。每个断言通过 [check] 回调上报（JVM 打印 PASS/FAIL，
 * Android 追加到界面日志）。
 */
class SyncScenario(private val check: (name: String, ok: Boolean) -> Unit) {

    suspend fun run(phone: KmoSync, pad: KmoSync, deviceIds: (KmoSync) -> String) {
        // ---------------- 阶段 1：手机读到 30% + 书签，同步 ----------------
        val sanTi = LocalBook(
            id = "book-santi",
            title = "三体",
            author = "刘慈欣",
            progress = ReadingProgress(chapterIndex = 3, pageIndex = 2, chapterProgress = 0.0f),
        )
        val phoneBookmark = BookBookmark(
            id = "bm-phone-1",
            bookId = sanTi.id,
            chapterIndex = 1,
            pageIndex = 0,
            title = "三体·序章",
        )
        check(
            "手机: 写入本地进度 30% + 1 书签",
            putMeta(phone, deviceIds(phone), sanTi, listOf(phoneBookmark), emptyList(), emptyList(), ts = 100),
        )
        check("手机: syncAll 上传", syncAllOk(phone, "手机"))

        // ---------------- 阶段 2：平板拉取，读到 65% + 书签/划线/笔记，同步 ----------------
        check("平板: syncAll 拉取", syncAllOk(pad, "平板"))
        val padState = localState(pad, sanTi.id)
        check("平板: 拉到手机进度 30%", padState.progressPercent == 0.3)
        check("平板: 拉到手机书签", padState.bookmarks.any { it.id == "bm-phone-1" })

        val sanTiPad = LocalBook(
            id = sanTi.id,
            title = sanTi.title,
            author = sanTi.author,
            progress = ReadingProgress(chapterIndex = 6, pageIndex = 5, chapterProgress = 0.5f),
        )
        val padBookmark = BookBookmark(
            id = "bm-pad-1",
            bookId = sanTi.id,
            chapterIndex = 6,
            pageIndex = 5,
            title = "三体·黑暗森林",
        )
        val padHighlight = DemoHighlight(
            id = "hl-pad-1",
            chapterIndex = 6,
            startOffset = 100,
            endOffset = 140,
            color = "green",
            comment = "黑暗森林法则",
        )
        val padNote = DemoNote(
            id = "note-pad-1",
            chapterIndex = 6,
            offset = 120,
            content = "不要回答！",
        )
        check(
            "平板: 写入本地进度 65% + 书签/划线/笔记",
            putMeta(pad, deviceIds(pad), sanTiPad, listOf(padBookmark), listOf(padHighlight), listOf(padNote), ts = 200),
        )
        check("平板: syncAll 上传", syncAllOk(pad, "平板"))

        // ---------------- 阶段 3：手机拉取，验证 union 合并 ----------------
        check("手机: syncAll 拉取", syncAllOk(phone, "手机"))
        val phoneAfterPull = localState(phone, sanTi.id)
        check("手机: 拉到平板进度 65%", phoneAfterPull.progressPercent == 0.65)
        check(
            "手机: 书签 union（双方各 1 条 = 2 条）",
            phoneAfterPull.bookmarks.map { it.id }.toSet() == setOf("bm-phone-1", "bm-pad-1"),
        )
        check("手机: 拉到划线", phoneAfterPull.highlights.any { it.id == "hl-pad-1" })
        check("手机: 拉到笔记", phoneAfterPull.notes.any { it.id == "note-pad-1" })
        check("手机: 无未解决冲突", noConflicts(phone, "手机"))

        // ---------------- 阶段 4：手机删除平板的书签，tombstone 传播 ----------------
        check("手机: 删除平板书签 bm-pad-1", deleteOk(phone, sanTi.id, "bm-pad-1"))
        check("手机: syncAll 上传删除", syncAllOk(phone, "手机"))
        check("平板: syncAll 拉取删除", syncAllOk(pad, "平板"))
        val padAfterDelete = localState(pad, sanTi.id)
        check(
            "平板: bm-pad-1 已被删除（tombstone 生效）",
            padAfterDelete.bookmarks.map { it.id } == listOf("bm-phone-1"),
        )

        // ---------------- 阶段 5：手机撤销删除，revival 跨设备复活 ----------------
        check("手机: 撤销删除 bm-pad-1", undoOk(phone, sanTi.id, "bm-pad-1"))
        check("手机: syncAll 上传复活", syncAllOk(phone, "手机"))
        check("平板: syncAll 拉取复活", syncAllOk(pad, "平板"))
        val padAfterRevive = localState(pad, sanTi.id)
        check(
            "平板: bm-pad-1 复活",
            padAfterRevive.bookmarks.map { it.id }.toSet() == setOf("bm-phone-1", "bm-pad-1"),
        )
        check("平板: 无未解决冲突", noConflicts(pad, "平板"))
    }

    private suspend fun putMeta(
        sync: KmoSync,
        deviceId: String,
        book: LocalBook,
        bookmarks: List<BookBookmark>,
        highlights: List<DemoHighlight>,
        notes: List<DemoNote>,
        ts: Long,
    ): Boolean = when (val result = sync.putLocalMetaJson(
        GuanzhiSyncMapper.buildMetaJson(
            book = book,
            bookmarks = bookmarks,
            highlights = highlights,
            notes = notes,
            deviceId = deviceId,
            logicalTs = ts,
        ),
    )) {
        is SyncResult.Success -> true
        is SyncResult.Failure -> {
            check("  putLocalMetaJson: code=${result.code} ${result.message}", false)
            false
        }
    }

    private suspend fun syncAllOk(sync: KmoSync, who: String): Boolean {
        val result = sync.syncAll(SyncMode.Bidirectional)
        if (result is SyncResult.Failure) {
            check("  [$who] syncAll: code=${result.code} ${result.message}", false)
        }
        return result is SyncResult.Success
    }

    private suspend fun deleteOk(sync: KmoSync, metaId: String, itemUuid: String): Boolean {
        val result = sync.markMetaItemDeleted(metaId, TombstoneItemType.Bookmark, itemUuid)
        if (result is SyncResult.Failure) {
            check("  markMetaItemDeleted: code=${result.code} ${result.message}", false)
        }
        return result is SyncResult.Success
    }

    private suspend fun undoOk(sync: KmoSync, metaId: String, itemUuid: String): Boolean {
        val result = sync.undoDeletion(metaId, itemUuid)
        if (result is SyncResult.Failure) {
            check("  undoDeletion: code=${result.code} ${result.message}", false)
        }
        return result is SyncResult.Success
    }

    private fun localState(sync: KmoSync, metaId: String): SyncedState =
        sync.getLocalMeta(metaId)?.let(GuanzhiSyncMapper::parseSyncedState)
            ?: SyncedState(null, null, emptyList(), emptyList(), emptyList())

    private fun noConflicts(sync: KmoSync, who: String): Boolean {
        val state = sync.getSyncState() ?: return false
        val conflictCount = Regex("\"conflict_count\":(\\d+)").find(state)?.groupValues?.get(1)?.toInt()
        check("  [$who] syncState: ${state.take(160)}", true)
        return conflictCount == 0
    }
}
