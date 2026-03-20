# Развертывание на Ubuntu Server 24.04

Эта инструкция описывает текущий практический способ поднять сервер BK-Wiver в локальной сети на `Ubuntu Server 24.04 LTS x86_64`.

Сценарий:

- сервер стоит в локальной сети, например `192.168.1.10`;
- API доступен по `http://192.168.1.10:8080`;
- PostgreSQL и сервер запускаются через `docker compose`;
- `coturn`, `nginx`, публичный TLS и интернет-публикация пока не обязательны.

Готовые шаблоны в репозитории:

- `deploy/ubuntu/env.lan.example`
- `deploy/ubuntu/env.lan.nginx.example`
- `deploy/ubuntu/docker-compose.lan.yml`
- `deploy/ubuntu/systemd/bk-wiver.service`
- `deploy/ubuntu/nginx/bk-wiver-lan.conf`
- `deploy/ubuntu/nginx/bk-wiver-lan-selfsigned.conf`
- `deploy/ubuntu/scripts/up-lan.sh`
- `deploy/ubuntu/scripts/logs-lan.sh`
- `deploy/ubuntu/scripts/install-systemd.sh`
- `deploy/ubuntu/scripts/install-nginx-http.sh`
- `deploy/ubuntu/scripts/install-nginx-selfsigned.sh`

## 1. Что нужно на сервере

Установи базовые пакеты:

```bash
sudo apt update
sudo apt install -y ca-certificates curl git ufw
```

Установи Docker Engine и Compose plugin удобным для тебя способом.
После установки проверь:

```bash
docker --version
docker compose version
```

## 2. Подготовка каталога приложения

Создай рабочий каталог и перенеси туда проект:

```bash
sudo mkdir -p /opt/bk-wiver
sudo chown $USER:$USER /opt/bk-wiver
cd /opt/bk-wiver
```

Если репозиторий уже есть локально, достаточно скопировать его в `/opt/bk-wiver`.
Если будешь клонировать:

```bash
git clone <URL-репозитория> /opt/bk-wiver
cd /opt/bk-wiver
```

## 3. Настройка `.env`

Создай локальный `.env`:

```bash
cp deploy/ubuntu/env.lan.example .env
```

Пример для локальной сети:

```dotenv
POSTGRES_DB=bkwiver
POSTGRES_USER=postgres
POSTGRES_PASSWORD=change-me
POSTGRES_PORT=5432
SERVER_PORT=8080
SERVER_PUBLIC_URL=http://192.168.1.10:8080
RUST_LOG=info,tower_http=info
```

Важно:

- `SERVER_PUBLIC_URL` должен содержать реальный IP сервера в локальной сети, а не `localhost`;
- если IP сервера другой, замени `192.168.1.10` на свой адрес;
- если PostgreSQL не должен быть доступен с других машин, можно позже убрать проброс `5432` из `docker-compose.yml`.

Узнать IP сервера можно так:

```bash
ip addr show
hostname -I
```

## 4. Запуск сервера

Из корня проекта:

```bash
docker compose -f docker-compose.yml -f deploy/ubuntu/docker-compose.lan.yml up --build -d
```

Или через готовый скрипт:

```bash
bash deploy/ubuntu/scripts/up-lan.sh
```

Проверка состояния:

```bash
docker compose -f docker-compose.yml -f deploy/ubuntu/docker-compose.lan.yml ps
docker compose -f docker-compose.yml -f deploy/ubuntu/docker-compose.lan.yml logs -f server
docker compose -f docker-compose.yml -f deploy/ubuntu/docker-compose.lan.yml logs -f postgres
```

Если установлен `just`, можно использовать:

```bash
just up-lan
just ps-lan
just logs-lan
```

Что даёт `deploy/ubuntu/docker-compose.lan.yml`:

- убирает внешний проброс PostgreSQL;
- оставляет наружу только HTTP API сервера;
- позволяет ограничить bind-адрес через `SERVER_BIND_IP`.

## 5. Проверка доступности в локальной сети

Проверь локально на сервере:

```bash
curl http://127.0.0.1:8080/healthz
```

Проверь по LAN-адресу:

```bash
curl http://192.168.1.10:8080/healthz
```

