# Book Sync

Book Sync 是一个面向阅读类应用的跨平台同步 SDK。项目核心由 Rust 实现，同时提供 C ABI、JNI/JNA 绑定和 Kotlin Multiplatform 封装，方便 Android、JVM 桌面端和 iOS 应用接入。

它适合用来同步阅读进度、书签、划线、笔记、删除状态和书籍文件，并把数据存放到用户自己的 S3 兼容对象存储或 WebDAV 服务中。

## 能做什么

- 多设备同步阅读元数据。
- 同步书籍文件，支持大文件。
- 通过带 ETag 条件写和重试的 last-write-wins 合并阅读元数据；删除状态通过远端 tombstone 跨设备传播。
- 检测书籍文件冲突，避免直接覆盖远端内容。
- 支持 `age` 加密和 AES-GCM 信封加密。
- 支持 Envelope KEK 轮换：只重包 EDEK，不重新加密正文数据。
- 蜂窝网络下暂停或限制书籍文件同步流量。
- 同一个 Rust 核心可通过 Rust、C ABI、JNI/JNA 和 KMP API 调用。
- 把进度、书签等单类元数据变更压到单对象 PUT，并减少重复 HEAD / 大文件下载校验（详见下文"远端对象布局"）。

## 仓库结构

```text
kmo_sync/                 Rust 核心、C ABI、JNI 导出、native 构建脚本
kmo-sync-kmp/             Kotlin Multiplatform 封装
samples/kmp-reader-sync/  Android 和 iOS 示例应用
scripts/                  发布验证和辅助脚本
kmo_sync/docs/            协议设计文档（REEDEN_REFACTOR 等）
CHANGELOG.md               版本更新与兼容性说明
```

## 构建与验证

需要准备：

- Rust toolchain
- Gradle
- Android SDK/NDK，用于 Android 构建
- Xcode，用于 iOS 构建
- Docker，仅在运行 MinIO/WebDAV 集成测试时需要

运行完整本地发布验收：

```bash
./scripts/verify_release.sh
```

该脚本会执行 Rust 格式检查、clippy、测试、release 构建、C smoke test、KMP JVM 测试、Android native 构建、iOS XCFramework 构建，以及 Android/iOS 示例应用构建。

## Rust 核心使用

构建 Rust native 库：

```bash
cd kmo_sync
cargo build --release
sh scripts/generate_header.sh
```

生成的 C 头文件位于：

```text
kmo_sync/target/include/kmo_sync.h
```

## 存储配置

本地文件存储，适合 smoke test：

```json
{
  "type": "file",
  "root_dir": "/tmp/kmo-sync-remote"
}
```

S3 兼容对象存储：

```json
{
  "type": "s3",
  "endpoint": "https://s3.example.com",
  "bucket": "kmo-sync",
  "access_key": "ACCESS_KEY",
  "secret_key": "SECRET_KEY",
  "region": "us-east-1",
  "root_prefix": "kmo-sync",
  "path_style": true,
  "allow_http": false
}
```

Cloudflare R2 使用同一份 S3 配置，通常写法如下：

```json
{
  "type": "s3",
  "endpoint": "https://<account-id>.r2.cloudflarestorage.com",
  "bucket": "reeden",
  "access_key": "from-env-or-keystore",
  "secret_key": "from-env-or-keystore",
  "region": "auto",
  "root_prefix": "kmo-sync",
  "path_style": true,
  "allow_http": false
}
```

> `root_prefix` 的默认值就是 `kmo-sync`，所有协议层对象都落在该前缀之下；
> 不建议覆盖。如果之前已经用 `yuewei` 部署过老版本，重新 sync 一次即可让数据
> 落到新前缀。

WebDAV：

```json
{
  "type": "webdav",
  "url": "https://dav.example.com/kmo-sync",
  "username": "user",
  "password": "password",
  "root_dir": "kmo-sync"
}
```

## 远端对象布局

Book Sync 把所有同步状态写在远端存储的 `kmo-sync/` 前缀下，便于把不同 reader 的数据隔离到同一个 bucket：

```text
kmo-sync/
  books/<book_hash>                            # 书籍文件，明文 / .enc / .env（流式传输）
  book_progress/<book_hash>.json               # 阅读进度 envelope
  bookmarks/<book_hash>.json                   # 书签 / 划线 / 笔记 envelope
  metadata_backups/<ts>_<device>.zip           # 可选，本地按设备打的 zip 备份
```

