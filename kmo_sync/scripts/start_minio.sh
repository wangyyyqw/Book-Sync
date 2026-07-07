#!/usr/bin/env sh
set -eu

ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"

if docker compose version >/dev/null 2>&1; then
  COMPOSE="docker compose"
elif command -v docker-compose >/dev/null 2>&1; then
  COMPOSE="docker-compose"
else
  echo "Docker Compose is required. Install Docker Desktop or docker-compose." >&2
  exit 1
fi

cd "$ROOT_DIR"
$COMPOSE -f docker-compose.minio.yml up -d

cat <<'EOF'
MinIO is starting.

API endpoint: http://127.0.0.1:9000
Console:      http://127.0.0.1:9001
User:         minioadmin
Password:     minioadmin
Bucket:       kmo-test

Run S3 integration tests with:
  ./scripts/test_s3_minio.sh
EOF
