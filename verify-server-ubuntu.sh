#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${1:-http://127.0.0.1:8080}"

log() {
  printf '[bk-wiver-verify] %s\n' "$1"
}

fail() {
  printf '[bk-wiver-verify] ERROR: %s\n' "$1" >&2
  exit 1
}

require_cmd() {
  local cmd="$1"
  command -v "$cmd" >/dev/null 2>&1 || fail "required command is not installed: $cmd"
}

join_csv() {
  local first=1
  for item in "$@"; do
    if [[ $first -eq 1 ]]; then
      printf '%s' "$item"
      first=0
    else
      printf ', %s' "$item"
    fi
  done
}

status_in() {
  local actual="$1"
  shift
  local allowed
  for allowed in "$@"; do
    if [[ "$actual" == "$allowed" ]]; then
      return 0
    fi
  done
  return 1
}

request_status() {
  local method="$1"
  local url="$2"
  local body="${3:-}"
  shift 3 || true

  local -a curl_args
  curl_args=(
    --silent
    --show-error
    --output /dev/null
    --write-out '%{http_code}'
    --request "$method"
  )

  while (($#)); do
    curl_args+=("$1")
    shift
  done

  if [[ -n "$body" ]]; then
    curl_args+=(
      -H 'Content-Type: application/json'
      --data "$body"
    )
  fi

  curl_args+=("$url")
  curl "${curl_args[@]}"
}

check_status() {
  local label="$1"
  local method="$2"
  local url="$3"
  local body="$4"
  shift 4
  local actual
  actual="$(request_status "$method" "$url" "$body")"
  if ! status_in "$actual" "$@"; then
    fail "${label}: expected HTTP $(join_csv "$@"), got ${actual} for ${url}"
  fi
  log "${label}: HTTP ${actual}"
}

check_ws_route() {
  local label="$1"
  local url="$2"
  local actual
  actual="$(
    request_status \
      GET \
      "$url" \
      "" \
      --http1.1 \
      -H 'Connection: Upgrade' \
      -H 'Upgrade: websocket' \
      -H 'Sec-WebSocket-Version: 13' \
      -H 'Sec-WebSocket-Key: SGVsbG8sIHdvcmxkIQ=='
  )"
  if ! status_in "$actual" 101 400 401 403 426; then
    fail "${label}: expected HTTP 101/400/401/403/426, got ${actual} for ${url}"
  fi
  log "${label}: HTTP ${actual}"
}

require_cmd curl

BASE_URL="${BASE_URL%/}"
log "base url: ${BASE_URL}"

check_status \
  "healthz" \
  GET \
  "${BASE_URL}/healthz" \
  "" \
  200

check_status \
  "admin web ui" \
  GET \
  "${BASE_URL}/admin" \
  "" \
  200

check_status \
  "user login route" \
  POST \
  "${BASE_URL}/api/v1/auth/login" \
  '{"login":"invalid","password":"invalid","desktopVersion":{"version":"smoke","commit":"verify"}}' \
  400 401 403

check_status \
  "admin login route" \
  POST \
  "${BASE_URL}/api/v1/admin/auth/login" \
  '{"login":"invalid","password":"invalid"}' \
  400 401 403

check_status \
  "devices list route" \
  GET \
  "${BASE_URL}/api/v1/devices" \
  "" \
  401 403

check_status \
  "device register route" \
  POST \
  "${BASE_URL}/api/v1/devices/register" \
  '{"enrollmentToken":"invalid","desktopVersion":{"version":"smoke","commit":"verify"},"hostInfo":{"hostname":"probe","os":"linux","osVersion":"24.04","arch":"x86_64","username":"probe"},"permissions":{"screenCapture":true,"inputControl":true,"accessibility":true,"fileTransfer":true}}' \
  401 403

check_status \
  "device heartbeat route" \
  POST \
  "${BASE_URL}/api/v1/devices/heartbeat" \
  '{"deviceId":"probe","permissions":{"screenCapture":true,"inputControl":true,"accessibility":true,"fileTransfer":true},"unixTimeMs":1770000000000}' \
  401 403

check_status \
  "session create route" \
  POST \
  "${BASE_URL}/api/v1/sessions" \
  '{"deviceId":"probe"}' \
  401 403

check_status \
  "enrollment token route" \
  POST \
  "${BASE_URL}/api/v1/enrollment-tokens" \
  '{"comment":"probe","expiresAtMs":1771000000000,"singleUse":true}' \
  401 403

check_status \
  "audit route" \
  GET \
  "${BASE_URL}/api/v1/audit?limit=1" \
  "" \
  401 403

check_status \
  "admin devices route" \
  GET \
  "${BASE_URL}/api/v1/admin/devices" \
  "" \
  401 403

check_status \
  "admin users route" \
  GET \
  "${BASE_URL}/api/v1/admin/users" \
  "" \
  401 403

check_status \
  "admin enrollment route" \
  POST \
  "${BASE_URL}/api/v1/admin/enrollment-tokens" \
  '{"comment":"probe","expiresAtMs":1771000000000,"singleUse":true}' \
  401 403

check_status \
  "admin enrollment list route" \
  GET \
  "${BASE_URL}/api/v1/admin/enrollment-tokens" \
  "" \
  401 403

check_status \
  "admin audit route" \
  GET \
  "${BASE_URL}/api/v1/admin/audit?limit=1" \
  "" \
  401 403

check_ws_route \
  "signal websocket route" \
  "${BASE_URL}/ws/v1/signal?token=probe-invalid-token"

check_ws_route \
  "media websocket route" \
  "${BASE_URL}/ws/v1/media?token=probe-invalid-token&sessionId=probe-session"

log "server verification completed successfully"
