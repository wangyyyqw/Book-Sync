# Reeden 风格远程协议

> 把 kmo-sync 的远程协议从 `yuewei/books/<hash>/{blobs,metas,tombstones}` 改成
> Reeden R2 bucket 风格的扁平 `kmo-sync/{books,book_progress,bookmarks}/...` 布局，
> 用 last-write-wins（LWW）取代显式冲突 / tombstone 同步，把每次 sync 的 A 类操作
> 从 N+2（书分块）压到 1。

## 目标

* **降低 R2 Class A 操作的次数**，让多设备、多本书的同步可以稳定跑在 R2 free tier
  （每月 100 万次 Class A）之下。
* **去掉冗余的协议文件**：`_sync_header.json`、FastCDC manifest + chunk index、
  per-book tombstone JSON。
* **保留 LWW 语义**：进度与书签都用 `last_write_ts`（同分时按 `device_id` 字典序）
  做决断，per-item 用 `create_ts` 取大者。
* **不再迁移旧数据**：用户重新 sync 一次即可，旧的 `yuewei/books/<hash>/...` 路径
  既不被读也不被写。

## 远程布局

```
kmo-sync/books/<hash>             # 整本书的密文 / 明文，单对象 PUT
kmo-sync/book_progress/<hash>.json # 进度 JSON envelope（明文 / .enc / .env）
kmo-sync/bookmarks/<hash>.json     # 书签 / 高亮 / 笔记 envelope
kmo-sync/metadata_backups/<ts>_<dev>.zip  # 可选，本地导出，调用方决定何时上传
```

每个 envelope 都是一份独立的 `serde_json` 文档：

```json
// book_progress/<hash>.json
{
  "schema_version": 7,
  "book_hash": "…",
  "progress": { "cfi_position": "epubcfi(/6/2)", "progress_percent": 0.42, "chapter_id": "chapter-1" },
  "last_writer_device_id": "device-a",
  "last_write_ts": 1783260848775
}

// bookmarks/<hash>.json
{
  "schema_version": 7,
  "book_hash": "…",
  "bookmarks": [ { "bookmark_id": "…", "create_ts": 1783000000, … } ],
  "highlights": [ … ],
  "notes": [ … ],
  "last_writer_device_id": "device-a",
  "last_write_ts": 1783260848775
}
```

加密 extension 仍由 `crypto().remote_extension(...)` 注入（明文为 `"json"`，
age 为 `".enc"`，envelope 为 `".env"`），最终路径形如
`book_progress/<hash>.json.enc`。

## A 类操作对比

| 场景 | 旧协议 | 新协议 |
|---|---|---|
| 进度 0.42 → 0.43 | 1 HEAD + 1 PUT header + 1 PUT meta = **3** | **1** PUT `book_progress/<hash>.json` |
| 新增书签 | 1 HEAD + 1 PUT header + 1 PUT meta = **3** | **1** PUT `bookmarks/<hash>.json` |
| 首次上传 30 MiB 书 | 1 PUT manifest + N PUT chunks + 1 PUT header ≥ **N+2** | **1** PUT `books/<hash>` |
| 拉取 1 本书 | 1 LIST + 1 GET manifest + N GET chunks = **N+2** | **1** GET `books/<hash>` |

按 10 本书 × 3 设备 × 30 次 / 天的估算：旧协议约 90 万次 / 月，会撞 R2 free tier
上限；新协议约 27 万次 / 月，留出余量。

## LWW 规则

`merge_meta_with_remote` 内的逻辑按下面的顺序判断：

1. **进度 envelope**：remote 的 `last_write_ts` 更大（或相等时
   `last_writer_device_id` 字典序更大）→ 直接采纳 remote 的 progress，并把
   `wall_clock_ts` / `device_id` 同步到 remote。否则保留 local。
2. **bookmarks envelope**：
   * 远端 writer 严格更新 → base.bookmarks/highlights/notes = remote 快照；
     再用本地独有的 items（remote 没观察到的）做一次 union（id 取并集、
     `create_ts` 取大），防止远端丢本地尚未 push 的项。
   * 同 writer（`wall_clock_ts` 与 `device_id` 都相同）→ 单纯 union（id + create_ts max）。
   * 本地严格更新 → 不引入远端条目，避免把 tombstone 之类已经删除的项带回。

