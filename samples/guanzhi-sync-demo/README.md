# 观止 (guanzhi) × Book Sync 同步演示

为 `guanzhi/kmpApp`（观止阅读器）编写的 Book Sync 接入演示：用两台模拟设备
（手机 `phone-1` / 平板 `pad-1`）通过共享远端存储同步阅读进度、书签、划线、
笔记，并验证删除 / 复活 tombstone 的跨设备传播。

两种运行形态共用同一套场景逻辑（`commonMain/SyncScenario.kt`）：

- **Android 安装包**（推荐真机体验）
- **JVM 命令行版**（CI / 本机快速验证）

## Android 安装包

构建 APK（需要 Android SDK + NDK，首次会自动交叉编译三个 ABI 的 Rust 库）：

```bash
gradle :samples:guanzhi-sync-demo:assembleDebug
```

产物：

```text
samples/guanzhi-sync-demo/build/outputs/apk/debug/guanzhi-sync-demo-debug.apk
```

安装：`adb install samples/guanzhi-sync-demo/build/outputs/apk/debug/guanzhi-sync-demo-debug.apk`
（或直接拷贝到手机安装）。

App 内点击「运行双端同步演示」：单台手机上同时创建两个 `KmoSync` 实例模拟
手机和平板，共用应用外部存储目录作为远端，逐条显示 PASS/FAIL 断言，最后
给出汇总。远端协议对象（`book_progress/`、`bookmarks/`、`tombstones/`）落在
`Android/data/guanzhi.syncdemo/files/kmo-sync-remote/`，可导出检查。

> 说明：单机演示的"两台设备"共用同一台手机的远端目录。要测试两台真机
> 跨设备同步，把 storage 配置换成 S3/R2/WebDAV 即可（见 Book Sync README
> 的存储配置，`MainActivity.createDevice` 里的 `storageConfigJson`）。

## JVM 命令行版

```bash
gradle :samples:guanzhi-sync-demo:runDemo
```

可选环境变量：

| 变量 | 作用 |
|---|---|
| `KMO_DEMO_REMOTE_DIR` | 指定共享远端目录（默认临时目录）。指定后演示结束可以手工检查协议对象 |
| `KMO_SYNC_NATIVE_LIB_DIR` | 覆盖内置 native 库路径（见 Book Sync README） |

任一断言失败时进程以非零码退出，可接入 CI。

## 演示场景

1. 手机导入《三体》，读到 30%（第 3 章），添加书签「三体·序章」，同步
2. 平板拉取 → 确认进度 30% + 书签到达；平板继续读到 65%，添加书签、
   划线（黑暗森林法则）和笔记（不要回答！），同步
3. 手机拉取 → 确认进度 65%，书签按 union 合并为 2 条，划线 / 笔记各 1 条
4. 手机删除平板的书签 → 平板拉取后书签消失（tombstone 跨设备传播）
5. 手机撤销删除 → 平板拉取后书签复活（revival 传播）

## 代码结构

```
src/
├── commonMain/kotlin/guanzhi/syncdemo/
│   ├── GuanzhiModels.kt      # 观止模型最小镜像（LocalBook / ReadingProgress / BookBookmark）
│   ├── GuanzhiSyncMapper.kt  # 观止数据 <-> Book Sync meta JSON 的映射层（可直接搬进观止 commonMain）
│   └── SyncScenario.kt       # 双端同步场景 + 断言（JVM / Android 共用）
├── jvmMain/kotlin/guanzhi/syncdemo/
│   └── JvmDemoMain.kt        # 命令行入口（runDemo）
└── androidMain/
    ├── kotlin/guanzhi/syncdemo/MainActivity.kt  # Android 入口（assembleDebug）
    └── AndroidManifest.xml
```

## 接入观止时需要注意

- `book_hash`：demo 用图书 id 充当；真实接入应使用书籍文件内容的 blake3
  小写十六进制哈希（Book Sync 以它作为远端对象键）。
- 书签创建时间：观止模型没有 `create_ts`，映射层用同步时的 `logical_ts`
  充当，保证同 ID 书签按时间取大者的合并语义；真实接入时建议在观止的
  `BookBookmark` 中补充创建时间字段。
- `logicalTs` 应随每次本地编辑单调递增（观止可用自己仓库的全局版本号），
  Book Sync 按 `wall_clock_ts` + `device_id` 做 last-write-wins。
- 删除：`markMetaItemDeleted` 会保存被删条目的本地快照，同一设备可以
  `undoDeletion` 恢复；tombstone 与 revival 在下次 sync 时传播到其他设备。
