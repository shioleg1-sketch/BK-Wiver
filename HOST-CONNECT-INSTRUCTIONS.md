# Инструкция по подключению хоста BK-Host

## Дата: 2026-03-24

## 1. Enrollment Token для регистрации

**Token:** `enroll_36ed104013904fa892af496551ac0398`

**Срок действия:** 7 дней

**Одноразовый:** Нет (можно использовать многократно)

---

## 2. Подключение хоста (Windows)

### Вариант A: Через UI (рекомендуется)

1. Откройте **BK-Host** на Windows ПК
2. В поле **Server URL** укажите: `http://172.16.100.164:8080`
3. В поле **Enrollment Token** вставьте: `enroll_36ed104013904fa892af496551ac0398`
4. Нажмите кнопку **Connect** (Подключиться)
5. Дождитесь сообщения "Host зарегистрирован на сервере и готов к подключениям"

### Вариант B: Через PowerShell (автоматически)

```powershell
# Путь к исполняемому файлу BK-Host
$hostExe = "C:\Program Files\BK-Host\bk-wiver-host.exe"

# Запуск с параметрами (если поддерживается)
& $hostExe --server http://172.16.100.164:8080 --token enroll_36ed104013904fa892af496551ac0398
```

---

## 3. Проверка регистрации на сервере

### Через API

```bash
# Получить список устройств
curl -s http://172.16.100.164:8080/api/v1/admin/devices \
  -H "Authorization: Bearer <admin_token>" | jq

# Ожидаемый ответ:
# {"devices":[{"deviceId":"001","deviceName":"ADM-PC","online":true,...}]}
```

### Через логи сервера

```bash
# Следить за логами в реальном времени
docker logs bk-wiver-server -f

# Искать логи регистрации:
# [REGISTER] Registering new device: device_id=001 hostname=ADM-PC
# [HEARTBEAT] OK device_id=001 last_seen_ms=...
# [SIGNAL AUTH] Device authorized: device_id=001
```

### Через SQL

```sql
-- Проверить все устройства
SELECT device_id, device_name, online, last_seen_ms 
FROM devices 
ORDER BY last_seen_ms DESC;

-- Проверить конкретное устройство
SELECT * FROM devices WHERE device_id = '001';
```

---

## 4. Веб-интерфейс администратора

**URL:** `http://172.16.100.164:8080/admin`

**Логин:** `admin`  
**Пароль:** `admin` (создаётся автоматически при первом входе)

### Что проверить в интерфейсе:

1. **Устройства** — должно появиться новое устройство
2. **Статус** — `online` (зелёный значок)
3. **Last Seen** — должно быть "just now" или "< 1m ago"
4. **Connect Code** — 6-значный код для подключения операторов

---

## 5. Диагностика проблем

### Хост не регистрируется

**Проверьте логи хоста:**
- Windows: `C:\Users\<user>\AppData\Local\BK-Wiver\state\host-runtime.log`
- Ищите: `[INFO] [host.connect]`, `[WARN]`, `[ERROR]`

**Проверьте логи сервера:**
```bash
docker logs bk-wiver-server 2>&1 | grep -E "\[(REGISTER|HEARTBEAT|ADMIN_LOGIN)\]"
```

**Проверьте сеть:**
```powershell
# С Windows хоста
Test-NetConnection 172.16.100.164 -Port 8080
```

### Хост зарегистрирован, но offline

**Причины:**
- Heartbeat не доходит (проверьте сеть)
- Устройство заблокировано
- Прошло больше 2 минут с последнего heartbeat

**Решение:**
```sql
-- Проверить статус
SELECT device_id, online, last_seen_ms, blocked 
FROM devices 
WHERE device_id = '001';

-- Разблокировать если нужно
UPDATE devices SET blocked = FALSE WHERE device_id = '001';
```

### Хост подключается к signal, но не виден в списке

**Новые логи диагностики покажут:**

```
[SIGNAL AUTH] Device authorized: device_id=001     ← signal работает
[FETCH_DEVICES] Returned 0 unblocked devices       ← устройство не видно
[LIST_ADMIN_DEVICES] Found 0 devices for admin=... ← пусто в интерфейсе
```

**Причины:**
- Устройство заблокировано (`blocked = TRUE`)
- `last_seen_ms` старше 2 минут
- Проблема с запросом списка устройств

**Проверка:**
```sql
SELECT device_id, device_name, blocked, online, 
       last_seen_ms,
       EXTRACT(EPOCH FROM CURRENT_TIMESTAMP * 1000)::BIGINT - last_seen_ms AS ms_ago
FROM devices;
```

---

## 6. Команды для оператора

### Подключение к хосту по Connect Code

1. Откройте **BK-Console** (оператор)
2. Введите 6-значный код из веб-интерфейса
3. Нажмите **Connect**

### Подключение по Device ID

1. Откройте **BK-Console**
2. Выберите устройство из списка
3. Нажмите **Connect**

---

## 7. Логи для отладки

### Сервер (Ubuntu)

```bash
# Все логи
docker logs bk-wiver-server

# Только диагностика
docker logs bk-wiver-server 2>&1 | grep -E "\[(REGISTER|HEARTBEAT|FETCH|LIST|SIGNAL|LOGIN|AUTH)\]"

# В реальном времени
docker logs bk-wiver-server -f
```

### Хост (Windows)

```
# Расположение логов
C:\Users\<user>\AppData\Local\BK-Wiver\state\host-runtime.log

# Экспорт логов
# Кнопка "Сохранить лог" в UI BK-Host
```

---

## 8. Контакты для поддержки

При проблемах с подключением предоставьте:

1. Логи сервера за последние 5 минут
2. Логи хоста (`host-runtime.log`)
3. Результат `curl http://172.16.100.164:8080/healthz`
4. SQL-дамп таблицы `devices`
