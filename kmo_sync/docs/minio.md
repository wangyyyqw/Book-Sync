# MinIO Integration Test Setup

Book Sync uses MinIO to validate the S3 adapter locally.

## Start MinIO

```bash
./scripts/start_minio.sh
```

This starts:

- S3 API: `http://127.0.0.1:9000`
- Console: `http://127.0.0.1:9001`
- User/password: `minioadmin` / `minioadmin`
- Bucket: `kmo-test`

## Run S3 Tests

```bash
./scripts/test_s3_minio.sh
```

Equivalent manual command:

```bash
KMO_S3_ENDPOINT=http://127.0.0.1:9000 \
KMO_S3_BUCKET=kmo-test \
KMO_S3_ACCESS_KEY=minioadmin \
KMO_S3_SECRET_KEY=minioadmin \
KMO_S3_REGION=us-east-1 \
cargo test --test s3_integration -- --ignored --test-threads=1
```

The ignored tests cover:

- S3 object contract: write/read/list/stat/remove.
- Meta sync through S3: device A uploads, device B pulls.

## Stop MinIO

```bash
docker compose -f docker-compose.minio.yml down
```
