# BK-Wiver Server - Инструкция по эксплуатации

## Дата обновления: 2026-03-24

---

## 🚀 Быстрый старт

### 1. Доступ к серверу

**Админ-панель:**
- URL: http://172.16.100.164/admin
- Логин: `admin`
- Пароль: `admin`

**Healthcheck:**
- URL: http://172.16.100.164/healthz

---

## 📋 Текущее состояние

### Сервисы

| Сервис | Статус | Порт |
|--------|--------|------|
| bk-wiver-server | ✅ Работает | 8080 (внутренний) |
| bk-wiver-postgres | ✅ Работает (healthy) | 5432 |
| bk-wiver-nginx | ✅ Работает | 80, 443 |

### Учётные данные

**Администратор:**
```
Login: admin
Password: admin
```

**Enrollment Token (для регистрации хостов):**
```
enroll_36ed104013904fa892af496551ac0398
```
Срок действия: 7 дней

---

## 🖥️ Подключение хоста (Windows)

### Шаг 1: Настройка BK-Host

1. Откройте **BK-Host** на Windows ПК
2. Заполните поля:
   - **Server URL:** `http://172.16.100.164:8080`
   - **Enrollment Token:** `enroll_36ed104013904fa892af496551ac0398`
3. Нажмите **Connect**

### Шаг 2: Проверка регистрации

**Через веб-интерфейс:**
1. Откройте http://172.16.100.164/admin
2. Войдите как `admin`
3. Перейдите на вкладку **Devices**
4. Устройство должно появиться со статусом **online**

**Через API:**
```bash
curl http://172.16.100.164/api/v1/admin/devices \
  -H "Authorization: Bearer <token>"
```

**Через логи:**
```bash
docker logs bk-wiver-server -f 2>&1 | grep -E "\[(REGISTER|HEARTBEAT|SIGNAL)\]"
```

Ожидаемые логи:
```
[REGISTER] Registering new device: device_id=001 hostname=ADM-PC
[HEARTBEAT] OK device_id=001 last_seen_ms=1774354018817
[SIGNAL AUTH] Device authorized: device_id=001
```

---

## 🔧 Управление

### Перезапуск сервера

```bash
cd /opt/bk-wiver
docker compose restart
```

### Просмотр логов

```bash
# Все логи
docker logs bk-wiver-server

# Логи в реальном времени
docker logs bk-wiver-server -f

# Только диагностика
docker logs bk-wiver-server 2>&1 | grep -E "\[(REGISTER|HEARTBEAT|FETCH|LIST|SIGNAL|LOGIN|AUTH)\]"
```

### Создание enrollment token

```bash
# Войти как администратор
TOKEN=$(curl -s -X POST http://172.16.100.164/api/v1/admin/auth/login \
  -H "Content-Type: application/json" \
  -d '{"login":"admin","password":"admin"}' | jq -r '.accessToken')

# Создать token
curl -X POST http://172.16.100.164/api/v1/admin/enrollment-tokens \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"comment":"New token","expiresAtMs":1774958400000,"singleUse":false}'
```

---

## 📊 Диагностика

### Таблицы диагностических логов

| Префикс | Описание | Пример |
|---------|----------|--------|
| `[REGISTER]` | Регистрация нового устройства | `device_id=001 hostname=ADM-PC` |
| `[HEARTBEAT]` | Heartbeat от устройства | `OK device_id=001 last_seen_ms=...` |
| `[FETCH_DEVICES]` | Запрос списка устройств | `Returned 1 unblocked devices` |
| `[LIST_DEVICES]` | Список для пользователя | `device_id=001 online=true` |
| `[LIST_ADMIN_DEVICES]` | Список для администратора | `device_id=001 online=true` |
| `[SIGNAL AUTH]` | Авторизация WebSocket | `Device authorized: device_id=001` |
| `[DESKTOP_LOGIN]` | Вход пользователя | `Success: user_id=usr_xxx` |
| `[ADMIN_LOGIN]` | Вход администратора | `Success: admin_id=adm_xxx` |
| `[USER_AUTH]` | Авторизация токена пользователя | `Success: user_id=usr_xxx` |
| `[ADMIN_AUTH]` | Авторизация токена администратора | `Success: admin_id=adm_xxx` |

