#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PACKAGE_ROOT="$SCRIPT_DIR"
REPO_ROOT="${1:-$(cd "${SCRIPT_DIR}/../.." && pwd)}"
COMPOSE_ARGS=(-f docker-compose.yml -f Server/deploy/docker-compose.lan.yml)

log() {
  printf '[bk-wiver-update] %s\n' "$1"
}

fail() {
  printf '[bk-wiver-update] ERROR: %s\n' "$1" >&2
  exit 1
}

require_file() {
  local path="$1"
  [[ -f "$path" ]] || fail "missing file: $path"
}

require_cmd() {
  local cmd="$1"
  command -v "$cmd" >/dev/null 2>&1 || fail "required command is not installed: $cmd"
}

sync_file() {
  local source_path="$1"
  local target_path="$2"

  [[ -f "$source_path" ]] || fail "missing update package file: $source_path"

  mkdir -p "$(dirname "$target_path")"
  cp "$source_path" "$target_path"
  log "synced $(realpath --relative-to="$REPO_ROOT" "$target_path" 2>/dev/null || printf '%s' "$target_path")"
}

check_media_route() {
  local label="$1"
  local base_url="$2"
  local http_code
  http_code="$(
    curl \
      --silent \
      --show-error \
      --output /dev/null \
      --write-out '%{http_code}' \
      --http1.1 \
      -H 'Connection: Upgrade' \
      -H 'Upgrade: websocket' \
      -H 'Sec-WebSocket-Version: 13' \
      -H 'Sec-WebSocket-Key: SGVsbG8sIHdvcmxkIQ==' \
      "${base_url}/ws/v1/media?token=probe-invalid-token&sessionId=probe-session" \
      || true
  )"

  [[ -n "$http_code" ]] || fail "media route check failed for ${label}: empty HTTP status"
  [[ "$http_code" != "404" ]] || fail "media route is still missing for ${label}: ${base_url}/ws/v1/media returned 404"

  log "media route check for ${label}: HTTP ${http_code}"
}

require_cmd docker
require_cmd curl

[[ -d "$REPO_ROOT" ]] || fail "repo root does not exist: $REPO_ROOT"
cd "$REPO_ROOT"

require_file ".env"
require_file "${PACKAGE_ROOT}/Cargo.toml"
require_file "${PACKAGE_ROOT}/Cargo.lock"
require_file "${PACKAGE_ROOT}/docker-compose.yml"
require_file "${PACKAGE_ROOT}/apps/server/Cargo.toml"
require_file "${PACKAGE_ROOT}/apps/server/Dockerfile"
require_file "${PACKAGE_ROOT}/apps/server/src/main.rs"
require_file "${PACKAGE_ROOT}/apps/server/src/server.rs"
require_file "${PACKAGE_ROOT}/deploy/ubuntu/docker-compose.lan.yml"

log "repo root: $REPO_ROOT"
log "update package root: $PACKAGE_ROOT"
log "syncing packaged files into the repo"

sync_file "${PACKAGE_ROOT}/Cargo.toml" "${REPO_ROOT}/Cargo.toml"
sync_file "${PACKAGE_ROOT}/Cargo.lock" "${REPO_ROOT}/Cargo.lock"
sync_file "${PACKAGE_ROOT}/docker-compose.yml" "${REPO_ROOT}/docker-compose.yml"
sync_file "${PACKAGE_ROOT}/apps/server/Cargo.toml" "${REPO_ROOT}/Server/app/Cargo.toml"
sync_file "${PACKAGE_ROOT}/apps/server/Dockerfile" "${REPO_ROOT}/Server/app/Dockerfile"
sync_file "${PACKAGE_ROOT}/apps/server/src/main.rs" "${REPO_ROOT}/Server/app/src/main.rs"
sync_file "${PACKAGE_ROOT}/apps/server/src/server.rs" "${REPO_ROOT}/Server/app/src/server.rs"
sync_file "${PACKAGE_ROOT}/deploy/ubuntu/docker-compose.lan.yml" "${REPO_ROOT}/Server/deploy/docker-compose.lan.yml"

SERVER_PUBLIC_URL="$(grep -E '^SERVER_PUBLIC_URL=' .env | tail -n 1 | cut -d '=' -f 2- || true)"
[[ -n "$SERVER_PUBLIC_URL" ]] || fail "SERVER_PUBLIC_URL is not set in .env"

if [[ "$SERVER_PUBLIC_URL" == *"127.0.0.1"* || "$SERVER_PUBLIC_URL" == *"localhost"* ]]; then
  fail "SERVER_PUBLIC_URL points to localhost/127.0.0.1. Set a LAN address, for example http://172.16.100.164:8080"
fi

log "building and starting containers"
docker compose "${COMPOSE_ARGS[@]}" up --build -d

log "container status"
docker compose "${COMPOSE_ARGS[@]}" ps

log "recent server logs"
docker compose "${COMPOSE_ARGS[@]}" logs --tail=200 server

log "checking local health: http://127.0.0.1:8080/healthz"
curl --fail --silent --show-error http://127.0.0.1:8080/healthz
printf '\n'

log "checking public health: ${SERVER_PUBLIC_URL}/healthz"
curl --fail --silent --show-error "${SERVER_PUBLIC_URL}/healthz"
printf '\n'

log "checking local admin page: http://127.0.0.1:8080/admin"
curl --fail --silent --show-error http://127.0.0.1:8080/admin >/dev/null

log "checking public admin page: ${SERVER_PUBLIC_URL}/admin"
curl --fail --silent --show-error "${SERVER_PUBLIC_URL}/admin" >/dev/null

log "checking local media route: http://127.0.0.1:8080/ws/v1/media"
check_media_route "local" "http://127.0.0.1:8080"

log "checking public media route: ${SERVER_PUBLIC_URL}/ws/v1/media"
check_media_route "public" "${SERVER_PUBLIC_URL}"

log "update completed successfully"
