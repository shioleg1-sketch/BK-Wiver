#!/usr/bin/env bash
set -euo pipefail

DOMAIN="${1:-bk-wiver.lan}"
CERT_PATH="/etc/ssl/certs/bk-wiver-lan.crt"
KEY_PATH="/etc/ssl/private/bk-wiver-lan.key"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
CONF_SRC="${REPO_ROOT}/Server/deploy/nginx/bk-wiver-lan-selfsigned.conf"
CONF_DST="/etc/nginx/sites-available/bk-wiver-lan.conf"

if [[ ! -f "${CONF_SRC}" ]]; then
  echo "Nginx HTTPS config not found: ${CONF_SRC}" >&2
  exit 1
fi

sudo apt install -y nginx openssl
sudo openssl req -x509 -nodes -days 365 \
  -newkey rsa:2048 \
  -keyout "${KEY_PATH}" \
  -out "${CERT_PATH}" \
  -subj "/CN=${DOMAIN}"

sudo cp "${CONF_SRC}" "${CONF_DST}"
sudo sed -i "s/server_name bk-wiver.lan;/server_name ${DOMAIN};/g" "${CONF_DST}"
sudo ln -sf "${CONF_DST}" /etc/nginx/sites-enabled/bk-wiver-lan.conf
sudo rm -f /etc/nginx/sites-enabled/default
sudo nginx -t
sudo systemctl enable nginx
sudo systemctl restart nginx