### SQL-запросы для диагностики

```bash
# Подключиться к PostgreSQL
docker exec -it bk-wiver-postgres psql -U postgres -d bkwiver
```

```sql
-- Все устройства
SELECT device_id, device_name, online, last_seen_ms, blocked 
FROM devices 
ORDER BY last_seen_ms DESC;

-- Проверить конкретное устройство
SELECT * FROM devices WHERE device_id = '001';

-- Устройства, которые не отправляли heartbeat больше 2 минут
SELECT device_id, device_name, 
       EXTRACT(EPOCH FROM CURRENT_TIMESTAMP * 1000)::BIGINT - last_seen_ms AS ms_ago
FROM devices
WHERE last_seen_ms < EXTRACT(EPOCH FROM CURRENT_TIMESTAMP * 1000)::BIGINT - 120000;

-- Разблокировать устройство
UPDATE devices SET blocked = FALSE WHERE device_id = '001';

-- Удалить устройство
DELETE FROM devices WHERE device_id = '001';
```

---

## 🐛 Решение проблем

### Проблема: 502 Bad Gateway

**Причина:** nginx не видит сервер

**Решение:**
```bash
cd /opt/bk-wiver
docker compose -f docker-compose.nginx.yml restart
```

### Проблема: Устройство не видно в списке

**Проверьте логи:**
```bash
docker logs bk-wiver-server 2>&1 | grep -E "\[(REGISTER|HEARTBEAT|FETCH|LIST)\]"
```

**Ожидаемые логи:**
```
[REGISTER] Registering new device: device_id=001
[HEARTBEAT] OK device_id=001
[FETCH_DEVICES] Returned 1 unblocked devices
[LIST_ADMIN_DEVICES] device_id=001 online=true
```

**Если устройство не регистрируется:**
1. Проверьте сеть между хостом и сервером
2. Проверьте, что enrollment token действителен
3. Пересоздайте token

**Если heartbeat не доходит:**
1. Проверьте firewall на сервере
2. Проверьте логи хоста
3. Перезапустите службу BK-Host на Windows

### Проблема: Хост подключился, но offline

**Причины:**
- Heartbeat не доходит (сеть)
- Прошло больше 2 минут с последнего heartbeat
- Устройство заблокировано

**Проверка:**
```sql
SELECT device_id, online, last_seen_ms, blocked,
       EXTRACT(EPOCH FROM CURRENT_TIMESTAMP * 1000)::BIGINT - last_seen_ms AS ms_ago
FROM devices;
```

**Решение:**
```sql
-- Разблокировать
UPDATE devices SET blocked = FALSE WHERE device_id = '001';

-- Принудительно пометить online
UPDATE devices SET online = TRUE, last_seen_ms = EXTRACT(EPOCH FROM CURRENT_TIMESTAMP * 1000)::BIGINT WHERE device_id = '001';
```

---

## 📁 Файлы

### Расположение

**Сервер (Ubuntu):**
- Конфигурация: `/opt/bk-wiver/.env`
- Логи: `docker logs bk-wiver-server`
- База данных: Docker volume `bk-wiver_bk_wiver_postgres_data`

**Хост (Windows):**
- Регистрация: `C:\Users\<user>\AppData\Local\BK-Wiver\state\device-registration.json`
- Логи: `C:\Users\<user>\AppData\Local\BK-Wiver\state\host-runtime.log`
- Статус: `C:\Users\<user>\AppData\Local\BK-Wiver\state\agent-status.json`

### Важные файлы

- `DIAGNOSTICS-IMPROVEMENTS.md` — описание улучшений диагностики
- `HOST-CONNECT-INSTRUCTIONS.md` — инструкция по подключению хоста
- `SERVER-OPERATION.md` — этот файл

---

## 📞 Поддержка

При проблемах предоставьте:

1. **Логи сервера:**
   ```bash
   docker logs bk-wiver-server > server-log.txt
   ```

2. **Логи хоста:**
   - Кнопка "Сохранить лог" в UI BK-Host

3. **Диагностика:**
   ```bash
   curl http://172.16.100.164/healthz
   docker ps
   docker compose config
   ```

4. **SQL-дамп:**
   ```bash
   docker exec bk-wiver-postgres pg_dump -U postgres bkwiver > dump.sql
   ```
