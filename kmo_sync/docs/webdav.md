# WebDAV Integration Test Setup

Book Sync uses a local WebDAV server to validate the WebDAV adapter.

## Start WebDAV

```bash
./scripts/start_webdav.sh
```

This starts:

- URL: `http://127.0.0.1:8080`
- User/password: `kmo` / `kmo`

## Run WebDAV Tests

```bash
./scripts/test_webdav.sh
```

Equivalent manual command:

```bash
KMO_WEBDAV_URL=http://127.0.0.1:8080 \
KMO_WEBDAV_USERNAME=kmo \
KMO_WEBDAV_PASSWORD=kmo \
cargo test --test webdav_integration -- --ignored --test-threads=1
```

The ignored tests cover:

- WebDAV object contract: write/read/list/stat/remove.
- Meta sync through WebDAV: device A uploads, device B pulls.

## Stop WebDAV

```bash
docker compose -f docker-compose.webdav.yml down
```
