# Server

Minimal Rust control-plane server for BK-Wiver MVP.

Current scope:

- `GET /ws/v1/signal`
- `POST /api/v1/auth/login`
- `POST /api/v1/admin/auth/login`
- `POST /api/v1/devices/register`
- `POST /api/v1/devices/heartbeat`
- `GET /api/v1/devices`
- `POST /api/v1/sessions`
- `POST /api/v1/enrollment-tokens`
- `GET /api/v1/audit`
- `GET /api/v1/admin/devices`
- `GET /api/v1/admin/users`
- `PATCH /api/v1/admin/users/{userId}`
- `PATCH /api/v1/admin/devices/{deviceId}`
- `POST /api/v1/admin/enrollment-tokens`
- `GET /api/v1/admin/audit`

Notes:

- device, token, enrollment token, audit and session state is stored in PostgreSQL;
- desktop and admin authentication use separate generated bearer tokens;
- desktop users and admins are stored in dedicated `users` and `admins` tables;
- enrollment token expiration and single-use semantics are enforced on registration;
- device online status is derived from recent heartbeat activity, not only from a stale boolean flag;
- `/api/v1/admin/*` endpoints require an admin access token;
- admins can change desktop user role and block state via `/api/v1/admin/users/{userId}`;
- websocket signaling accepts either `Authorization: Bearer <token>` or `?token=...`;
- signaling messages are queued in PostgreSQL for offline recipients and flushed on next websocket connect;
- signaling and admin web UI are not implemented yet.

Run locally:

```powershell
$env:BK_WIVER_DATABASE_URL="postgres://postgres:postgres@127.0.0.1:5432/bkwiver"
cargo run -p bk-wiver-server
```

Run with Docker:

```powershell
docker compose up --build
```

Cross-platform shortcuts via `just`:

```powershell
just up
just logs
just check
```

Docker Compose reads optional overrides from project `.env`.
Use [.env.example](C:/BK-Wiver/.env.example) as the starting point if you need custom ports, credentials or the public server URL for LAN access.
