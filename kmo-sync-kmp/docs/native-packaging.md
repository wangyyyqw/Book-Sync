# Native Packaging

## Android

Build the release AAR. Gradle builds all Rust shared libraries first:

```bash
../gradle :kmo-sync-kmp:assembleRelease
```

Output:

```text
kmo-sync-kmp/src/androidMain/jniLibs/
├── arm64-v8a/libkmo_sync.so
├── armeabi-v7a/libkmo_sync.so
└── x86_64/libkmo_sync.so
```

The AAR also ships consumer R8 rules for JNI entry points and `onEvent`.

## JVM

`jvmJar` and `jvmTest` build the current host Rust library and package it under
`native/<os>-<arch>/`. Runtime extraction is automatic. Use an OS/architecture CI
matrix when publishing artifacts for more than one desktop platform.

## iOS

Build KMP frameworks; Gradle builds the matching Rust static library first:

```bash
../gradle :kmo-sync-kmp:linkReleaseFrameworkIosArm64 \
  :kmo-sync-kmp:linkReleaseFrameworkIosSimulatorArm64
```

For the standalone C ABI XCFramework, run `kmo_sync/scripts/build_ios_xcframework.sh`.

Output:

```text
kmo_sync/target/apple/KmoSync.xcframework
```

The generated `kmo_sync.h` is included from `kmo_sync/target/include`.
