package guanzhi.syncdemo

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive

/**
 * 观止 (guanzhi) 阅读数据 <-> Book Sync meta JSON 的映射层。
 *
 * 真实接入时这段代码可以直接放进 guanzhi 的 commonMain，把
 * [LocalBook] / [BookBookmark] 序列化成 Book Sync 的 meta JSON，
 * 同步完成后用 [parseSyncedState] 把拉取结果写回观止自己的仓库。
 *
 * 说明：
 * - `book_hash` 用图书 id 充当（真实应用应使用书籍文件内容的 blake3 hex，
 *   见 Book Sync README；meta 同步把该值当不透明标识符处理）。
 * - 观止模型没有书签创建时间，映射时用同步的 logical_ts 作为 create_ts，
 *   保证"同 ID 书签按创建时间取大者"的合并规则符合直觉。
 */
object GuanzhiSyncMapper {

    /** 演示用的章节总数，用于把 chapterIndex + 章内进度折算成 0..1 总进度。 */
    var totalChapters: Int = 10

    private const val ANNOTATION_PREFIX = "gzh://"

    fun bookHashFor(book: LocalBook): String = book.id

    fun progressToJson(book: LocalBook): JsonObject {
        val p = book.progress
        val percent =
            ((p.chapterIndex + p.chapterProgress.toDouble()) / totalChapters)
                .coerceIn(0.0, 1.0)
        return buildJsonObject {
            put(
                "cfi_position",
                JsonPrimitive(
                    "$ANNOTATION_PREFIX${book.id}?chapter=${p.chapterIndex}&page=${p.pageIndex}&offset=${p.characterOffset}",
                ),
            )
            put("progress_percent", JsonPrimitive(percent))
            put("chapter_id", JsonPrimitive(p.chapterIndex.toString()))
        }
    }

    fun bookmarksToJson(book: LocalBook, bookmarks: List<BookBookmark>, ts: Long): JsonArray =
        buildJsonArray {
            for (bm in bookmarks) {
                add(
                    buildJsonObject {
                        put("bookmark_id", JsonPrimitive(bm.id))
                        put(
                            "cfi_range",
                            JsonPrimitive(
                                "$ANNOTATION_PREFIX${book.id}?chapter=${bm.chapterIndex}&page=${bm.pageIndex}&offset=${bm.characterOffset}",
                            ),
                        )
                        put("title", JsonPrimitive(bm.title))
                        put("create_ts", JsonPrimitive(ts))
                    },
                )
            }
        }

    /**
     * 把观止的图书状态序列化为 Book Sync 的 meta JSON。
     * `logicalTs` 应随每次本地编辑单调递增（观止可用自己仓库里的全局版本号）。
     */
    fun buildMetaJson(
        book: LocalBook,
        bookmarks: List<BookBookmark>,
        highlights: List<DemoHighlight>,
        notes: List<DemoNote>,
        deviceId: String,
        logicalTs: Long,
    ): String {
        val meta = buildJsonObject {
            put("meta_id", JsonPrimitive(book.id))
            put("book_hash", JsonPrimitive(bookHashFor(book)))
            put("modified_ts", JsonPrimitive(logicalTs))
            put("device_id", JsonPrimitive(deviceId))
            put("progress", progressToJson(book))
            put("bookmarks", bookmarksToJson(book, bookmarks, logicalTs))
            put("highlights", highlightsToJson(book, highlights, logicalTs))
            put("notes", notesToJson(book, notes, logicalTs))
            put("wall_clock_ts", JsonPrimitive(logicalTs))
            put("logical_ts", JsonPrimitive(logicalTs))
            put("origin_device_id", JsonPrimitive(deviceId))
            put("edit_history", buildJsonArray {})
            put("progress_write_ts", JsonPrimitive(logicalTs))
            put("progress_writer_device", JsonPrimitive(deviceId))
            put("bookmarks_write_ts", JsonPrimitive(logicalTs))
            put("bookmarks_writer_device", JsonPrimitive(deviceId))
        }
        return Json.encodeToString(JsonObject.serializer(), meta)
    }

    private fun highlightsToJson(
        book: LocalBook,
        highlights: List<DemoHighlight>,
        ts: Long,
    ): JsonArray = buildJsonArray {
        for (h in highlights) {
            add(
                buildJsonObject {
                    put("highlight_id", JsonPrimitive(h.id))
                    put("cfi_start", JsonPrimitive("$ANNOTATION_PREFIX${book.id}?chapter=${h.chapterIndex}&offset=${h.startOffset}"))
                    put("cfi_end", JsonPrimitive("$ANNOTATION_PREFIX${book.id}?chapter=${h.chapterIndex}&offset=${h.endOffset}"))
                    put("color", JsonPrimitive(h.color))
                    put("comment", JsonPrimitive(h.comment))
                    put("create_ts", JsonPrimitive(ts))
                },
            )
        }
    }

