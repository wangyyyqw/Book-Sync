#!/usr/bin/env sh
set -eu

if ! command -v docker >/dev/null 2>&1; then
  echo "docker was not found. Install Docker to run the local WebDAV integration server." >&2
  exit 1
fi

docker compose -f docker-compose.webdav.yml up -d
