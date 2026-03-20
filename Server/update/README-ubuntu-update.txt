BK-Wiver Ubuntu update package

What to copy into the Ubuntu repo root:
- Cargo.toml
- Cargo.lock
- docker-compose.yml
- Server/app/Cargo.toml
- Server/app/Dockerfile
- Server/app/src/main.rs
- Server/app/src/server.rs
- Server/deploy/docker-compose.lan.yml
- Update/update-server-ubuntu.sh

Suggested deploy flow on Ubuntu 24.04:
1. Copy these files over the existing repo at /opt/bk-wiver
2. cd /opt/bk-wiver
3. chmod +x Update/update-server-ubuntu.sh
4. chmod +x Update/verify-server-ubuntu.sh
5. bash Update/update-server-ubuntu.sh /opt/bk-wiver
6. bash Update/verify-server-ubuntu.sh http://127.0.0.1:8080

What the script checks:
- required files are present
- docker compose rebuilds and restarts the server
- /healthz works locally and publicly
- /ws/v1/media no longer returns HTTP 404 locally and publicly

What verify-server-ubuntu.sh checks:
- /healthz returns 200
- core REST routes exist and return expected auth/validation codes instead of 404
- /ws/v1/signal exists
- /ws/v1/media exists
