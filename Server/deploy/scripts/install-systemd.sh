#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
UNIT_SRC="${REPO_ROOT}/Server/deploy/systemd/bk-wiver.service"
UNIT_DST="/etc/systemd/system/bk-wiver.service"

if [[ ! -f "${UNIT_SRC}" ]]; then
  echo "Systemd unit not found: ${UNIT_SRC}" >&2
  exit 1
fi

sudo cp "${UNIT_SRC}" "${UNIT_DST}"
sudo systemctl daemon-reload
sudo systemctl enable bk-wiver.service
sudo systemctl restart bk-wiver.service
sudo systemctl status bk-wiver.service --no-pager
