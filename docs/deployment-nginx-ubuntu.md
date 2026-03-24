# Развёртывание BK-Wiver Server с Nginx на Ubuntu Server 24.04

## Обзор

В этом руководстве описан полный процесс развёртывания BK-Wiver Server с Nginx в качестве reverse proxy на Ubuntu Server 24.04.

## Требования

- Ubuntu Server 24.04 LTS (x86_64)
- Минимум 2 GB RAM
- 10 GB свободного места на диске
- Доступ к интернету для установки зависимостей
- Доменное имя (опционально, для HTTPS)

## Быстрый старт

### 1. Подготовка сервера

```bash
# Обновление пакетов
sudo apt update && sudo apt upgrade -y

# Установка базовых зависимостей
sudo apt install -y ca-certificates curl git ufw

# Настройка фаервола
sudo ufw allow OpenSSH
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp
sudo ufw enable
```

### 2. Установка Docker

```bash
# Установка Docker
curl -fsSL https://get.docker.com | sh

# Добавление пользователя в группу docker
sudo usermod -aG docker $USER

# Применение изменений (выйдите и зайдите снова)
newgrp docker
```

### 3. Клонирование проекта

```bash
# Клонирование репозитория
git clone git@github.com:shioleg1-sketch/BK-Wiver.git /opt/bk-wiver
cd /opt/bk-wiver
```

### 4. Настройка окружения

Создайте файл `.env` с вашими параметрами:

```bash
# PostgreSQL
POSTGRES_DB=bkwiver
POSTGRES_USER=postgres
POSTGRES_PASSWORD=<ваш_надёжный_пароль>
POSTGRES_PORT=5432

# Server
SERVER_PORT=8080
SERVER_PUBLIC_URL=http://<ваш-IP-или-домен>:8080

# Logging
RUST_LOG=info,tower_http=info

# Nginx (если используется)
NGINX_HTTP_PORT=80
NGINX_HTTPS_PORT=443
```

### 5. Установка и настройка Nginx

```bash
# Запуск скрипта установки
chmod +x setup-nginx-ubuntu.sh
sudo ./setup-nginx-ubuntu.sh
```

### 6. Запуск сервера

```bash
# Запуск с Nginx
sudo docker compose -f docker-compose.nginx.yml up -d --build

# Проверка статуса
sudo docker compose -f docker-compose.nginx.yml ps

# Просмотр логов
sudo docker compose -f docker-compose.nginx.yml logs -f server
```

## Проверка работы

### Health check

```bash
# Локально
curl http://127.0.0.1:8080/healthz

# Через Nginx
curl http://<ваш-IP>/healthz
```

Ожидаемый ответ:
```json
{"ok":true,"now_ms":1234567890}
```

### Доступ к админ-панели

Откройте в браузере: `http://<ваш-IP>/admin`

## Настройка HTTPS (рекомендуется)

### Получение сертификата Let's Encrypt

```bash
# Установка certbot
sudo apt install -y certbot python3-certbot-nginx

# Получение сертификата
sudo certbot --nginx -d your-domain.com

# Автоматическое обновление сертификатов
sudo systemctl enable certbot.timer
sudo systemctl start certbot.timer
```

### Обновление конфигурации Nginx

Отредактируйте `deploy/nginx/nginx.conf` и раскомментируйте секцию HTTPS server.

## API Endpoints

### Публичные endpoints

| Endpoint | Метод | Описание |
|----------|-------|----------|
| `/healthz` | GET | Проверка здоровья сервера |
| `/admin` | GET | Web-интерфейс администратора |

### API для устройств (Host)

| Endpoint | Метод | Описание |
|----------|-------|----------|
| `/api/v1/devices/register` | POST | Регистрация устройства |
| `/api/v1/devices/heartbeat` | POST | Отправка телеметрии |

### Admin API

| Endpoint | Метод | Описание |
|----------|-------|----------|
| `/api/v1/admin/auth/login` | POST | Вход администратора |
| `/api/v1/admin/devices` | GET | Список устройств |
| `/api/v1/admin/devices/{id}/inventory` | GET | История инвентаря устройства |
| `/api/v1/admin/inventory/history` | GET | Общая история инвентаря |
| `/api/v1/admin/users` | GET | Список пользователей |
| `/api/v1/admin/audit` | GET | Audit log |
| `/api/v1/admin/enrollment-tokens` | GET/POST | Токены регистрации |

## Управление сервисами

```bash
# Остановка
sudo docker compose -f docker-compose.nginx.yml down

# Перезапуск
sudo docker compose -f docker-compose.nginx.yml restart

# Пересборка и перезапуск
sudo docker compose -f docker-compose.nginx.yml up -d --build

# Просмотр логов
sudo docker compose -f docker-compose.nginx.yml logs -f

# Остановка только сервера
sudo docker compose -f docker-compose.nginx.yml stop server
```

## Обновление

```bash
cd /opt/bk-wiver

# Получение обновлений
git pull

# Пересборка и перезапуск
sudo docker compose -f docker-compose.nginx.yml up -d --build
```

## Мониторинг

### Статус сервисов

```bash
# Docker контейнеры
sudo docker ps

# Nginx статус
sudo systemctl status nginx

# Использование ресурсов
sudo docker stats
```

### Логи

```bash
# Server логи
sudo docker compose -f docker-compose.nginx.yml logs server

# Nginx логи доступа
sudo tail -f /var/log/nginx/access.log

# Nginx логи ошибок
sudo tail -f /var/log/nginx/error.log
```

## Безопасность

### Рекомендации

1. **Всегда используйте HTTPS** в production
2. **Смените пароль PostgreSQL** по умолчанию
3. **Настройте fail2ban** для защиты от brute-force
4. **Регулярно обновляйте** систему и пакеты
5. **Используйте надёжные токены** для регистрации устройств

### Fail2ban (опционально)

```bash
sudo apt install -y fail2ban

# Создание конфигурации для Nginx
sudo cat > /etc/fail2ban/jail.local << EOF
[nginx-http-auth]
enabled = true
port = http,https
filter = nginx-http-auth
logpath = /var/log/nginx/error.log
maxretry = 5
bantime = 3600
EOF

sudo systemctl restart fail2ban
```

## Решение проблем

### Сервер не запускается

```bash
# Проверка логов Docker
sudo docker compose -f docker-compose.nginx.yml logs

# Проверка портов
sudo ss -tlnp | grep :8080
sudo ss -tlnp | grep :80
```

### Nginx не проксирует запросы

```bash
# Проверка конфигурации
sudo nginx -t

# Проверка логов
sudo tail -f /var/log/nginx/error.log

# Перезапуск Nginx
sudo systemctl restart nginx
```

### Ошибки подключения к БД

```bash
# Проверка статуса PostgreSQL
sudo docker compose -f docker-compose.nginx.yml ps postgres

# Проверка логов БД
sudo docker compose -f docker-compose.nginx.yml logs postgres
```

## Дополнительные ресурсы

- [Документация Docker Compose](https://docs.docker.com/compose/)
- [Документация Nginx](https://nginx.org/en/docs/)
- [Документация Let's Encrypt](https://letsencrypt.org/docs/)
