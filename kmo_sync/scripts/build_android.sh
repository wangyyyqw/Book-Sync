#!/usr/bin/env sh
set -eu

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
CRATE_DIR="$ROOT_DIR/kmo_sync"
KMP_DIR="$ROOT_DIR/kmo-sync-kmp"
ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
if [ -z "$ANDROID_HOME" ] && [ -f "$ROOT_DIR/local.properties" ]; then
  ANDROID_HOME="$(sed -n 's/^sdk\.dir=//p' "$ROOT_DIR/local.properties" | head -1)"
fi
ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
MIN_SDK_VERSION="${MIN_SDK_VERSION:-23}"

if [ ! -d "$ANDROID_HOME/ndk" ]; then
  echo "Android NDK not found under $ANDROID_HOME/ndk" >&2
  exit 1
fi

NDK_DIR="${ANDROID_NDK_HOME:-$(find "$ANDROID_HOME/ndk" -maxdepth 1 -mindepth 1 -type d | sort | tail -1)}"
TOOLCHAIN_DIR="$(find "$NDK_DIR/toolchains/llvm/prebuilt" -maxdepth 1 -mindepth 1 -type d | sort | head -1)"
BIN_DIR="$TOOLCHAIN_DIR/bin"

rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android

export AR_aarch64_linux_android="$BIN_DIR/llvm-ar"
export CC_aarch64_linux_android="$BIN_DIR/aarch64-linux-android${MIN_SDK_VERSION}-clang"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$BIN_DIR/aarch64-linux-android${MIN_SDK_VERSION}-clang"

export AR_armv7_linux_androideabi="$BIN_DIR/llvm-ar"
export CC_armv7_linux_androideabi="$BIN_DIR/armv7a-linux-androideabi${MIN_SDK_VERSION}-clang"
export CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER="$BIN_DIR/armv7a-linux-androideabi${MIN_SDK_VERSION}-clang"

export AR_x86_64_linux_android="$BIN_DIR/llvm-ar"
export CC_x86_64_linux_android="$BIN_DIR/x86_64-linux-android${MIN_SDK_VERSION}-clang"
export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="$BIN_DIR/x86_64-linux-android${MIN_SDK_VERSION}-clang"

cd "$CRATE_DIR"
cargo build --release --target aarch64-linux-android
cargo build --release --target armv7-linux-androideabi
cargo build --release --target x86_64-linux-android

mkdir -p \
  "$KMP_DIR/src/androidMain/jniLibs/arm64-v8a" \
  "$KMP_DIR/src/androidMain/jniLibs/armeabi-v7a" \
  "$KMP_DIR/src/androidMain/jniLibs/x86_64"

cp "$CRATE_DIR/target/aarch64-linux-android/release/libkmo_sync.so" \
  "$KMP_DIR/src/androidMain/jniLibs/arm64-v8a/libkmo_sync.so"
cp "$CRATE_DIR/target/armv7-linux-androideabi/release/libkmo_sync.so" \
  "$KMP_DIR/src/androidMain/jniLibs/armeabi-v7a/libkmo_sync.so"
cp "$CRATE_DIR/target/x86_64-linux-android/release/libkmo_sync.so" \
  "$KMP_DIR/src/androidMain/jniLibs/x86_64/libkmo_sync.so"

echo "Android native libraries copied to $KMP_DIR/src/androidMain/jniLibs"
