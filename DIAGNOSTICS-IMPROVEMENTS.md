# Улучшения диагностики и логирования BK-Wiver

## Дата: 2026-03-24

## Проблема
Хост подключился к серверу (signal channel), но устройство не отображается в веб-интерфейсе администратора.

## Внесённые изменения

### 1. Увеличен таймаут offline устройства

**Файлы:**
- `Server/app/src/server.rs:25`
- `Server/update/apps/server/src/server.rs:25`

**Изменение:**
```rust
// Было:
const DEVICE_OFFLINE_AFTER_MS: i64 = (DEVICE_HEARTBEAT_INTERVAL_SEC as i64) * 3_000; // 45 секунд

// Стало:
const DEVICE_OFFLINE_AFTER_MS: i64 = 120_000; // 2 минуты
```

**Причина:** 45 секунд слишком мало для нестабильных сетей. Устройство могло считаться offline при временных проблемах со связью.

---

### 2. Увеличен read_timeout WebSocket на хосте

**Файл:** `Host/app/src/signal.rs:73`

**Изменение:**
```rust
// Было:
let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));

// Стало:
let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
```

**Причина:** 250мс вызывало ложные разрывы соединения при временных задержках сети.

---

### 3. Добавлена обработка Pong сообщений

**Файл:** `Host/app/src/signal.rs:103`

**Изменение:**
```rust
Ok(Message::Pong(_)) => {}
```

**Причина:** Поддержка keep-alive механизма WebSocket.

---

### 4. Логирование регистрации устройства

**Файлы:**
- `Server/app/src/server.rs:897`
- `Server/update/apps/server/src/server.rs:897`

**Лог:**
```
[REGISTER] Registering new device: device_id=001 hostname=ADM token_prefix=device_6f4a9266da1d4836
```

---

### 5. Логирование heartbeat

**Файлы:**
- `Server/app/src/server.rs:1057-1069`
- `Server/update/apps/server/src/server.rs:957-969`

**Логи:**
```
[HEARTBEAT] OK device_id=001 last_seen_ms=1774351581439
[HEARTBEAT] Device ID mismatch: token_device=001 request_device=002
[HEARTBEAT] No rows affected for device_id=001
```

---

### 6. Логирование списка устройств

**Файлы:**
- `Server/app/src/server.rs:1086-1092, 1102-1109, 2298-2344`
- `Server/update/apps/server/src/server.rs:974-980, 990-997, 1967-2013`

**Логи:**
```
[FETCH_DEVICES] Online cutoff: 120000 ms ago (120 sec)
[FETCH_DEVICES] Returned 1 unblocked devices
[LIST_DEVICES] Found 1 devices for user=usr_xxx
[LIST_DEVICES] device_id=001 device_name=ADM online=true last_seen_ms=1774351581439
[LIST_ADMIN_DEVICES] Found 1 devices for admin=adm_xxx
[LIST_ADMIN_DEVICES] device_id=001 device_name=ADM online=true last_seen_ms=1774351581439
```

---

### 7. Логирование авторизации signal

**Файлы:**
- `Server/app/src/server.rs:2107-2147`
- `Server/update/apps/server/src/server.rs:2107-2147`

**Логи:**
```
[SIGNAL AUTH] User authorized: user_id=usr_xxx
[SIGNAL AUTH] Device authorized: device_id=001
[SIGNAL AUTH] Device blocked: device_id=001
[SIGNAL AUTH] Device token not found in DB: token_prefix=device_6f4a9266da1d4836
```

---

### 8. Логирование входа администратора

**Файлы:**
- `Server/app/src/server.rs:706-841`
- `Server/update/apps/server/src/server.rs:706-841`

**Логи:**
```
[ADMIN_LOGIN] Attempting login for admin=admin
[ADMIN_LOGIN] Existing admin found: admin_id=adm_xxx role=admin
[ADMIN_LOGIN] Creating new admin: admin_id=adm_xxx login=admin role=admin
[ADMIN_LOGIN] Success: admin_id=adm_xxx login=admin role=admin token_prefix=admin_access_xxx
[ADMIN_AUTH] Success: admin_id=adm_xxx
[ADMIN_AUTH] Failed: admin access token is invalid or expired
```

---

### 9. Логирование входа пользователя (desktop)

