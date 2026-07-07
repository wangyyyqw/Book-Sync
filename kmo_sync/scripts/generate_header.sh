#!/usr/bin/env sh
set -eu

mkdir -p target/include
if command -v cbindgen >/dev/null 2>&1; then
  cbindgen --config cbindgen.toml --crate kmo_sync --output target/include/kmo_sync.h
else
  cp include/kmo_sync.h target/include/kmo_sync.h
fi

if ! grep -q "KMO_ERR_NETWORK" target/include/kmo_sync.h; then
  cat >> target/include/kmo_sync.h <<'EOF'

#define KMO_OK 0
#define KMO_ERR_NETWORK 1
#define KMO_ERR_STORAGE 2
#define KMO_ERR_CRYPTO 3
#define KMO_ERR_CONFLICT 4
#define KMO_ERR_INVALID_ARG 5
#define KMO_ERR_INTERNAL 6
#define KMO_ERR_VERSION_MISMATCH 11

#define KMO_NETWORK_WIFI 0
#define KMO_NETWORK_CELLULAR 1
#define KMO_NETWORK_UNKNOWN 2
EOF
fi
