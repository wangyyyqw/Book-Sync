#!/usr/bin/env sh
set -eu

ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
CRATE_DIR="$ROOT_DIR/kmo_sync"
NATIVE_LIB_DIR="$CRATE_DIR/target/release"
ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}}"
export ANDROID_HOME

cd "$CRATE_DIR"
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
sh scripts/generate_header.sh

cd "$ROOT_DIR"
cc -I kmo_sync/target/include kmo_sync/tests/ffi_smoke.c \
  -L "$NATIVE_LIB_DIR" -lkmo_sync \
  -Wl,-rpath,"$NATIVE_LIB_DIR" \
  -o /tmp/kmo_sync_ffi_smoke
/tmp/kmo_sync_ffi_smoke

gradle :kmo-sync-kmp:jvmTest

gradle :kmo-sync-kmp:assembleRelease
AAR="$ROOT_DIR/kmo-sync-kmp/build/outputs/aar/kmo-sync-kmp-release.aar"
for ABI in arm64-v8a armeabi-v7a x86_64; do
  unzip -Z1 "$AAR" | grep -qx "jni/$ABI/libkmo_sync.so"
done

gradle \
  :kmo-sync-kmp:linkReleaseFrameworkIosArm64 \
  :kmo-sync-kmp:linkReleaseFrameworkIosSimulatorArm64

sh "$CRATE_DIR/scripts/build_ios_xcframework.sh"

gradle :samples:kmp-reader-sync:androidApp:assembleDebug

xcodebuild \
  -project samples/kmp-reader-sync/iosApp/KmoSyncSample.xcodeproj \
  -target KmoSyncSample \
  -sdk iphonesimulator \
  CODE_SIGNING_ALLOWED=NO \
  build

if [ "${KMO_SYNC_RUN_DOCKER_INTEGRATION:-0}" = "1" ]; then
  cd "$CRATE_DIR"
  sh scripts/test_s3_minio.sh
  sh scripts/test_webdav.sh
fi

echo "KMO-Sync release verification passed."
