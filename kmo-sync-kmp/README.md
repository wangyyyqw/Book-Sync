# Book Sync KMP 封装

本模块把 Rust 实现的 Book Sync 核心封装成 Kotlin Multiplatform API。

支持的目标：

- `commonMain`：共享 `KmoSync` API、配置类型、同步模式、事件和结果类型。
- `androidMain`：通过 JNI 调用 `libkmo_sync.so`。
- `jvmMain`：通过 JNA 调用 Rust 动态库。
- iOS app：链接生成的 `KmoSync.xcframework`，通过 Swift/ObjC 调用 C ABI。示例 app 已经演示该方式。

## 添加到 KMP 应用

在 `settings.gradle.kts` 中包含模块：

```kotlin
include(":kmo-sync-kmp")
```

在 app 模块中添加依赖：

```kotlin
dependencies {
    implementation(project(":kmo-sync-kmp"))
}
```

## Android 打包

构建并复制 Android native 库：

```bash
cd ../kmo_sync
./scripts/build_android.sh
```

输出位置：

```text
kmo-sync-kmp/src/androidMain/jniLibs/
├── arm64-v8a/libkmo_sync.so
├── armeabi-v7a/libkmo_sync.so
└── x86_64/libkmo_sync.so
```

Android app 依赖 `:kmo-sync-kmp` 后，会自动打包这些 `.so` 文件。

## JVM 打包

构建 Rust native 库：

```bash
cd ../kmo_sync
cargo build --release
```

让 JNA 找到 native 库：

```bash
KMO_SYNC_NATIVE_LIB_DIR=/absolute/path/to/book-sync/kmo_sync/target/release \
../gradle :kmo-sync-kmp:jvmTest
```

应用也可以设置 JVM system property：

```text
-Dkmo.sync.native.lib.dir=/absolute/path/to/kmo_sync/target/release
```

## iOS 打包

构建 XCFramework：

```bash
cd ../kmo_sync
./scripts/build_ios_xcframework.sh
```

输出位置：

```text
kmo_sync/target/apple/KmoSync.xcframework
kmo_sync/target/include/kmo_sync.h
```

把 `KmoSync.xcframework` 加入 iOS app target，并通过 `kmo_sync.h` 在 Swift/ObjC 桥接代码中调用。

## 基础 API

```kotlin
val sync = KmoSync(
    KmoSyncConfig(
        storageConfigJson = """{"type":"file","root_dir":"/tmp/kmo-remote"}""",
        encryptionConfigJson = """{"type":"envelope","passphrase":"secret"}""",
        deviceId = "device-a",
        localCacheDir = "/tmp/kmo-cache"
    )
)

sync.syncAll(SyncMode.Bidirectional)
sync.syncSingleMeta("meta-id")
sync.syncBook("book-hash")
sync.close()
```

常用操作：

- `putLocalMetaJson(metaJson)`
- `putLocalBook(bookHash, localFilePath)`
- `syncAll(mode)`
- `syncSingleMeta(metaId)`
- `syncBook(bookHash)`
- `getLocalMeta(metaId)`
- `getSyncState()`
- `resolveMetaConflict(metaId, "local" | "remote")`
- `resolveBlobConflict(bookHash, "local" | "remote")`
- `markMetaItemDeleted(metaId, itemType, itemUuid)`
- `undoDeletion(metaId, itemUuid)`
- `resolveTombstoneRevival(metaId, itemUuid, Delete | Restore)`
- `rotateEnvelopeKek(newEncryptionConfigJson)`

完整应用接入示例见根目录 `README.md` 和 `samples/kmp-reader-sync`。
