# API сервера BK-Wiver

## Назначение

Этот документ фиксирует минимальный API для первой рабочей связки:

- `Desktop` логинится;
- `Desktop` регистрирует устройство;
- `Desktop` отправляет heartbeat;
- `Desktop` получает список устройств;
- `Desktop` создает сессию;
- `Desktop` и `Server` обмениваются signaling-сообщениями;
- администратор управляет сервером через web-интерфейс.

## Базовые принципы

- внешний API работает только по `HTTPS`;
- signaling работает по `WSS`;
- access token пользователя и device token устройства разделены;
- admin web session отделена от desktop session;
- сервер не передает пароль дальше точки логина;
- все даты и времена передаются в `unix time ms`.

## HTTP API v1

Базовый префикс:

```text
/api/v1
```

## 1. Логин пользователя

```http
POST /api/v1/auth/login
Content-Type: application/json
```

Тело запроса:

```json
{
  "login": "admin",
  "password": "secret",
  "desktopVersion": {
    "version": "0.1.0",
    "commit": "dev"
  }
}
```

Успешный ответ:

```json
{
  "userId": "usr_001",
  "accessToken": "jwt-or-random-token",
  "refreshToken": "refresh-token"
}
```

## 1.1 Логин администратора в web-интерфейс

```http
POST /api/v1/admin/auth/login
Content-Type: application/json
```

Тело запроса:

```json
{
  "login": "admin",
  "password": "secret"
}
```

Успешный ответ:

```json
{
  "adminId": "adm_001",
  "accessToken": "admin_access_token",
  "refreshToken": "admin_refresh_token"
}
```

## 2. Регистрация устройства

```http
POST /api/v1/devices/register
Content-Type: application/json
Authorization: Bearer <accessToken>
```

Тело запроса:

```json
{
  "enrollmentToken": "enroll_abc",
  "desktopVersion": {
    "version": "0.1.0",
    "commit": "dev"
  },
  "hostInfo": {
    "hostname": "work-mac",
    "os": "macos",
    "osVersion": "15.0",
    "arch": "arm64",
    "username": "oleg"
  },
  "permissions": {
    "screenCapture": true,
    "inputControl": true,
    "accessibility": true,
    "fileTransfer": true
  }
}
```

Успешный ответ:

```json
{
  "deviceId": "dev_001",
  "deviceName": "Oleg MacBook",
  "serverUrl": "https://bk.example.com",
  "deviceToken": "device_token",
  "heartbeatIntervalSec": 15
}
```

## 3. Heartbeat устройства

```http
POST /api/v1/devices/heartbeat
Content-Type: application/json
Authorization: Bearer <deviceToken>
```

Тело запроса:

```json
{
  "deviceId": "dev_001",
  "permissions": {
    "screenCapture": true,
    "inputControl": true,
    "accessibility": true,
    "fileTransfer": true
  },
  "unixTimeMs": 1770000000000
}
```

Успешный ответ:

```json
{
  "ok": true
}
```

## 4. Список устройств

```http
GET /api/v1/devices
Authorization: Bearer <accessToken>
```

Успешный ответ:

```json
{
  "devices": [
    {
      "deviceId": "dev_001",
      "deviceName": "Oleg MacBook",
      "hostInfo": {
        "hostname": "work-mac",
        "os": "macos",
        "osVersion": "15.0",
        "arch": "arm64",
        "username": "oleg"
      },
      "online": true,
      "lastSeenMs": 1770000000000,
      "permissions": {
        "screenCapture": true,
        "inputControl": true,
        "accessibility": true,
        "fileTransfer": true
      }
    }
  ]
}
```

## 5. Создание сессии

```http
POST /api/v1/sessions
Content-Type: application/json
Authorization: Bearer <accessToken>
```

Тело запроса:

```json
{
  "deviceId": "dev_001"
}
```

Успешный ответ:

```json
{
  "sessionId": "ses_001",
  "sessionToken": "session_token",
  "expiresAtMs": 1770000030000
}
```

## 6. Enrollment token

Создание enrollment token нужно для добавления нового устройства.

```http
POST /api/v1/enrollment-tokens
Content-Type: application/json
Authorization: Bearer <accessToken>
```

Тело запроса:

```json
{
  "comment": "main office",
  "expiresAtMs": 1771000000000,
  "singleUse": true
}
```

Для web-панели это должен быть отдельный admin endpoint:

```http
POST /api/v1/admin/enrollment-tokens
Authorization: Bearer <adminAccessToken>
```

## 7. Аудит

```http
GET /api/v1/audit?limit=100
Authorization: Bearer <accessToken>
```

Для административного интерфейса:

```http
GET /api/v1/admin/audit?limit=100
Authorization: Bearer <adminAccessToken>
```

Примеры событий:

- логин пользователя;
- логин администратора;
- регистрация устройства;
- создание сессии;
- отклонение подключения;
- начало и завершение remote-session.

## 8. Web admin endpoints

Минимальный набор для web-интерфейса:

```http
GET /api/v1/admin/devices
GET /api/v1/admin/users
GET /api/v1/admin/audit
POST /api/v1/admin/enrollment-tokens
PATCH /api/v1/admin/devices/{deviceId}
```

Что должен позволять web-интерфейс:

- просматривать устройства;
- видеть online/offline статус;
- отключать или блокировать устройство;
- создавать enrollment token;
- просматривать аудит;
- управлять ролями пользователей на базовом уровне.

## WebSocket signaling

Endpoint:

```text
/ws/v1/signal
```

Подключение:

```text
wss://bk.example.com/ws/v1/signal
```

Аутентификация:

- пользовательский `accessToken` для исходящих сессий;
- `deviceToken` для опубликованного устройства;
- вариант передачи: заголовок `Authorization` или query param только если клиентская библиотека ограничена.

Web-интерфейсу отдельный signaling websocket не нужен. Он работает поверх обычного `HTTPS API`.

## Типы signaling-сообщений

Минимальный набор:

- `session.offer`
- `session.answer`
- `session.ice_candidate`
- `session.request`
- `session.accepted`
- `session.rejected`
- `session.closed`

Пример `session.request`:

```json
{
  "type": "session.request",
  "sessionId": "ses_001",
  "targetDeviceId": "dev_001",
  "fromUserId": "usr_001"
}
```

Пример `session.offer`:

```json
{
  "type": "session.offer",
  "sessionId": "ses_001",
  "sdp": "v=0..."
}
```

Пример `session.ice_candidate`:

```json
{
  "type": "session.ice_candidate",
  "sessionId": "ses_001",
  "candidate": {
    "candidate": "candidate:...",
    "sdpMid": "0",
    "sdpMlineIndex": 0
  }
}
```

## Коды ошибок

Минимальный набор кодов:

- `AUTH_INVALID_CREDENTIALS`
- `AUTH_TOKEN_EXPIRED`
- `DEVICE_NOT_FOUND`
- `DEVICE_OFFLINE`
- `DEVICE_PERMISSION_DENIED`
- `SESSION_NOT_FOUND`
- `SESSION_EXPIRED`
- `ENROLLMENT_TOKEN_INVALID`
- `ENROLLMENT_TOKEN_EXPIRED`
- `RATE_LIMITED`

Формат ошибки:

```json
{
  "error": {
    "code": "DEVICE_OFFLINE",
    "message": "Device is offline"
  }
}
```
