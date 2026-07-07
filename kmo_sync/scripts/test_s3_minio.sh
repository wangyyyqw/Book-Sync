#!/usr/bin/env sh
set -eu

ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT_DIR"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker was not found. Install Docker Desktop, then rerun this script." >&2
  exit 1
fi

./scripts/start_minio.sh

export KMO_S3_ENDPOINT="${KMO_S3_ENDPOINT:-http://127.0.0.1:9000}"
export KMO_S3_BUCKET="${KMO_S3_BUCKET:-kmo-test}"
export KMO_S3_ACCESS_KEY="${KMO_S3_ACCESS_KEY:-minioadmin}"
export KMO_S3_SECRET_KEY="${KMO_S3_SECRET_KEY:-minioadmin}"
export KMO_S3_REGION="${KMO_S3_REGION:-us-east-1}"

cargo test --test s3_integration -- --ignored --test-threads=1
