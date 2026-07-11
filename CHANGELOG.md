# 更新日志

本项目的主要变更记录在此文件中。格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本遵循语义化版本规则。

## [未发布]

### 新增

- 为 File、S3/R2 和 WebDAV 存储增加版本化读取与条件写入能力。
- 增加元数据 CAS 重试机制：远端版本发生变化时自动重新读取、合并并重试，防止多设备同时同步造成静默覆盖。
- tombstone 现在保存被删除条目的快照，允许在原设备通过 `undoDeletion` 恢复书签、划线或笔记。
- 增加 `KMOENV2` Envelope 分块加密格式，支持书籍文件流式加密、解密和 KEK 重包。
- 增加原子 KEK 轮换：先写入新命名空间，再通过 `_active_namespace.json` 条件写一次性切换。
- KMP 增加 `iosArm64`、`iosSimulatorArm64` target、C interop 和 iOS native bridge。
- Android native bridge 增加事件回调，`events` Flow 现在可以接收同步状态和冲突事件。
- 增加并发设备书签合并、旧本地划线覆盖、同设备撤销删除、路径逃逸、协议版本和 FFI 并发生命周期回归测试。
- S3/R2 与 WebDAV 集成测试增加条件写和陈旧版本拒绝场景。

### 修复

- 修复标准 Android `assembleRelease` 不包含 `libkmo_sync.so` 的问题；AAR 构建现在自动触发三个 ABI 的 Rust 构建并校验产物。
- 修复 Android 消费方开启 R8/minify 后 JNI 事件回调可能被改名的问题，AAR 现在携带 consumer keep 规则。
- 修复 KMP iOS framework 在干净 checkout 中依赖预先存在 staticlib 的问题，link/cinterop task 会自动构建对应 Rust target。
- 修复并发 KMP 调用可能组合出错误码 A 和错误文本 B 的问题；调用与取错串行化，FFI 错误同时按线程隔离。
- 修复 iOS 在 `close()` 后抛异常、与 Android/JVM 返回失败不一致的问题；三个平台现在统一返回 `InvalidArg` 失败。
- 修复订阅前或突发超过 64 条的同步事件静默丢失问题，事件改为无界队列并在关闭时终止 Flow。
- 修复 JVM native smoke test 未配置外部库时直接跳过的问题；JAR 现在内置当前宿主 native 库，测试始终实际加载。
- 修复阻塞 native 同步固定占用 `Dispatchers.Default` 的问题，`KmoSync` 允许注入专用 worker dispatcher。
- 修复两台设备同时添加书签或划线时，最后一次无条件 PUT 覆盖另一台设备数据的问题。
- 修复较新的远端划线评论或笔记内容被旧本地副本反向覆盖的问题。
- 修复选择远端元数据版本时错误地将本地数据写回远端的问题。
- 修复 `undoDeletion` 只撤销 tombstone、但不能恢复原条目的问题。
- 修复 FFI 从同一裸指针创建多个可变引用，以及并发 `close()` 可能触发 use-after-free 的问题。
- 修复大文件同步将整本书载入内存的问题；明文、age 和 Envelope 文件现在均走流式传输。
- 修复 KEK 轮换中途失败后远端同时存在新旧密钥对象、导致设备无法完整读取的问题。
- 修复 `meta_id` 和 `book_hash` 未校验导致缓存目录路径逃逸的问题。
- 修复远端元数据 `schema_version` 未验证的问题；不兼容版本现在返回版本错误。
- 修复 Android KMP 层丢弃 native 事件回调的问题。
- 修复没有 ETag 时仅按文件大小判断远端书籍未变化的问题；现在会同时校验修改时间。
- 修复跨设备集成测试依赖开发者电脑绝对 EPUB 路径的问题，改为运行时生成确定性测试文件。
- 修复 C ABI smoke test 与 tombstone API 当前行为不一致的问题。

### 变更

- S3/R2 和 WebDAV 的默认远端前缀由 `kmo_sync` 统一为文档所述的 `kmo-sync`。
- 同一 ID 的书签、划线和笔记由较新的元数据快照获胜，同时保留另一侧独有的条目。
- 本地同一同步实例的网络与轮换操作会串行执行；不同设备之间仍通过远端条件写并发协调。
- Android `.so` 不再提交到仓库，由 Gradle 在 Android 构建前从当前 Rust 源码自动生成。
- `local.properties` 不再纳入版本控制，发布脚本会从 `ANDROID_HOME` 或默认 SDK 路径定位 Android SDK。
- 发布验收现在覆盖格式检查、Clippy、Rust 全量测试、C ABI、自包含 KMP JVM、Android Release AAR 三 ABI、Android 示例、两个 KMP iOS framework、iOS XCFramework 和 iOS 示例构建。

### 兼容性说明

- `KMOENV1` 仍可读取；新上传的大型 Envelope 加密书籍使用 `KMOENV2`。
- 已显式配置 `root_prefix` 或 WebDAV `root_dir` 的部署不受默认前缀调整影响。
- 旧版本若省略 `root_prefix`，实际数据可能位于 `kmo_sync`。升级后应显式保留 `"root_prefix":"kmo_sync"`，或将远端数据迁移到 `kmo-sync`。
- WebDAV 服务必须返回 ETag 才能执行安全的并发元数据更新；缺少 ETag 时同步会明确失败，而不会退化为可能丢数据的无条件覆盖。

### 验证

- 67 项 Rust 单元测试通过。
- 三本书的 phone → pad → phone 跨设备模拟通过。
- 并发设备新增不同书签的收敛测试通过。
- C ABI smoke test 和 FFI 跨设备测试通过。
- KMP JVM、Android AAR、Android 示例 APK、KMP iOS simulator framework、iOS XCFramework 和 Swift 示例构建通过。

## [0.1.0] - 2026-07-11

### 新增

- 初始版本。
- 提供 Rust 同步核心、C ABI、Android/JVM KMP 封装和 iOS C ABI 示例。
- 支持阅读进度、书签、划线、笔记、书籍文件和 tombstone 同步。
- 支持本地文件、S3/R2 和 WebDAV 存储。
- 支持不加密、age 加密和 AES-GCM Envelope 加密。