    private fun notesToJson(book: LocalBook, notes: List<DemoNote>, ts: Long): JsonArray =
        buildJsonArray {
            for (n in notes) {
                add(
                    buildJsonObject {
                        put("note_id", JsonPrimitive(n.id))
                        put("relate_cfi", JsonPrimitive("$ANNOTATION_PREFIX${book.id}?chapter=${n.chapterIndex}&offset=${n.offset}"))
                        put("content", JsonPrimitive(n.content))
                        put("create_ts", JsonPrimitive(ts))
                    },
                )
            }
        }

    /** 把拉取到的 meta JSON 还原成可写回观止仓库的状态。 */
    fun parseSyncedState(metaJson: String): SyncedState {
        val root = Json.parseToJsonElement(metaJson).jsonObject
        val progress = root["progress"]?.jsonObject
        val bookmarks = root["bookmarks"]?.jsonArray?.mapNotNull { bm ->
            val o = bm.jsonObject
            val cfi = o["cfi_range"]?.jsonPrimitive?.contentOrNull ?: ""
            SyncedBookmark(
                id = o["bookmark_id"]?.jsonPrimitive?.contentOrNull ?: "",
                title = o["title"]?.jsonPrimitive?.contentOrNull ?: "",
                chapter = cfi.chapterOf() ?: 0,
                page = cfi.pageOf() ?: 0,
                createTs = o["create_ts"]?.jsonPrimitive?.longOrNull() ?: 0L,
            )
        } ?: emptyList()
        val highlights = root["highlights"]?.jsonArray?.mapNotNull { h ->
            val o = h.jsonObject
            val cfi = o["cfi_start"]?.jsonPrimitive?.contentOrNull ?: ""
            SyncedHighlight(
                id = o["highlight_id"]?.jsonPrimitive?.contentOrNull ?: "",
                color = o["color"]?.jsonPrimitive?.contentOrNull ?: "",
                comment = o["comment"]?.jsonPrimitive?.contentOrNull ?: "",
                chapter = cfi.chapterOf() ?: 0,
                createTs = o["create_ts"]?.jsonPrimitive?.longOrNull() ?: 0L,
            )
        } ?: emptyList()
        val notes = root["notes"]?.jsonArray?.mapNotNull { n ->
            val o = n.jsonObject
            val cfi = o["relate_cfi"]?.jsonPrimitive?.contentOrNull ?: ""
            SyncedNote(
                id = o["note_id"]?.jsonPrimitive?.contentOrNull ?: "",
                content = o["content"]?.jsonPrimitive?.contentOrNull ?: "",
                chapter = cfi.chapterOf() ?: 0,
                createTs = o["create_ts"]?.jsonPrimitive?.longOrNull() ?: 0L,
            )
        } ?: emptyList()
        return SyncedState(
            progressPercent = progress?.get("progress_percent")?.jsonPrimitive?.doubleOrNull,
            chapterId = progress?.get("chapter_id")?.jsonPrimitive?.contentOrNull,
            bookmarks = bookmarks,
            highlights = highlights,
            notes = notes,
        )
    }

    private fun String.chapterOf(): Int? {
        val param = substringAfter("?chapter=", missingDelimiterValue = "")
        return param.substringBefore('&').toIntOrNull()
    }

    private fun String.pageOf(): Int? {
        val param = substringAfter("?page=", missingDelimiterValue = "")
        return param.substringBefore('&').toIntOrNull()
    }
}

/** 演示用的划线数据（观止当前模型尚未包含，后续版本可扩展）。 */
data class DemoHighlight(
    val id: String,
    val chapterIndex: Int,
    val startOffset: Int,
    val endOffset: Int,
    val color: String = "yellow",
    val comment: String = "",
)

/** 演示用的笔记数据。 */
data class DemoNote(
    val id: String,
    val chapterIndex: Int,
    val offset: Int,
    val content: String,
)

/** 同步后从远端还原出的阅读状态。 */
data class SyncedState(
    val progressPercent: Double?,
    val chapterId: String?,
    val bookmarks: List<SyncedBookmark>,
    val highlights: List<SyncedHighlight>,
    val notes: List<SyncedNote>,
) {
    override fun toString(): String {
        val sb = StringBuilder()
        sb.append("progress=$progressPercent% (chapter=$chapterId)")
        sb.append(", bookmarks=[${bookmarks.joinToString { "${it.title}@${it.chapter}" }}]")
        sb.append(", highlights=[${highlights.joinToString { "${it.color}:${it.comment}" }}]")
        sb.append(", notes=[${notes.joinToString { it.content }}]")
        return sb.toString()
    }
}

data class SyncedBookmark(
    val id: String,
    val title: String,
    val chapter: Int,
    val page: Int,
    val createTs: Long,
)

data class SyncedHighlight(
    val id: String,
    val color: String,
    val comment: String,
    val chapter: Int,
    val createTs: Long,
)

data class SyncedNote(
    val id: String,
    val content: String,
    val chapter: Int,
    val createTs: Long,
)

private fun kotlinx.serialization.json.JsonPrimitive.longOrNull(): Long? =
    contentOrNull?.toLongOrNull()
