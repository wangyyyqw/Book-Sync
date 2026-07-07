#!/usr/bin/env sh
set -eu

CRATE_DIR="$(cd "$(dirname "$0")/.." && pwd)"
OUTPUT_DIR="$CRATE_DIR/target/apple"
FRAMEWORK_DIR="$OUTPUT_DIR/KmoSync.xcframework"

rustup target add aarch64-apple-ios aarch64-apple-ios-sim

cd "$CRATE_DIR"
cargo build --release --target aarch64-apple-ios
cargo build --release --target aarch64-apple-ios-sim
sh scripts/generate_header.sh

rm -rf "$FRAMEWORK_DIR"
mkdir -p "$OUTPUT_DIR"

xcodebuild -create-xcframework \
  -library "$CRATE_DIR/target/aarch64-apple-ios/release/libkmo_sync.a" \
  -headers "$CRATE_DIR/target/include" \
  -library "$CRATE_DIR/target/aarch64-apple-ios-sim/release/libkmo_sync.a" \
  -headers "$CRATE_DIR/target/include" \
  -output "$FRAMEWORK_DIR"

echo "iOS xcframework written to $FRAMEWORK_DIR"