> 注意：reeden 的"tombstone 仅本地"语义意味着 `mark_meta_item_deleted` 在 A 上
> 删除高亮后，B 的本地 cache 仍保留该高亮直到 A 把 `highlights: []` push 到
> remote 并被 B 拉取。因为远端的空 list 在 LWW 中是"权威"，B 端的本地条目
> 会被覆盖。

## 移除项

* `_sync_header.json`、`SyncHeader`、`SyncFeatures`、`ensure_protocol_compatible`、
  `write_sync_header`、`CURRENT_PROTOCOL_VERSION`、`MIN_COMPATIBLE_PROTOCOL_VERSION`。
* `BLOB_CAS_THRESHOLD_BYTES`、`FASTCDC_MIN_SIZE/AVG_SIZE/MAX_SIZE`、
  `write_remote_book_manifest`、`read_remote_book_from_manifest`、`upload_large`、
  `cache_merkle_nodes`。
* `tombstones_for_mode`、`sync_tombstones_inner`、`write_remote_tombstones`、
  `read_remote_tombstones`、`apply_tombstones_to_meta`、`tombstone_revival_conflict`、
  `remove_meta_item`（来自 tombstone 路径的那份，`meta.rs` 里的同名实现保留）、
  `record_tombstone_revival_conflict`。
* `sync_meta_history_archive_inner`、`write_remote_history_archive`、
  `read_remote_history_archive`、`remote_history_path`。
* 公开 API 中 `mark_meta_item_deleted`、`undo_deletion`、`resolve_tombstone_revival`
  现在只动本地 tombstone 表；不再产生远程 R2 调用。
* `record_meta_conflict` 在 LWW 下不再被调用（`conflict_count()` 仍然存在以保证
  FFI 表面不变）。

## 新增项

* `REMOTE_PROTOCOL_VERSION = 7`，挂在每个 envelope 的 `schema_version` 字段上。
* `RemoteProgress` / `RemoteBookmarks` 两个 serde 结构体。
* `remote_book_path` / `remote_progress_path` / `remote_bookmarks_path` 三个路径助手。
* `write_remote_progress` / `read_remote_progress` 与
  `write_remote_bookmarks` / `read_remote_bookmarks` 四个 I/O 函数。
* `discover_meta_pairs` / `discover_book_hashes` 用扁平前缀重组：
  本地仍然从 `local_cache_dir/metas/` 与 `blob_index` 读；remote 从
  `book_progress/`、`bookmarks/`、`books/` 三个前缀派生。
* `KmoSyncFacade::write_local_tombstones_for_test` 测试 helper。

## 测试覆盖

`facade.rs` 末尾新增的 8 个测试覆盖了新协议的关键不变量：

* `reeden_layout_no_sync_header_is_ever_written`
* `reeden_layout_no_protocol_version_check_is_performed`
* `reeden_layout_sync_progress_writes_only_one_object`
* `reeden_layout_sync_bookmarks_merges_by_id_and_takes_largest_create_ts`
* `reeden_layout_book_upload_writes_single_object_no_manifest`
* `reeden_layout_pull_only_discovers_single_book_object`
* `tombstone_delete_is_local_only_in_reeden_layout`
* `tombstone_revival_is_resolved_by_last_write_wins`
* `resolve_tombstone_revival_is_local_only_in_reeden_layout`
* `concurrent_meta_update_resolves_via_last_write_wins_without_recording_conflict`
* `resolve_meta_conflict_is_a_noop_under_last_write_wins`

`tests/cross_device_simulation.rs` 与 `tests/ffi_meta_e2e.rs` 也被改造为新布局的
fixture（`book_hash == meta_id`），保持 e2e 路径有覆盖。

## 风险与回退

* 没有迁移路径 —— 老 `yuewei/books/<hash>/metas/*` 数据需要用户重新 sync 才能
  被新协议覆盖。建议在升级说明里写明这一点。
* `metadata_backups/<ts>_<dev>.zip` 现在仅本地 stub，是否上 R2 由上层业务决定。
* R2 bucket 中确实存在一个真实的 `reeden` 前缀，那不是 kmo-sync 写入的；
  本次重构不引入混居，两个 bucket 前缀（`kmo-sync/`、`reeden/`）互相隔离。

## 不在本次重构范围

* `storage::upload_large` / `download_large` 仍保留在 trait 上，后续可单独删除。
* WebDAV adapter 没改路径布局（它自带虚拟前缀）。
* `kmo_index.db` 的 schema 没动，本地元数据表 / tombstone 表都保留以兼容 API。