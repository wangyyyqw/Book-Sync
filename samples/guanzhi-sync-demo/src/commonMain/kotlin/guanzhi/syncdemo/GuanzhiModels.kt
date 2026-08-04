package guanzhi.syncdemo

/**
 * 与 `guanzhi/kmpApp` 中 `guanzhi.model.LibraryModels.kt` 保持一致的最小镜像。
 * 观止 (guanzhi) 的图书 / 进度 / 书签模型。真实应用里应直接引用该模型；
 * 此处为独立 JVM demo 复制了与同步相关的字段。
 */
data class LocalBook(
    val id: String,
    val title: String,
    val author: String = "",
    val format: BookFormat = BookFormat.TXT,
    val progress: ReadingProgress = ReadingProgress(),
)

enum class BookFormat { TXT, EPUB, WEB }

data class ReadingProgress(
    val chapterIndex: Int = 0,
    val pageIndex: Int = 0,
    val chapterProgress: Float = 0F,
    val characterOffset: Int = 0,
)

/**
 * 观止的书签。注意观止模型本身没有"创建时间"，Book Sync 合并书签时按
 * `create_ts` 取大者获胜，因此映射层需要用同步时的 logical_ts 充当 create_ts
 * （见 [GuanzhiSyncMapper]）。
 */
data class BookBookmark(
    val id: String,
    val bookId: String,
    val chapterIndex: Int,
    val pageIndex: Int,
    val title: String,
    val summary: String = "",
    val characterOffset: Int = 0,
)
