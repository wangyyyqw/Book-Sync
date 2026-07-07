# Book Sync iOS Sample

Build the Rust xcframework first:

```bash
cd ../../../kmo_sync
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

The app creates a Book Sync instance using memory storage and runs `kmo_sync_all(handle, 0)`.