Ожидается JSON вида:

```json
{"ok":true,"now_ms":1770000000000}
```

С другой машины в той же сети проверь:

```bash
curl http://192.168.1.10:8080/healthz
```

Если ответ есть, сервер доступен по локальной сети.

## 6. Настройка firewall

Если используется `ufw`, открой только нужный порт API:

```bash
sudo ufw allow 8080/tcp
sudo ufw enable
sudo ufw status
```

Если не нужен внешний доступ к PostgreSQL, не открывай `5432/tcp`.

## 7. Автозапуск после перезагрузки

Контейнеры уже используют `restart: unless-stopped`, но удобнее добавить `systemd` unit для `docker compose`.

Скопируй готовый unit:

```bash
bash deploy/ubuntu/scripts/install-systemd.sh
```

Затем:

```bash
sudo systemctl status bk-wiver.service
```

## 8. Локальный reverse proxy через nginx

Если хочешь открыть API по `http://192.168.1.10/` без указания порта `8080`, поставь `nginx`:

```bash
bash deploy/ubuntu/scripts/install-nginx-http.sh
```

После этого:

- HTTP API будет доступен по `http://192.168.1.10/`;
- websocket signaling будет проксироваться через `http://192.168.1.10/ws/v1/signal`;
- внутри сервера приложение по-прежнему может слушать `127.0.0.1:8080` или `0.0.0.0:8080`.

Если используешь `nginx`, можно в `.env` указать:

```dotenv
SERVER_PUBLIC_URL=http://192.168.1.10
SERVER_BIND_IP=127.0.0.1
```

Тогда сам контейнер сервера не будет доступен напрямую по сети, а только через reverse proxy.

Для такого варианта удобнее сразу взять отдельный шаблон:

```bash
cp deploy/ubuntu/env.lan.nginx.example .env
```

После этого замени `bk-wiver.lan` на свой локальный DNS-имя или оставь его как есть и добавь запись в DNS/`hosts`.

## 9. Self-signed HTTPS/WSS в локальной сети

Если нужен защищённый websocket и HTTPS внутри LAN без внешнего CA, можно использовать self-signed сертификат.

Сгенерируй сертификат:

```bash
bash deploy/ubuntu/scripts/install-nginx-selfsigned.sh bk-wiver.lan
```

Обнови `.env`:

```dotenv
SERVER_PUBLIC_URL=https://bk-wiver.lan
SERVER_BIND_IP=127.0.0.1
```

После этого:

- HTTP будет редиректиться на HTTPS;
- websocket нужно открывать как `wss://bk-wiver.lan/ws/v1/signal`;
- клиентам придётся доверить self-signed сертификат вручную.

## 10. Обновление сервера

Когда код изменится:

```bash
cd /opt/bk-wiver
git pull
docker compose -f docker-compose.yml -f deploy/ubuntu/docker-compose.lan.yml up --build -d
```

Или:

```bash
bash deploy/ubuntu/scripts/up-lan.sh
```

Если репозиторий не клонируется, а копируется вручную, обнови файлы проекта и снова выполни:

```bash
docker compose -f docker-compose.yml -f deploy/ubuntu/docker-compose.lan.yml up --build -d
```

## 11. Что важно для текущего MVP

На текущем этапе сервер:

- не поднимает `HTTPS`;
- не поднимает `WSS`;
- не включает `coturn`;
- не публикует отдельную web-панель;
- работает как control-plane API поверх `Axum + PostgreSQL`.

Для локальной сети этого достаточно, чтобы:

- проверить `healthz`;
- логинить desktop-пользователей и админов;
- регистрировать устройства;
- создавать enrollment tokens;
- смотреть устройства, пользователей и audit;
- создавать remote sessions на уровне control-plane.

## 12. Рекомендуемые следующие шаги

Перед более широким использованием в сети стоит сделать:

1. убрать проброс PostgreSQL наружу, если он не нужен;
2. вынести пароль БД из простого примера в отдельный секрет;
3. добавить reverse proxy перед сервером;
4. добавить TLS, если сервер будет использоваться не только в доверенной LAN;
5. позже добавить `coturn` и signaling websocket.
