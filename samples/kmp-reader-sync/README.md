# KMP Reader Sync Sample

This sample contains Android and iOS smoke apps for Book Sync.

Both apps expose two manual actions:

- Push local `shared-meta` to the configured remote.
- Pull `shared-meta` from the configured remote and display the JSON.
- Show the current sync state JSON, including pending conflicts.
- Resolve the first pending conflict with the sample default policy.

## Shared Remote Config

Default config is memory storage, useful only for single-app smoke testing.

For Android, edit:

```text
samples/kmp-reader-sync/androidApp/src/main/assets/kmo_sync_sample_config.json
```

For iOS, edit:

```text
samples/kmp-reader-sync/iosApp/KmoSyncSample/kmo_sync_sample_config.json
```

Use the same remote config on both sides. A MinIO/S3 example is available at:

```text
samples/kmp-reader-sync/kmo_sync_sample_config.example.json
```

## Android

Build Android native libraries:

```bash
cd kmo_sync
./scripts/build_android.sh
```

Build the debug APK:

```bash
gradle :samples:kmp-reader-sync:androidApp:assembleDebug
```

APK output:

```text
samples/kmp-reader-sync/androidApp/build/outputs/apk/debug/androidApp-debug.apk
```

## iOS

Build the Rust xcframework:

```bash
cd kmo_sync
./scripts/build_ios_xcframework.sh
```

Build the simulator app:

```bash
xcodebuild \
  -project samples/kmp-reader-sync/iosApp/KmoSyncSample.xcodeproj \
  -target KmoSyncSample \
  -sdk iphonesimulator \
  CODE_SIGNING_ALLOWED=NO \
  build
```

## Manual E2E Flow

1. Put the same S3/WebDAV config into both sample config files.
2. Build Android native libraries and the Android APK.
3. Build the iOS xcframework and iOS sample.
4. On Android, tap `Push Android Meta`.
5. On iOS, tap `Pull Shared Meta`; progress should show `0.42`.
6. On iOS, tap `Push iOS Meta`.
7. On Android, tap `Pull Shared Meta`; progress should show `0.84`.

For conflict smoke testing, tap `Show Sync State` after creating a conflict. If
`conflicts` is non-empty, tap `Resolve First Conflict`. The sample resolves Meta
conflicts with `remote` and Tombstone revival conflicts with `restore`.

## Automated Core E2E

The same Android/iOS meta handoff is covered at the C ABI level with a shared file remote:

```bash
cd kmo_sync
./scripts/test_sample_meta_e2e.sh
```

This test performs:

1. Android-like device pushes `shared-meta` progress `0.42`.
2. iOS-like device pulls and verifies `0.42`.
3. iOS-like device pushes `shared-meta` progress `0.84`.
4. Android-like device pulls and verifies `0.84`.
