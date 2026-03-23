BK-Wiver Ubuntu 24.04 update package

This folder is now the update source of truth for the server.

What to copy to Ubuntu:
- the whole `Server/update` directory

Suggested deploy flow on Ubuntu 24.04:
1. Copy `Server/update` into the repo at `/opt/bk-wiver/Server/update`
2. `cd /opt/bk-wiver`
3. `chmod +x Server/update/update-server-ubuntu.sh`
4. `chmod +x Server/update/verify-server-ubuntu.sh`
5. `bash Server/update/update-server-ubuntu.sh /opt/bk-wiver`
6. `bash Server/update/verify-server-ubuntu.sh http://127.0.0.1:8080`

What `update-server-ubuntu.sh` does:
- verifies Docker, curl and `.env`
- takes packaged files from `Server/update`
- syncs them into live repo paths such as `Cargo.toml`, `docker-compose.yml`, `Server/app/...`, `Server/deploy/...`
- rebuilds and restarts containers with Docker Compose
- checks `/healthz`, `/admin` and `/ws/v1/media` locally and through `SERVER_PUBLIC_URL`

What `verify-server-ubuntu.sh` checks:
- `/healthz` returns `200`
- `/admin` returns `200`
- core REST routes exist and return expected auth or validation statuses instead of `404`
- admin routes for devices, users, audit and enrollment tokens exist
- `/ws/v1/signal` exists
- `/ws/v1/media` exists

What is included in this package:
- root Cargo workspace files
- server Docker and Rust sources
- Ubuntu LAN Docker Compose override
- the update and verify scripts