`<book_hash>` 使用书籍文件内容的 blake3 小写十六进制哈希。`.env` 后缀表示对象使用了信封加密；`.enc` 表示 age 加密；是否添加后缀由 `crypto().remote_extension(...)` 决定，读者应用无需关心。

进度与书签两个 envelope 的字段：

```json
// book_progress/<hash>.json
{
  "schema_version": 7,
  "book_hash": "<hash>",
  "progress": { "cfi_position": "epubcfi(/6/2)", "progress_percent": 0.42, "chapter_id": "chapter-1" },
  "last_writer_device_id": "android-phone-1",
  "last_write_ts": 1783260848775
}

// bookmarks/<hash>.json
{
  "schema_version": 7,
  "book_hash": "<hash>",
  "bookmarks":  [ { "bookmark_id": "bm-1", "create_ts": 1783000000, ... } ],
  "highlights": [ ... ],
  "notes":      [ ... ],
  "last_writer_device_id": "android-phone-1",
  "last_write_ts": 1783260848775
}
```

合并规则：进度走 `last_write_ts` 的 last-write-wins（相等时按 `device_id` 字典序）；书签 / 划线 / 笔记快照也按写入时间确定同 ID 的胜者，同时保留另一侧独有的 ID。`metadata_backups` 是可选的本地导出，何时上传到 R2 由调用方决定。

所有元数据写入都携带读取时获得的 ETag/版本条件；条件失败时会重新读取、合并并重试，避免两个设备同时同步时由最后一次 PUT 静默覆盖另一台设备。WebDAV 服务必须提供 ETag，否则 SDK 会拒绝不安全的并发写入。

本布局对应线缆协议版本 7。低于 7 的旧版本客户端与本布局不兼容，需要升级到最新版本。完整的协议设计说明见 [`kmo_sync/docs/REEDEN_REFACTOR.md`](kmo_sync/docs/REEDEN_REFACTOR.md)。

### 远端操作优化

| 场景 | 旧协议 | 新协议 |
|---|---|---|
| 修改进度 0.42 → 0.43 | 1 HEAD + 1 PUT header + 1 PUT meta = **3** | **1** PUT `book_progress/<hash>.json` |
| 新增书签 | 1 HEAD + 1 PUT header + 1 PUT meta = **3** | **1** PUT `bookmarks/<hash>.json` |
| 首次上传 30 MiB 书 | 1 PUT manifest + N PUT chunks + 1 PUT header ≥ **N+2** | **1** PUT `books/<hash>` |
| 拉取 1 本书 | 1 LIST + 1 GET manifest + N GET chunks = **N+2** | **1** GET `books/<hash>` |

按 10 本书 × 3 设备 × 30 次 / 天的估算：旧协议约 90 万次 / 月，会撞 R2 free tier
上限（每月 100 万次 Class A）；新协议约 27 万次 / 月，留出余量。

当前实现还做了两类读路径优化：

- 元数据读取使用一次 `GET` 处理存在 / 不存在，不再先 `HEAD` 再 `GET`。
- 书籍文件已同步过且远端 `size` / `etag` 与本地索引匹配时，只做 `stat` 确认，不再下载整本 EPUB 重新 hash。

WebDAV 后端默认启用连接超时、请求超时和 429 / 502 / 503 / 504 临时错误重试；S3 / R2 后端对常见临时对象存储错误也会指数退避重试。

## 加密配置

不加密，仅建议本地测试使用：

```json
{
  "type": "none"
}
```

`age` 加密：

```json
{
  "type": "age",
  "identity": "AGE-SECRET-KEY-..."
}
```

使用用户口令派生 KEK 的信封加密：

```json
{
  "type": "envelope",
  "passphrase": "replace-with-user-secret",
  "kek_id": "primary",
  "kek_version": 1
}
```

如果应用把 KEK 存在系统 Keychain/Keystore 中，也可以传入原始 `kek_hex`。

## KMP 跨平台应用如何接入

KMP 封装是 Android、JVM 桌面端和 Kotlin 共享代码推荐使用的 API。

### 1. 添加模块

如果把本仓库作为源码依赖，在应用的 `settings.gradle.kts` 中加入：

