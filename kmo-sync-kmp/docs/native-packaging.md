# Native Packaging

## Android

Build and copy Rust shared libraries into the KMP wrapper:

```bash
cd ../kmo_sync
./scripts/build_android.sh
```

Output:

```text
kmo-sync-kmp/src/androidMain/jniLibs/
├── arm64-v8a/libkmo_sync.so
├── armeabi-v7a/libkmo_sync.so
└── x86_64/libkmo_sync.so
```

## iOS

Build the static libraries and package an xcframework:

```bash
cd ../kmo_sync
./scripts/build_ios_xcframework.sh
```

Output:

```text
kmo_sync/target/apple/KmoSync.xcframework
```

The generated `kmo_sync.h` is included from `kmo_sync/target/include`.
