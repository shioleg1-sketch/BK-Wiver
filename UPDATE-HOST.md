# Инструкция по обновлению BK-Host

## Дата: 2026-03-24

---

## 🚨 Текущая проблема

Хост зарегистрирован и отправляет heartbeat, но signal channel постоянно переподключается:

```
[WARN] [signal] disconnected, reconnecting
```

**Причина:** На Windows установлена старая версия BK-Host без исправлений таймаутов.

---

## ✅ Что исправлено в новой версии

1. **Увеличен read_timeout WebSocket** с 250мс до 10 секунд
2. **Добавлена обработка Pong сообщений** для keep-alive
3. **Улучшено логирование** heartbeat

---

## 📦 Сборка новой версии

### На macOS (для Windows)

```bash
cd /opt/bk-wiver

# Сборка Windows версии
cargo build --release -p bk-wiver-host --target x86_64-pc-windows-msvc

# Копирование ffmpeg
cp scripts/ffmpeg.exe target/x86_64-pc-windows-msvc/release/

# Создание установщика
pwsh -File ./scripts/prepare_windows_bundle.ps1 \
  -HostExe ./target/x86_64-pc-windows-msvc/release/bk-wiver-host.exe \
  -ConsoleExe ./target/x86_64-pc-windows-msvc/release/bk-wiver-console.exe

# Сборка installer
& "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" "Host/installer/windows/BK-Wiver-Host.iss"
```

### Готовые артефакты

- `Host/dist/BK-Host-Setup.exe` — установщик
- `target/x86_64-pc-windows-msvc/release/bk-wiver-host.exe` — portable версия

---

## 🖥️ Установка на Windows

### Вариант A: Через установщик

1. Скопируйте `BK-Host-Setup.exe` на Windows ПК
2. Запустите установщик
3. После установки BK-Host автоматически переподключится

### Вариант B: Portable версия

1. Остановите текущий BK-Host:
   ```powershell
   Stop-Process -Name "bk-wiver-host" -Force
   ```

2. Замените исполняемый файл:
   ```powershell
   Copy-Item bk-wiver-host.exe "C:\Program Files\BK-Host\" -Force
   ```

3. Запустите заново:
   ```powershell
   Start-Process "C:\Program Files\BK-Host\bk-wiver-host.exe"
   ```

---

## 🔍 Проверка после обновления

### Логи хоста

Откройте `C:\Users\oleg\AppData\Local\BK-Wiver\state\host-runtime.log`

**Ожидаемые логи:**
```
[INFO] [signal] connected
[DEBUG] [host.heartbeat] heartbeat sent device_id=001
```

**Не должно быть:**
```
[WARN] [signal] disconnected, reconnecting  ← каждые 2 секунды
```

### Логи сервера

```bash
docker logs bk-wiver-server 2>&1 | grep -E "\[(SIGNAL|HEARTBEAT)\]"
```

**Ожидаемые логи:**
```
[SIGNAL AUTH] Device authorized: device_id=001
[HEARTBEAT] OK device_id=001 last_seen_ms=...
```

### Веб-интерфейс

1. Откройте http://172.16.100.164/admin
2. Устройство должно быть **online**
3. Статус signal должен быть **connected** (не reconnecting)

---

## 🐛 Если проблема сохраняется

### Проверка сети

```powershell
# С Windows ПК
Test-NetConnection 172.16.100.164 -Port 8080
ping 172.16.100.164
```

### Проверка DNS

Хост использует `http://wiver.bk.local`, но DNS может не работать.

**Решение:** Обновите `device-registration.json`:

```json
{
  "serverUrl": "http://172.16.100.164:8080"
}
```

Или добавьте в `C:\Windows\System32\drivers\etc\hosts`:
```
172.16.100.164    wiver.bk.local
```

### Перерегистрация хоста

1. Удалите файл регистрации:
   ```powershell
   Remove-Item "C:\Users\oleg\AppData\Local\BK-Wiver\state\device-registration.json"
   ```

2. Перезапустите BK-Host

3. Введите enrollment token заново:
   ```
   enroll_36ed104013904fa892af496551ac0398
   ```

---

## 📊 Диагностика

### Команды для проверки

**На сервере:**
```bash
# Статус устройства
curl http://172.16.100.164/api/v1/admin/devices \
  -H "Authorization: Bearer <token>" | jq '.devices[] | {deviceId, online, lastSeenMs}'

# Логи signal
docker logs bk-wiver-server 2>&1 | grep "SIGNAL AUTH"

# Логи heartbeat
docker logs bk-wiver-server 2>&1 | grep "HEARTBEAT"
```

**На хосте:**
```powershell
# Проверка файла регистрации
Get-Content "C:\Users\oleg\AppData\Local\BK-Wiver\state\device-registration.json" | ConvertFrom-Json

# Последние логи
Get-Content "C:\Users\oleg\AppData\Local\BK-Wiver\state\host-runtime.log" -Tail 50
```

---

## 📞 Контакты

При проблемах предоставьте:

1. Версию BK-Host (из UI или `--version`)
2. Последние 50 строк лога хоста
3. Результат `Test-NetConnection 172.16.100.164 -Port 8080`
