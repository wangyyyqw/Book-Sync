#!/usr/bin/env sh
set -eu

if ! command -v docker >/dev/null 2>&1; then
  echo "docker was not found. Install Docker to run the local WebDAV integration server." >&2
  exit 1
fi

docker compose -f docker-compose.webdav.yml up -d

KMO_WEBDAV_URL="${KMO_WEBDAV_URL:-http://127.0.0.1:8080}" \
KMO_WEBDAV_USERNAME="${KMO_WEBDAV_USERNAME:-kmo}" \
KMO_WEBDAV_PASSWORD="${KMO_WEBDAV_PASSWORD:-kmo}" \
cargo test --test webdav_integration -- --ignored --test-threads=1