**Файлы:**
- `Server/app/src/server.rs:590-693`
- `Server/update/apps/server/src/server.rs:590-693`

**Логи:**
```
[DESKTOP_LOGIN] Attempting login for user=operator version=0.1.0
[DESKTOP_LOGIN] Existing user found: user_id=usr_xxx
[DESKTOP_LOGIN] Creating new user: user_id=usr_xxx login=operator role=operator
[DESKTOP_LOGIN] Success: user_id=usr_xxx login=operator token_prefix=access_xxx
[USER_AUTH] Success: user_id=usr_xxx
[USER_AUTH] Failed: access token is invalid or expired
```

---

### 10. Улучшено логирование heartbeat на хосте

**Файл:** `Host/app/src/app.rs:517`

**Лог:**
```
[DEBUG] [host.heartbeat] heartbeat sent device_id=001
```

---

## Диагностика проблем

### Сценарий 1: Устройство не видно в веб-интерфейсе

**Проверьте логи сервера:**

1. **Регистрация:**
   ```
   [REGISTER] Registering new device: device_id=001 ...
   ```
   Если нет — устройство не зарегистрировано. Переподключите хост.

2. **Heartbeat:**
   ```
   [HEARTBEAT] OK device_id=001 last_seen_ms=...
   ```
   Если нет — heartbeat не доходит. Проверьте сеть и токен.

3. **Список устройств:**
   ```
   [FETCH_DEVICES] Returned 0 unblocked devices
   ```
   Если 0 — устройство заблокировано или удалено из БД.

4. **Signal авторизация:**
   ```
   [SIGNAL AUTH] Device token not found in DB
   ```
   Если это так — токен недействителен. Перерегистрируйте устройство.

---

### Сценарий 2: Администратор не может войти

**Проверьте логи сервера:**

```
[ADMIN_LOGIN] Attempting login for admin=admin
[ADMIN_LOGIN] Existing admin found: admin_id=adm_xxx role=admin
[ADMIN_AUTH] Success: admin_id=adm_xxx
```

Если видите `Failed` или `Creating new admin` — проблема с учётной записью.

---

### Сценарий 3: Хост не может подключиться к signal

**Проверьте логи:**

**На хосте:**
```
[INFO] [signal] connected
[WARN] [signal] disconnected, reconnecting
```

**На сервере:**
```
[SIGNAL AUTH] Device authorized: device_id=001
[SIGNAL AUTH] Device token not found in DB
```

---

## SQL-запросы для диагностики

```sql
-- Проверить все устройства
SELECT device_id, device_name, device_token, blocked, online, last_seen_ms 
FROM devices 
ORDER BY last_seen_ms DESC;

-- Проверить конкретное устройство
SELECT * FROM devices WHERE device_id = '001';

-- Проверить, видно ли устройство в списке
SELECT device_id, device_name, 
       CASE WHEN online = TRUE AND last_seen_ms >= (EXTRACT(EPOCH FROM CURRENT_TIMESTAMP * 1000)::BIGINT - 120000) THEN TRUE ELSE FALSE END AS online
FROM devices
WHERE blocked = FALSE;

-- Проверить токены доступа
SELECT * FROM admin_access_tokens LIMIT 10;
SELECT * FROM access_tokens LIMIT 10;

-- Проверить администраторов
SELECT * FROM admins;

-- Проверить пользователей
SELECT * FROM users;
```

---

## Рекомендации по развёртыванию

1. **При первом запуске:**
   - Войдите как администратор (логин/пароль по умолчанию: `admin`/`admin` или создаётся автоматически)
   - Создайте enrollment token
   - Зарегистрируйте хост с этим токеном

2. **При проблемах с подключением:**
   - Проверьте логи сервера на наличие `[HEARTBEAT] OK`
   - Проверьте, что устройство не заблокировано
   - Перерегистрируйте хост

3. **Для сбора диагностической информации:**
   - Включите логирование сервера в файл
   - Сохраните логи за последние 5 минут
   - Выполните SQL-запросы выше

---

## Следующие шаги

1. Пересобрать сервер с новыми изменениями
2. Пересобрать хост с новыми изменениями
3. Развернуть на тестовом окружении
4. Проверить полный цикл подключения
5. Проверить отображение устройства в веб-интерфейсе
