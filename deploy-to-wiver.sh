#!/bin/bash
# Скрипт развёртывания BK-Wiver на сервере wiver.bk.local (172.16.100.164)
# Использование: sudo ./deploy-to-wiver.sh

set -e

echo "=== BK-Wiver Deployment ==="
echo "Server: wiver.bk.local (172.16.100.164)"
echo ""

# Проверка запуска от root
if [ "$EUID" -ne 0 ]; then
    echo "Ошибка: запустите скрипт от имени root (sudo)"
    exit 1
fi

cd /opt/bk-wiver

# 1. Установка зависимостей
echo "[1/6] Установка зависимостей..."
apt-get update -qq
apt-get install -y nginx docker.io docker-compose-plugin

# 2. Копирование конфигурации Nginx
echo "[2/6] Настройка Nginx..."
cp -f deploy/nginx/nginx-wiver.conf /etc/nginx/sites-available/bk-wiver
ln -sf /etc/nginx/sites-available/bk-wiver /etc/nginx/sites-enabled/bk-wiver
rm -f /etc/nginx/sites-enabled/default

# 3. Проверка Nginx
echo "[3/6] Проверка конфигурации Nginx..."
nginx -t

# 4. Копирование .env
echo "[4/6] Настройка окружения..."
cp -f .env.server .env

# 5. Запуск Docker
echo "[5/6] Запуск Docker контейнеров..."
docker compose -f docker-compose.nginx.yml down || true
docker compose -f docker-compose.nginx.yml up -d --build

# 6. Перезапуск Nginx
echo "[6/6] Перезапуск Nginx..."
systemctl restart nginx

echo ""
echo "=== Развёртывание завершено ==="
echo ""
echo "Проверка:"
echo "  curl http://127.0.0.1:8080/healthz"
echo "  curl http://wiver.bk.local/healthz"
echo ""
echo "Админ-панель: http://wiver.bk.local/admin"
echo ""
echo "Логи:"
echo "  docker compose logs -f server"
echo "  tail -f /var/log/nginx/bk-wiver-error.log"
echo ""