```kotlin
include(":kmo-sync-kmp")
```

然后在 app 模块中添加依赖：

```kotlin
dependencies {
    implementation(project(":kmo-sync-kmp"))
}
```

如果是独立应用，可以复制或发布 `kmo-sync-kmp` 模块，并同时带上下面说明的 native 产物。

### 2. Android 引用 native 库

直接构建 AAR：

```bash
gradle :kmo-sync-kmp:assembleRelease
```

Gradle 会先调用 Rust Android 构建并把三个 ABI 自动装入 AAR：

```text
kmo-sync-kmp/src/androidMain/jniLibs/
├── arm64-v8a/libkmo_sync.so
├── armeabi-v7a/libkmo_sync.so
└── x86_64/libkmo_sync.so
```

Android 应用依赖 `:kmo-sync-kmp` 后，会自动把这些 `.so` 打进 APK/AAB。仓库不提交预构建 `.so`；Gradle 的 Android `preBuild` 会保证 native 库与当前 Rust 源码一致。AAR 同时携带 consumer R8 规则，保留 JNI 入口和事件回调方法名。

### 3. JVM 桌面端引用 native 库

直接构建或测试 JVM 产物：

```bash
gradle :kmo-sync-kmp:jvmTest
```

Gradle 会构建当前宿主平台的 Rust 动态库并装入 JAR，运行时自动解压加载。发布 macOS、Linux 和 Windows 的完整产物时，应在对应系统的 CI runner 分别构建；单个 runner 只生成当前 OS/架构的 native 资源。

开发或诊断时仍可用环境变量 `KMO_SYNC_NATIVE_LIB_DIR`，或 JVM system property 覆盖内置库：

```text
-Dkmo.sync.native.lib.dir=/absolute/path/to/kmo_sync/target/release
```

### 4. iOS 引用 native 库

构建 XCFramework：

```bash
cd kmo_sync
./scripts/build_ios_xcframework.sh
```

产物位置：

```text
kmo_sync/target/apple/KmoSync.xcframework
```

KMP 模块包含 `iosArm64` 和 `iosSimulatorArm64` target；执行 `linkReleaseFrameworkIosArm64` 或 `linkReleaseFrameworkIosSimulatorArm64` 时，Gradle 会自动构建并链接对应 Rust staticlib。纯 Swift 项目也可以把 `KmoSync.xcframework` 加入 app target，并通过下面的头文件接入 C ABI：

```text
kmo_sync/target/include/kmo_sync.h
```

仓库中的 iOS 示例应用已经完成了这个链接方式。

### 5. KMP API 示例

```kotlin
import com.kmosync.KmoSync
import com.kmosync.KmoSyncConfig
import com.kmosync.SyncMode

val sync = KmoSync(
    KmoSyncConfig(
        storageConfigJson = """
            {
              "type": "s3",
              "endpoint": "https://s3.example.com",
              "bucket": "kmo-sync",
              "access_key": "ACCESS_KEY",
              "secret_key": "SECRET_KEY",
              "region": "us-east-1",
              "root_prefix": "kmo-sync"
            }
        """.trimIndent(),
        encryptionConfigJson = """
            {
              "type": "envelope",
              "passphrase": "replace-with-user-secret",
              "kek_id": "primary",
              "kek_version": 1
            }
        """.trimIndent(),
        deviceId = "android-phone-1",
        localCacheDir = "/app/private/kmo-sync"
    )
)

val result = sync.syncAll(SyncMode.Bidirectional)
val stateJson = sync.getSyncState()
sync.close()
```

`KmoSync` 的阻塞式 native 调用会切换到构造函数的 `workerDispatcher`。Android/JVM 应用可传入 `Dispatchers.IO` 或自己的有界线程池，避免多本大书同步占用默认计算线程池。`events` 会缓存订阅前和突发事件；一个实例应由一个事件收集器消费。

### 6. 常用 KMP 操作

```kotlin
sync.putLocalMetaJson(metaJson)
sync.syncSingleMeta(bookHash, "meta-id")
sync.putLocalBook(bookHash, localFilePath)
sync.syncBook(bookHash)
sync.setNetworkType(NetworkType.Cellular)
sync.setBlobByteLimit(10L * 1024L * 1024L)
sync.resolveMetaConflict("meta-id", "remote")
sync.resolveBlobConflict(bookHash, "local")
sync.rotateEnvelopeKek(newEncryptionConfigJson)
sync.markMetaItemDeleted("meta-id", "highlight", "highlight-1")
sync.undoDeletion("meta-id", "highlight-1")
sync.exportMetadataBackup()
```

