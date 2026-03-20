#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
CONF_SRC="${REPO_ROOT}/Server/deploy/nginx/bk-wiver-lan.conf"
CONF_DST="/etc/nginx/sites-available/bk-wiver-lan.conf"

if [[ ! -f "${CONF_SRC}" ]]; then
  echo "Nginx config not found: ${CONF_SRC}" >&2
  exit 1
fi

sudo apt install -y nginx
sudo cp "${CONF_SRC}" "${CONF_DST}"
sudo ln -sf "${CONF_DST}" /etc/nginx/sites-enabled/bk-wiver-lan.conf
sudo rm -f /etc/nginx/sites-enabled/default
sudo nginx -t
sudo systemctl enable nginx
sudo systemctl restart nginx
