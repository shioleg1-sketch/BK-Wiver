#!/bin/bash
# Скрипт установки и настройки Nginx для BK-Wiver Server
# Использование: sudo ./setup-nginx-ubuntu.sh

set -e

echo "=== BK-Wiver Nginx Setup ==="
echo "ОС: $(lsb_release -ds 2>/dev/null || cat /etc/os-release | grep PRETTY_NAME | cut -d'\"' -f2)"
echo ""

# Проверка запуска от root
if [ "$EUID" -ne 0 ]; then
    echo "Ошибка: запустите скрипт от имени root (sudo)"
    exit 1
fi

# Установка Nginx
echo "[1/5] Установка Nginx..."
apt-get update -qq
apt-get install -y nginx

# Копирование конфигурации
echo "[2/5] Копирование конфигурации Nginx..."
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cp -f "$SCRIPT_DIR/deploy/nginx/nginx.conf" /etc/nginx/sites-available/bk-wiver

# Создание символической ссылки
echo "[3/5] Активация конфигурации..."
ln -sf /etc/nginx/sites-available/bk-wiver /etc/nginx/sites-enabled/bk-wiver
rm -f /etc/nginx/sites-enabled/default

# Проверка конфигурации
echo "[4/5] Проверка конфигурации Nginx..."
nginx -t

# Перезапуск Nginx
echo "[5/5] Перезапуск Nginx..."
systemctl restart nginx
systemctl enable nginx

echo ""
echo "=== Настройка завершена ==="
echo ""
echo "Nginx установлен и настроен как reverse proxy для BK-Wiver Server."
echo ""
echo "Следующие шаги:"
echo "  1. Проверьте статус: systemctl status nginx"
echo "  2. Проверьте логи: journalctl -u nginx -f"
echo "  3. Откройте браузер: http://<server-ip>/"
echo ""
echo "Для настройки HTTPS (рекомендуется):"
echo "  1. Установите certbot: apt-get install -y certbot python3-certbot-nginx"
echo "  2. Получите сертификат: certbot --nginx -d your-domain.com"
echo ""
