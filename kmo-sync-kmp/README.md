# Book Sync KMP 封装

本模块把 Rust 实现的 Book Sync 核心封装成 Kotlin Multiplatform API。

支持的目标：

- `commonMain`：共享 `KmoSync` API、配置类型、同步模式、事件和结果类型。
- `androidMain`：通过 JNI 调用 `libkmo_sync.so`。
- `jvmMain`：通过 JNA 调用 Rust 动态库。
- `iosArm64` / `iosSimulatorArm64`：通过 cinterop 调用 Rust staticlib，也可供 Swift/ObjC app 使用生成的 framework。

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

Gradle 会在 Android `preBuild` 前构建 Rust native 库。直接执行：

```bash
../gradle :kmo-sync-kmp:assembleRelease
```

输出位置：

```text
kmo-sync-kmp/src/androidMain/jniLibs/
├── arm64-v8a/libkmo_sync.so
├── armeabi-v7a/libkmo_sync.so
└── x86_64/libkmo_sync.so
```

Android app 依赖 `:kmo-sync-kmp` 后，会自动打包这些 `.so` 文件；AAR 的 consumer R8 规则会保留 JNI 和事件回调入口。

## JVM 打包

Gradle 会构建并打包当前宿主平台的 Rust native 库：

```bash
../gradle :kmo-sync-kmp:jvmTest
```

JAR 运行时会自动解压内置库。跨 OS 发布应使用 macOS、Linux、Windows CI runner 分别构建。也可以用 JVM system property 覆盖内置库：

```text
-Dkmo.sync.native.lib.dir=/absolute/path/to/kmo_sync/target/release
```

## iOS 打包

构建 KMP iOS framework（Gradle 会先构建 Rust staticlib）：

```bash
../gradle :kmo-sync-kmp:linkReleaseFrameworkIosArm64 \
  :kmo-sync-kmp:linkReleaseFrameworkIosSimulatorArm64
```

纯 Swift/ObjC 接入需要 XCFramework 时仍可执行 `kmo_sync/scripts/build_ios_xcframework.sh`，输出位置：

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

可通过第二个构造参数传入 `Dispatchers.IO` 或专用 dispatcher 执行阻塞式 native 同步。`events` 使用不丢失的队列保留订阅前事件，一个 `KmoSync` 实例应只配置一个事件收集器。

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