`SyncResult.Failure` 会包含 native 错误码和安全错误信息。

> `markMetaItemDeleted` 会保存被删除条目的本地快照；`undoDeletion` 因而可以在同一设备恢复内容。tombstone 和 revival 会在下一次 sync 时通过条件写安全地传播到其他设备。

## 示例应用

构建 Android 示例：

```bash
cd kmo_sync
./scripts/build_android.sh
cd ..
gradle :samples:kmp-reader-sync:androidApp:assembleDebug
```

构建 iOS 示例：

```bash
cd kmo_sync
./scripts/build_ios_xcframework.sh
cd ..
xcodebuild \
  -project samples/kmp-reader-sync/iosApp/KmoSyncSample.xcodeproj \
  -target KmoSyncSample \
  -sdk iphonesimulator \
  CODE_SIGNING_ALLOWED=NO \
  build
```

示例配置文件：

```text
samples/kmp-reader-sync/androidApp/src/main/assets/kmo_sync_sample_config.json
samples/kmp-reader-sync/iosApp/KmoSyncSample/kmo_sync_sample_config.json
```

两个平台使用同一份远端存储配置，就可以测试跨设备同步。

## 可选集成测试

MinIO/S3：

```bash
cd kmo_sync
./scripts/start_minio.sh
./scripts/test_s3_minio.sh
```

WebDAV：

```bash
cd kmo_sync
./scripts/start_webdav.sh
./scripts/test_webdav.sh
```

R2 / S3 ignored 测试需要显式传环境变量，测试代码不会内置真实密钥：

```bash
cd kmo_sync
KMO_S3_ENDPOINT="https://<account-id>.r2.cloudflarestorage.com" \
KMO_S3_BUCKET="reeden" \
KMO_S3_REGION="auto" \
KMO_S3_ACCESS_KEY="..." \
KMO_S3_SECRET_KEY="..." \
cargo test --test r2_integration r2_three_books_phone_pad_phone_roundtrip -- --ignored --nocapture
```

`r2_three_books_phone_pad_phone_roundtrip` 会使用测试目录下的四本 EPUB，模拟 phone -> pad -> fresh phone -> reader-c 的进度、书签、划线、笔记、删除 / tombstone 合并，并在测试结束时清理本次 `kmo_sync_r2_phone_pad_phone_*` 前缀。

如果历史失败留下了该测试前缀，可以执行：

```bash
cd kmo_sync
KMO_S3_ENDPOINT="https://<account-id>.r2.cloudflarestorage.com" \
KMO_S3_BUCKET="reeden" \
KMO_S3_REGION="auto" \
KMO_S3_ACCESS_KEY="..." \
KMO_S3_SECRET_KEY="..." \
cargo test --test r2_dump r2_cleanup_phone_pad_phone_test_prefixes -- --ignored --nocapture
```

## 当前注意事项

- S3/WebDAV 集成测试默认 ignored，需要提供环境变量或使用 Docker helper 后才会执行。
- 远端对象布局从协议版本 7 起改为扁平 `kmo-sync/{books,book_progress,bookmarks}/...`，并改用 last-write-wins 合并。低于 7 的旧版本客户端不兼容，请同步升级。
- 老版本 `yuewei/books/<book_hash>/...` 路径下的数据**不会被自动迁移**；重新执行一次 `syncAll` 即可把数据写到新前缀，旧 `yuewei/` 路径可由用户自行决定是否清理。
- 删除 / 复活 tombstone 会跨设备传播，多设备间的并发写通过 ETag 条件写和重试收敛。
- Envelope 大文件使用 `KMOENV2` 分块加密格式，上传、下载和 KEK 重包不再把整本书载入内存。
- KEK 轮换先写入新命名空间，最后以单个 `_active_namespace.json` 条件写切换；失败时旧命名空间仍保持可读。
- iOS simulator 构建可能出现 native object 的 SDK 版本高于 app deployment target 的 linker warning；当前发布验收将其视为非致命警告。
- 不要把 `local.properties` 和真实密钥提交到 git。
