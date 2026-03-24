# Подключение к серверу BK-Wiver

## Сервер
- **IP адрес:** `172.16.100.164`
- **Домен:** `http://wiver.bk.local/`
- **Порт:** 80 (HTTP)

## 1. Настройка DNS (локально)

Добавьте запись в файл `hosts` на вашем компьютере:

### Windows (`C:\Windows\System32\drivers\etc\hosts`)
```
172.16.100.164  wiver.bk.local
```

### Linux/macOS (`/etc/hosts`)
```bash
sudo sh -c 'echo "172.16.100.164  wiver.bk.local" >> /etc/hosts'
```

## 2. Проверка доступности

```bash
# Проверка ping
ping wiver.bk.local

# Проверка health check
curl http://wiver.bk.local/healthz
```

Ожидаемый ответ:
```json
{"ok":true,"now_ms":1234567890}
```

## 3. Доступ к админ-панели

Откройте в браузере: **http://wiver.bk.local/admin**

### Первый вход
При первом входе создаётся новый аккаунт администратора:
- **Login:** любой (например, `admin`)
- **Password:** любой (пароли пока не проверяются)

## 4. Развёртывание на сервере

Если сервер ещё не настроен, выполните на сервере `172.16.100.164`:

```bash
# Перейдите в директорию проекта
cd /opt/bk-wiver

# Сделайте скрипт исполняемым
chmod +x deploy-to-wiver.sh

# Запустите развёртывание
sudo ./deploy-to-wiver.sh
```

## 5. Проверка после развёртывания

```bash
# Статус контейнеров
docker compose -f docker-compose.nginx.yml ps

# Логи сервера
docker compose -f docker-compose.nginx.yml logs -f server

# Проверка Nginx
systemctl status nginx
```

## 6. Подключение Host (агента)

Для подключения устройства к серверу:

1. В админ-панели создайте токен регистрации:
   - Вкладка **Tokens** → **Create Token**
   - Скопируйте токен

2. На устройстве запустите Host с токеном:
   ```
   bk-wiver-host.exe --server http://wiver.bk.local/ --token <ваш-токен>
   ```

## 7. API Endpoints

| Endpoint | URL |
|----------|-----|
| Health check | `http://wiver.bk.local/healthz` |
| Admin panel | `http://wiver.bk.local/admin` |
| Devices API | `http://wiver.bk.local/api/v1/admin/devices` |
| Inventory API | `http://wiver.bk.local/api/v1/admin/inventory/history` |

## Решение проблем

### Не resolves домен
```bash
# Проверьте hosts файл
cat /etc/hosts  # Linux/macOS
type C:\Windows\System32\drivers\etc\hosts  # Windows
```

### Ошибка подключения
```bash
# Проверьте доступность сервера
ping 172.16.100.164

# Проверьте порт
telnet 172.16.100.164 80
```

### Сервер не отвечает
```bash
# На сервере проверьте статус
sudo systemctl status nginx
sudo docker compose -f docker-compose.nginx.yml ps
```
