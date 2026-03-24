use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, patch, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool, Row, postgres::PgPoolOptions};
use tokio::sync::{RwLock, mpsc};
use tower_http::trace::TraceLayer;
use uuid::Uuid;

const DEVICE_HEARTBEAT_INTERVAL_SEC: u32 = 15;
const DEVICE_OFFLINE_AFTER_MS: i64 = (DEVICE_HEARTBEAT_INTERVAL_SEC as i64) * 3_000;
const SESSION_TTL_MS: u64 = 5 * 60 * 1000;

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/healthz", get(healthcheck))
        .route("/admin", get(admin_web_app))
        .route("/ws/v1/signal", get(signal_websocket))
        .route("/ws/v1/media", get(media_websocket))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/admin/auth/login", post(admin_login))
        .route("/api/v1/devices/register", post(register_device))
        .route("/api/v1/devices/heartbeat", post(device_heartbeat))
        .route("/api/v1/devices", get(list_devices))
        .route("/api/v1/sessions", post(create_session))
        .route("/api/v1/enrollment-tokens", post(create_enrollment_token))
        .route("/api/v1/audit", get(list_audit_events))
        .route("/api/v1/admin/devices", get(list_admin_devices))
        .route("/api/v1/admin/devices/export.csv", get(export_admin_devices_csv))
        .route("/api/v1/admin/users", get(list_users))
        .route("/api/v1/admin/devices/{device_id}", patch(update_device))
        .route("/api/v1/admin/users/{user_id}", patch(update_user))
        .route(
            "/api/v1/admin/enrollment-tokens",
            get(list_admin_enrollment_tokens).post(create_admin_enrollment_token),
        )
        .route("/api/v1/admin/devices/{device_id}/inventory", get(get_device_inventory_history))
        .route("/api/v1/admin/inventory/history", get(list_inventory_history))
        .route("/api/v1/admin/audit", get(list_audit_events))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

pub struct AppState {
    db: PgPool,
    server_url: String,
    signal_connections: RwLock<HashMap<String, SignalConnection>>,
    media_connections: RwLock<HashMap<String, HashMap<String, MediaConnection>>>,
}

impl AppState {
    pub async fn new(
        server_url: impl Into<String>,
        database_url: impl AsRef<str>,
    ) -> anyhow::Result<Self> {
        let db = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url.as_ref())
            .await?;

        run_migrations(&db).await?;

        Ok(Self {
            db,
            server_url: server_url.into(),
            signal_connections: RwLock::new(HashMap::new()),
            media_connections: RwLock::new(HashMap::new()),
        })
    }
}

#[derive(Clone)]
struct SignalConnection {
    connection_id: String,
    tx: mpsc::UnboundedSender<Message>,
}

#[derive(Clone)]
struct MediaConnection {
    connection_id: String,
    tx: mpsc::UnboundedSender<Message>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MediaWebSocketQuery {
    token: Option<String>,
    session_id: String,
}

#[derive(Serialize)]
struct HealthResponse {
    ok: bool,
    now_ms: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesktopLoginRequest {
    login: String,
    password: String,
    desktop_version: DesktopVersion,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopLoginResponse {
    user_id: String,
    access_token: String,
    refresh_token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdminLoginRequest {
    login: String,
    password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminLoginResponse {
    admin_id: String,
    login: String,
    role: String,
    access_token: String,
    refresh_token: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DesktopVersion {
    version: String,
    commit: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct HostInfo {
    hostname: String,
    os: String,
    os_version: String,
    arch: String,
    username: String,
    #[serde(default)]
    motherboard: String,
    #[serde(default)]
    cpu: String,
    #[serde(default)]
    ram_total_mb: u64,
    #[serde(default)]
    ip_addresses: Vec<String>,
    #[serde(default)]
    mac_addresses: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct PermissionStatus {
    screen_capture: bool,
    input_control: bool,
    accessibility: bool,
    file_transfer: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceRegistrationRequest {
    enrollment_token: String,
    desktop_version: DesktopVersion,
    host_info: HostInfo,
    permissions: PermissionStatus,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceRegistrationResponse {
    device_id: String,
    device_name: String,
    connect_code: String,
    connect_code_expires_at_ms: u64,
    server_url: String,
    device_token: String,
    heartbeat_interval_sec: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceHeartbeatRequest {
    device_id: String,
    permissions: PermissionStatus,
    unix_time_ms: u64,
}

#[derive(Serialize)]
struct AckResponse {
    ok: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DeviceSummary {
    device_id: String,
    device_name: String,
    connect_code: String,
    connect_code_expires_at_ms: u64,
    host_info: HostInfo,
    group_name: Option<String>,
    department: Option<String>,
    location: Option<String>,
    online: bool,
    last_seen_ms: u64,
    permissions: PermissionStatus,
}

#[derive(Serialize)]
struct ListDevicesResponse {
    devices: Vec<DeviceSummary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UserSummary {
    user_id: String,
    login: String,
    role: String,
    blocked: bool,
    last_login_at_ms: u64,
    desktop_version: DesktopVersion,
}

#[derive(Serialize)]
struct ListUsersResponse {
    users: Vec<UserSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateSessionRequest {
    device_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateSessionResponse {
    session_id: String,
    session_token: String,
    expires_at_ms: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateEnrollmentTokenRequest {
    comment: String,
    expires_at_ms: u64,
    single_use: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnrollmentTokenSummary {
    token_id: String,
    token: String,
    comment: String,
    expires_at_ms: u64,
    single_use: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnrollmentTokenDetailsSummary {
    token_id: String,
    token: String,
    comment: String,
    expires_at_ms: u64,
    single_use: bool,
    created_by_user_id: String,
    created_at_ms: u64,
    used_at_ms: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateEnrollmentTokenResponse {
    enrollment_token: EnrollmentTokenSummary,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListEnrollmentTokensResponse {
    enrollment_tokens: Vec<EnrollmentTokenDetailsSummary>,
}

#[derive(Deserialize, Default)]
struct AuditQuery {
    limit: Option<u32>,
}

#[derive(Deserialize, Default)]
struct SignalConnectQuery {
    token: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditEventSummary {
    event_id: String,
    actor_type: String,
    actor_id: String,
    action: String,
    target_type: String,
    target_id: String,
    created_at_ms: u64,
}

#[derive(Serialize)]
struct ListAuditEventsResponse {
    events: Vec<AuditEventSummary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InventoryHistoryRecord {
    record_id: String,
    device_id: String,
    motherboard: Option<String>,
    cpu: Option<String>,
    ram_total_mb: Option<i64>,
    ip_addresses: Option<String>,
    mac_addresses: Option<String>,
    os: String,
    os_version: String,
    arch: String,
    username: String,
    hostname: String,
    recorded_at_ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListInventoryHistoryResponse {
    records: Vec<InventoryHistoryRecord>,
}

#[derive(Deserialize, Default)]
struct InventoryHistoryQuery {
    device_id: Option<String>,
    limit: Option<u32>,
}

#[derive(FromRow)]
struct InventoryRecord {
    record_id: String,
    device_id: String,
    motherboard: Option<String>,
    cpu: Option<String>,
    ram_total_mb: Option<i64>,
    ip_addresses: Option<String>,
    mac_addresses: Option<String>,
    os: String,
    os_version: String,
    arch: String,
    username: String,
    hostname: String,
    recorded_at_ms: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateDeviceRequest {
    blocked: Option<bool>,
    device_name: Option<String>,
    group_name: Option<String>,
    department: Option<String>,
    location: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateUserRequest {
    role: Option<String>,
    blocked: Option<bool>,
}

#[derive(FromRow)]
struct DeviceRecord {
    device_id: String,
    device_name: String,
    #[allow(dead_code)]
    owner_user_id: Option<String>,
    hostname: String,
    os: String,
    os_version: String,
    arch: String,
    username: String,
    motherboard: Option<String>,
    cpu: Option<String>,
    ram_total_mb: Option<i64>,
    ip_addresses: Option<String>,
    mac_addresses: Option<String>,
    group_name: Option<String>,
    department: Option<String>,
    location: Option<String>,
    online: bool,
    last_seen_ms: i64,
    screen_capture: bool,
    input_control: bool,
    accessibility: bool,
    file_transfer: bool,
    blocked: bool,
}

#[derive(FromRow)]
struct AuditEventRecord {
    event_id: String,
    actor_type: String,
    actor_id: String,
    action: String,
    target_type: String,
    target_id: String,
    created_at_ms: i64,
}

#[derive(FromRow)]
struct UserRecord {
    user_id: String,
    login: String,
    role: String,
    blocked: bool,
    last_login_at_ms: i64,
    desktop_version: String,
    desktop_commit: String,
}

#[derive(FromRow)]
struct EnrollmentTokenRecord {
    token_id: String,
    token: String,
    comment: String,
    expires_at_ms: i64,
    single_use: bool,
    created_by_user_id: String,
    created_at_ms: i64,
    used_at_ms: Option<i64>,
}

#[derive(FromRow)]
struct SessionRecord {
    session_id: String,
    target_device_id: String,
    user_id: String,
    expires_at_ms: i64,
}

#[derive(FromRow)]
struct PendingSignalRecord {
    message_id: String,
    payload_json: String,
}

#[derive(Debug)]
struct ParsedSignalMessage {
    message_type: String,
    session_id: String,
    payload: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiErrorResponse {
    error: ApiErrorBody,
}

#[derive(Serialize)]
struct ApiErrorBody {
    code: &'static str,
    message: String,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "AUTH_TOKEN_EXPIRED", message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiErrorResponse {
                error: ApiErrorBody {
                    code: self.code,
                    message: self.message,
                },
            }),
        )
            .into_response()
    }
}

async fn healthcheck(State(state): State<Arc<AppState>>) -> Result<Json<HealthResponse>, ApiError> {
    sqlx::query("SELECT 1")
        .execute(&state.db)
        .await
        .map_err(|error| ApiError::internal(format!("database healthcheck failed: {error}")))?;

    Ok(Json(HealthResponse {
        ok: true,
        now_ms: now_ms(),
    }))
}

async fn admin_web_app() -> Html<&'static str> {
    Html(include_str!("admin-ui/admin.html"))
}

async fn signal_websocket(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SignalConnectQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let token = bearer_token_or_query(&headers, query.token)?;
    let actor = authorize_signal_actor(&state, &token).await?;

    Ok(ws.on_upgrade(move |socket| handle_signal_socket(state, socket, actor)))
}

async fn media_websocket(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<MediaWebSocketQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let token = bearer_token_or_query(&headers, query.token)?;
    let actor = authorize_signal_actor(&state, &token).await?;
    authorize_media_session_actor(&state, &actor, &query.session_id).await?;
    let session_id = query.session_id.clone();

    Ok(ws.on_upgrade(move |socket| handle_media_socket(state, socket, actor, session_id)))
}

async fn login(
    State(state): State<Arc<AppState>>,
    Json(request): Json<DesktopLoginRequest>,
) -> Result<Json<DesktopLoginResponse>, ApiError> {
    let login = request.login.trim();
    if login.is_empty() || request.password.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "AUTH_INVALID_CREDENTIALS",
            "login and password must be provided",
        ));
    }

    let user = sqlx::query(
        r#"
        SELECT user_id, blocked
        FROM users
        WHERE login = $1
        "#,
    )
    .bind(login)
    .fetch_optional(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to read user: {error}")))?;

    let user_id = if let Some(row) = user {
        if row.get::<bool, _>("blocked") {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "DEVICE_PERMISSION_DENIED",
                "user is blocked",
            ));
        }
        let user_id = row.get::<String, _>("user_id");
        sqlx::query(
            r#"
            UPDATE users
            SET
                last_login_at_ms = $2,
                desktop_version = $3,
                desktop_commit = $4
            WHERE user_id = $1
            "#,
        )
        .bind(&user_id)
        .bind(now_ms_i64())
        .bind(&request.desktop_version.version)
        .bind(&request.desktop_version.commit)
        .execute(&state.db)
        .await
        .map_err(|error| {
            ApiError::internal(format!("failed to update user login state: {error}"))
        })?;
        user_id
    } else {
        let user_id = format!("usr_{}", short_id());
        sqlx::query(
            r#"
            INSERT INTO users (
                user_id,
                login,
                role,
                blocked,
                last_login_at_ms,
                desktop_version,
                desktop_commit
            )
            VALUES ($1, $2, 'operator', FALSE, $3, $4, $5)
            "#,
        )
        .bind(&user_id)
        .bind(login)
        .bind(now_ms_i64())
        .bind(&request.desktop_version.version)
        .bind(&request.desktop_version.commit)
        .execute(&state.db)
        .await
        .map_err(|error| ApiError::internal(format!("failed to create user: {error}")))?;
        user_id
    };

    let access_token = format!("access_{}", Uuid::new_v4().simple());
    let refresh_token = format!("refresh_{}", Uuid::new_v4().simple());

    sqlx::query(
        r#"
        INSERT INTO access_tokens (
            token,
            user_id,
            refresh_token,
            desktop_version,
            desktop_commit,
            created_at_ms
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(&access_token)
    .bind(&user_id)
    .bind(&refresh_token)
    .bind(&request.desktop_version.version)
    .bind(&request.desktop_version.commit)
    .bind(now_ms_i64())
    .execute(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to persist access token: {error}")))?;

    record_audit_event(&state.db, "user", &user_id, "auth.login", "user", &user_id).await?;

    Ok(Json(DesktopLoginResponse {
        user_id,
        access_token,
        refresh_token,
    }))
}

async fn admin_login(
    State(state): State<Arc<AppState>>,
    Json(request): Json<AdminLoginRequest>,
) -> Result<Json<AdminLoginResponse>, ApiError> {
    let login = request.login.trim();
    if login.is_empty() || request.password.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "AUTH_INVALID_CREDENTIALS",
            "login and password must be provided",
        ));
    }

    let admin = sqlx::query(
        r#"
        SELECT admin_id, login, role, blocked
        FROM admins
        WHERE login = $1
        "#,
    )
    .bind(login)
    .fetch_optional(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to read admin: {error}")))?;

    let admin_id = if let Some(row) = admin {
        if row.get::<bool, _>("blocked") {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "DEVICE_PERMISSION_DENIED",
                "admin is blocked",
            ));
        }
        let admin_id = row.get::<String, _>("admin_id");
        let admin_login = row.get::<String, _>("login");
        let admin_role = row.get::<String, _>("role");
        sqlx::query(
            r#"
            UPDATE admins
            SET last_login_at_ms = $2
            WHERE admin_id = $1
            "#,
        )
        .bind(&admin_id)
        .bind(now_ms_i64())
        .execute(&state.db)
        .await
        .map_err(|error| {
            ApiError::internal(format!("failed to update admin login state: {error}"))
        })?;
        sqlx::query(
            r#"
            INSERT INTO admins (admin_id, login, role, blocked, last_login_at_ms)
            VALUES ($1, $2, $3, FALSE, $4)
            ON CONFLICT (admin_id) DO NOTHING
            "#,
        )
        .bind(&admin_id)
        .bind(&admin_login)
        .bind(&admin_role)
        .bind(now_ms_i64())
        .execute(&state.db)
        .await
        .ok();
        admin_id
    } else {
        let admin_id = format!("adm_{}", short_id());
        sqlx::query(
            r#"
            INSERT INTO admins (
                admin_id,
                login,
                role,
                blocked,
                last_login_at_ms
            )
            VALUES ($1, $2, 'admin', FALSE, $3)
            "#,
        )
        .bind(&admin_id)
        .bind(login)
        .bind(now_ms_i64())
        .execute(&state.db)
        .await
        .map_err(|error| ApiError::internal(format!("failed to create admin: {error}")))?;
        admin_id
    };

    let access_token = format!("admin_access_{}", Uuid::new_v4().simple());
    let refresh_token = format!("admin_refresh_{}", Uuid::new_v4().simple());

    sqlx::query(
        r#"
        INSERT INTO admin_access_tokens (
            token,
            admin_id,
            refresh_token,
            created_at_ms
        )
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(&access_token)
    .bind(&admin_id)
    .bind(&refresh_token)
    .bind(now_ms_i64())
    .execute(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to persist admin token: {error}")))?;

    record_audit_event(
        &state.db,
        "admin",
        &admin_id,
        "admin.auth.login",
        "admin",
        &admin_id,
    )
    .await?;

    let admin = sqlx::query(
        r#"
        SELECT login, role FROM admins WHERE admin_id = $1
        "#,
    )
    .bind(&admin_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let (admin_login, admin_role) = if let Some(row) = admin {
        (row.get::<String, _>("login"), row.get::<String, _>("role"))
    } else {
        (login.to_string(), "admin".to_string())
    };

    Ok(Json(AdminLoginResponse {
        admin_id,
        login: admin_login,
        role: admin_role,
        access_token,
        refresh_token,
    }))
}

async fn register_device(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<DeviceRegistrationRequest>,
) -> Result<Json<DeviceRegistrationResponse>, ApiError> {
    let user_id = authorize_access_token(&state, &headers).await?;
    let enrollment_token = request.enrollment_token.trim();
    let mut enrollment_token_id: Option<String> = None;
    let mut single_use = false;

    if !enrollment_token.is_empty() {
        let enrollment = sqlx::query(
            r#"
            SELECT token_id, expires_at_ms, single_use, used_at_ms
            FROM enrollment_tokens
            WHERE token = $1
            "#,
        )
        .bind(enrollment_token)
        .fetch_optional(&state.db)
        .await
        .map_err(|error| ApiError::internal(format!("failed to read enrollment token: {error}")))?;

        let enrollment = enrollment.ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "ENROLLMENT_TOKEN_INVALID",
                "enrollment token is invalid",
            )
        })?;

        enrollment_token_id = Some(enrollment.get::<String, _>("token_id"));
        let expires_at_ms = enrollment.get::<i64, _>("expires_at_ms") as u64;
        single_use = enrollment.get::<bool, _>("single_use");
        let used_at_ms = enrollment.get::<Option<i64>, _>("used_at_ms");

        if expires_at_ms < now_ms() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "ENROLLMENT_TOKEN_EXPIRED",
                "enrollment token is expired",
            ));
        }

        if single_use && used_at_ms.is_some() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "ENROLLMENT_TOKEN_EXPIRED",
                "enrollment token was already used",
            ));
        }
    }

    let device_id = next_device_id(&state.db).await?;
    let device_name = request.host_info.hostname.clone();
    let device_token = format!("device_{}", Uuid::new_v4().simple());
    let last_seen_ms = now_ms_i64();

    sqlx::query(
        r#"
        INSERT INTO devices (
            device_id,
            device_name,
            owner_user_id,
            hostname,
            os,
            os_version,
            arch,
            username,
            motherboard,
            cpu,
            ram_total_mb,
            ip_addresses,
            mac_addresses,
            group_name,
            department,
            location,
            online,
            last_seen_ms,
            screen_capture,
            input_control,
            accessibility,
            file_transfer,
            blocked,
            device_token,
            desktop_version,
            desktop_commit
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, NULL, NULL, NULL, TRUE, $14, $15, $16, $17, $18, FALSE, $19, $20, $21
        )
        "#,
    )
    .bind(&device_id)
    .bind(&device_name)
    .bind(&user_id)
    .bind(&request.host_info.hostname)
    .bind(&request.host_info.os)
    .bind(&request.host_info.os_version)
    .bind(&request.host_info.arch)
    .bind(&request.host_info.username)
    .bind(null_if_empty(&request.host_info.motherboard))
    .bind(null_if_empty(&request.host_info.cpu))
    .bind(if request.host_info.ram_total_mb == 0 {
        None
    } else {
        Some(request.host_info.ram_total_mb as i64)
    })
    .bind(serialize_string_list(&request.host_info.ip_addresses))
    .bind(serialize_string_list(&request.host_info.mac_addresses))
    .bind(last_seen_ms)
    .bind(request.permissions.screen_capture)
    .bind(request.permissions.input_control)
    .bind(request.permissions.accessibility)
    .bind(request.permissions.file_transfer)
    .bind(&device_token)
    .bind(&request.desktop_version.version)
    .bind(&request.desktop_version.commit)
    .execute(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to register device: {error}")))?;

    if single_use {
        sqlx::query(
            r#"
            UPDATE enrollment_tokens
            SET used_at_ms = $2
            WHERE token_id = $1
            "#,
        )
        .bind(enrollment_token_id.as_deref().unwrap_or_default())
        .bind(last_seen_ms)
        .execute(&state.db)
        .await
        .map_err(|error| {
            ApiError::internal(format!("failed to mark enrollment token as used: {error}"))
        })?;
    }

    record_audit_event(
        &state.db,
        "user",
        &user_id,
        "device.register",
        "device",
        &device_id,
    )
    .await?;

    // Запись начальных данных инвентаризации в историю
    record_inventory_change(
        &state.db,
        &device_id,
        null_if_empty(&request.host_info.motherboard).as_deref(),
        null_if_empty(&request.host_info.cpu).as_deref(),
        if request.host_info.ram_total_mb == 0 {
            None
        } else {
            Some(request.host_info.ram_total_mb as i64)
        },
        serialize_string_list(&request.host_info.ip_addresses).as_deref(),
        serialize_string_list(&request.host_info.mac_addresses).as_deref(),
        &request.host_info.os,
        &request.host_info.os_version,
        &request.host_info.arch,
        &request.host_info.username,
        &request.host_info.hostname,
    )
    .await?;

    let (connect_code, connect_code_expires_at_ms) = current_connect_code(&device_id, now_ms());

    Ok(Json(DeviceRegistrationResponse {
        device_id,
        device_name,
        connect_code,
        connect_code_expires_at_ms,
        server_url: state.server_url.clone(),
        device_token,
        heartbeat_interval_sec: DEVICE_HEARTBEAT_INTERVAL_SEC,
    }))
}

async fn device_heartbeat(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<DeviceHeartbeatRequest>,
) -> Result<Json<AckResponse>, ApiError> {
    let device_id = authorize_device_token(&state, &headers).await?;
    if device_id != request.device_id {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "DEVICE_PERMISSION_DENIED",
            "device token does not match deviceId",
        ));
    }

    let result = sqlx::query(
        r#"
        UPDATE devices
        SET
            online = TRUE,
            last_seen_ms = $2,
            screen_capture = $3,
            input_control = $4,
            accessibility = $5,
            file_transfer = $6
        WHERE device_id = $1
        "#,
    )
    .bind(&device_id)
    .bind((request.unix_time_ms.max(now_ms())) as i64)
    .bind(request.permissions.screen_capture)
    .bind(request.permissions.input_control)
    .bind(request.permissions.accessibility)
    .bind(request.permissions.file_transfer)
    .execute(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to update heartbeat: {error}")))?;

    if result.rows_affected() == 0 {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "DEVICE_NOT_FOUND",
            "device is not registered",
        ));
    }

    Ok(Json(AckResponse { ok: true }))
}

async fn list_devices(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<ListDevicesResponse>, ApiError> {
    let _user_id = authorize_access_token(&state, &headers).await?;
    let records = fetch_visible_devices(&state.db).await?;

    let devices = records.into_iter().map(DeviceSummary::from).collect();

    Ok(Json(ListDevicesResponse { devices }))
}

async fn list_admin_devices(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<ListDevicesResponse>, ApiError> {
    let _admin_id = authorize_admin_access_token(&state, &headers).await?;
    let records = fetch_visible_devices(&state.db).await?;
    let devices = records.into_iter().map(DeviceSummary::from).collect();
    Ok(Json(ListDevicesResponse { devices }))
}

async fn export_admin_devices_csv(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _admin_id = authorize_admin_access_token(&state, &headers).await?;
    let records = fetch_visible_devices(&state.db).await?;

    let mut csv = String::from(
        "Device ID,Device Name,Group,Department,Location,Hostname,User,OS,OS Version,Arch,CPU,Motherboard,RAM MB,IP Addresses,MAC Addresses,Online,Last Seen\n",
    );

    for device in records.into_iter().map(DeviceSummary::from) {
        let row = [
            device.device_id,
            device.device_name,
            device.group_name.unwrap_or_default(),
            device.department.unwrap_or_default(),
            device.location.unwrap_or_default(),
            device.host_info.hostname,
            device.host_info.username,
            device.host_info.os,
            device.host_info.os_version,
            device.host_info.arch,
            device.host_info.cpu,
            device.host_info.motherboard,
            device.host_info.ram_total_mb.to_string(),
            device.host_info.ip_addresses.join(" | "),
            device.host_info.mac_addresses.join(" | "),
            if device.online {
                "yes".to_owned()
            } else {
                "no".to_owned()
            },
            device.last_seen_ms.to_string(),
        ]
        .into_iter()
        .map(|value| csv_escape(&value))
        .collect::<Vec<_>>()
        .join(",");
        csv.push_str(&row);
        csv.push('\n');
    }

    Ok((
        [
            ("content-type", "text/csv; charset=utf-8"),
            (
                "content-disposition",
                "attachment; filename=\"bk-wiver-devices.csv\"",
            ),
        ],
        csv,
    )
        .into_response())
}

async fn list_users(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<ListUsersResponse>, ApiError> {
    let _admin_id = authorize_admin_access_token(&state, &headers).await?;
    let records = sqlx::query_as::<_, UserRecord>(
        r#"
        SELECT
            user_id,
            login,
            role,
            blocked,
            last_login_at_ms,
            desktop_version,
            desktop_commit
        FROM users
        ORDER BY last_login_at_ms DESC, login ASC
        "#,
    )
    .fetch_all(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to list users: {error}")))?;

    let users = records.into_iter().map(UserSummary::from).collect();
    Ok(Json(ListUsersResponse { users }))
}

async fn update_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    Json(request): Json<UpdateUserRequest>,
) -> Result<Json<UserSummary>, ApiError> {
    let admin_id = authorize_admin_access_token(&state, &headers).await?;
    let requested_role = normalize_user_role(request.role.as_deref())?;

    let user = sqlx::query_as::<_, UserRecord>(
        r#"
        UPDATE users
        SET
            role = COALESCE($2, role),
            blocked = COALESCE($3, blocked)
        WHERE user_id = $1
        RETURNING
            user_id,
            login,
            role,
            blocked,
            last_login_at_ms,
            desktop_version,
            desktop_commit
        "#,
    )
    .bind(&user_id)
    .bind(requested_role.as_deref())
    .bind(request.blocked)
    .fetch_optional(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to update user: {error}")))?
    .ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "DEVICE_NOT_FOUND",
            "user is not registered",
        )
    })?;

    let action = if request.blocked == Some(true) {
        "user.block"
    } else if request.blocked == Some(false) {
        "user.unblock"
    } else if requested_role.is_some() {
        "user.role.update"
    } else {
        "user.update"
    };

    record_audit_event(&state.db, "admin", &admin_id, action, "user", &user_id).await?;
    Ok(Json(user.into()))
}

async fn create_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CreateSessionRequest>,
) -> Result<Json<CreateSessionResponse>, ApiError> {
    let user_id = authorize_access_token(&state, &headers).await?;
    let target = find_target_device(&state.db, &request.device_id).await?;

    if target.blocked {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "DEVICE_PERMISSION_DENIED",
            "device is blocked",
        ));
    }

    if !is_device_online(target.last_seen_ms, target.online) {
        sqlx::query("UPDATE devices SET online = FALSE WHERE device_id = $1")
            .bind(&request.device_id)
            .execute(&state.db)
            .await
            .map_err(|error| {
                ApiError::internal(format!("failed to mark device offline: {error}"))
            })?;

        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "DEVICE_OFFLINE",
            "device is offline",
        ));
    }

    let session_id = format!("ses_{}", short_id());
    let session_token = format!("session_{}", Uuid::new_v4().simple());
    let expires_at_ms = now_ms() + SESSION_TTL_MS;

    sqlx::query(
        r#"
        INSERT INTO sessions (
            session_id,
            target_device_id,
            user_id,
            session_token,
            expires_at_ms,
            created_at_ms
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(&session_id)
    .bind(&request.device_id)
    .bind(&user_id)
    .bind(&session_token)
    .bind(expires_at_ms as i64)
    .bind(now_ms_i64())
    .execute(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to create session: {error}")))?;

    let request_payload = json!({
        "type": "session.request",
        "sessionId": session_id,
        "targetDeviceId": request.device_id,
        "fromUserId": user_id,
    });
    send_or_queue_signal(
        &state,
        &SignalActor::Device {
            device_id: request.device_id.clone(),
        }
        .connection_key(),
        request_payload,
    )
    .await?;

    record_audit_event(
        &state.db,
        "user",
        &user_id,
        "session.create",
        "session",
        &session_id,
    )
    .await?;

    Ok(Json(CreateSessionResponse {
        session_id,
        session_token,
        expires_at_ms,
    }))
}

async fn update_device(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Json(request): Json<UpdateDeviceRequest>,
) -> Result<Json<DeviceSummary>, ApiError> {
    let admin_id = authorize_admin_access_token(&state, &headers).await?;
    let requested_blocked = request.blocked;
    let requested_device_name = request.device_name.clone();
    let requested_group_name = request.group_name.clone();
    let requested_department = request.department.clone();
    let requested_location = request.location.clone();

    let device = sqlx::query_as::<_, DeviceRecord>(
        r#"
        UPDATE devices
        SET
            blocked = COALESCE($2, blocked),
            online = CASE WHEN COALESCE($2, FALSE) THEN FALSE ELSE online END,
            device_name = COALESCE(NULLIF($3, ''), device_name),
            group_name = COALESCE($4, group_name),
            department = COALESCE($5, department),
            location = COALESCE($6, location)
        WHERE device_id = $1
        RETURNING
            device_id,
            device_name,
            owner_user_id,
            hostname,
            os,
            os_version,
            arch,
            username,
            motherboard,
            cpu,
            ram_total_mb,
            ip_addresses,
            mac_addresses,
            group_name,
            department,
            location,
            online,
            last_seen_ms,
            screen_capture,
            input_control,
            accessibility,
            file_transfer,
            blocked
        "#,
    )
    .bind(&device_id)
    .bind(requested_blocked)
    .bind(requested_device_name.clone().unwrap_or_default())
    .bind(requested_group_name)
    .bind(requested_department)
    .bind(requested_location)
    .fetch_optional(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to update device: {error}")))?
    .ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "DEVICE_NOT_FOUND",
            "device is not registered",
        )
    })?;

    let action = if requested_blocked == Some(true) {
        "device.block"
    } else if requested_blocked == Some(false) {
        "device.unblock"
    } else if requested_device_name.is_some() {
        "device.rename"
    } else if request.group_name.is_some() || request.department.is_some() || request.location.is_some()
    {
        "device.classify"
    } else {
        "device.update"
    };

    record_audit_event(&state.db, "admin", &admin_id, action, "device", &device_id).await?;

    Ok(Json(device.into()))
}

async fn create_enrollment_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CreateEnrollmentTokenRequest>,
) -> Result<Json<CreateEnrollmentTokenResponse>, ApiError> {
    let actor = authorize_api_actor(&state, &headers).await?;
    if request.comment.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "ENROLLMENT_TOKEN_INVALID",
            "comment must be provided",
        ));
    }

    if request.expires_at_ms <= now_ms() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "ENROLLMENT_TOKEN_EXPIRED",
            "expiresAtMs must point to the future",
        ));
    }

    let token_id = format!("enr_{}", short_id());
    let token = format!("enroll_{}", Uuid::new_v4().simple());

    sqlx::query(
        r#"
        INSERT INTO enrollment_tokens (
            token_id,
            token,
            comment,
            expires_at_ms,
            single_use,
            created_by_user_id,
            created_at_ms
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(&token_id)
    .bind(&token)
    .bind(request.comment.trim())
    .bind(request.expires_at_ms as i64)
    .bind(request.single_use)
    .bind(&actor.actor_id)
    .bind(now_ms_i64())
    .execute(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to create enrollment token: {error}")))?;

    record_audit_event(
        &state.db,
        actor.actor_type,
        &actor.actor_id,
        "enrollment_token.create",
        "enrollment_token",
        &token_id,
    )
    .await?;

    Ok(Json(CreateEnrollmentTokenResponse {
        enrollment_token: EnrollmentTokenSummary {
            token_id,
            token,
            comment: request.comment,
            expires_at_ms: request.expires_at_ms,
            single_use: request.single_use,
        },
    }))
}

async fn create_admin_enrollment_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CreateEnrollmentTokenRequest>,
) -> Result<Json<CreateEnrollmentTokenResponse>, ApiError> {
    let _admin_id = authorize_admin_access_token(&state, &headers).await?;
    create_enrollment_token(State(state), headers, Json(request)).await
}

async fn list_admin_enrollment_tokens(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<ListEnrollmentTokensResponse>, ApiError> {
    let _admin_id = authorize_admin_access_token(&state, &headers).await?;

    let records = sqlx::query_as::<_, EnrollmentTokenRecord>(
        r#"
        SELECT
            token_id,
            token,
            comment,
            expires_at_ms,
            single_use,
            created_by_user_id,
            created_at_ms,
            used_at_ms
        FROM enrollment_tokens
        ORDER BY created_at_ms DESC, token_id DESC
        "#
    )
    .fetch_all(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to list enrollment tokens: {error}")))?;

    let enrollment_tokens = records
        .into_iter()
        .map(EnrollmentTokenDetailsSummary::from)
        .collect();

    Ok(Json(ListEnrollmentTokensResponse { enrollment_tokens }))
}

async fn list_audit_events(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<AuditQuery>,
) -> Result<Json<ListAuditEventsResponse>, ApiError> {
    let _admin_id = authorize_admin_access_token(&state, &headers).await?;
    let limit = query.limit.unwrap_or(100).clamp(1, 500) as i64;

    let records = sqlx::query_as::<_, AuditEventRecord>(
        r#"
        SELECT
            event_id,
            actor_type,
            actor_id,
            action,
            target_type,
            target_id,
            created_at_ms
        FROM audit_events
        ORDER BY created_at_ms DESC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to list audit events: {error}")))?;

    let events = records.into_iter().map(AuditEventSummary::from).collect();
    Ok(Json(ListAuditEventsResponse { events }))
}

async fn list_inventory_history(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<InventoryHistoryQuery>,
) -> Result<Json<ListInventoryHistoryResponse>, ApiError> {
    let _admin_id = authorize_admin_access_token(&state, &headers).await?;
    let device_id = query.device_id.as_deref();
    let limit = query.limit.unwrap_or(50).clamp(1, 200) as i64;

    let records = if let Some(device_id) = device_id {
        sqlx::query_as::<_, InventoryRecord>(
            r#"
            SELECT
                record_id,
                device_id,
                motherboard,
                cpu,
                ram_total_mb,
                ip_addresses,
                mac_addresses,
                os,
                os_version,
                arch,
                username,
                hostname,
                recorded_at_ms
            FROM inventory_history
            WHERE device_id = $1
            ORDER BY recorded_at_ms DESC
            LIMIT $2
            "#,
        )
        .bind(device_id)
        .bind(limit)
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as::<_, InventoryRecord>(
            r#"
            SELECT
                record_id,
                device_id,
                motherboard,
                cpu,
                ram_total_mb,
                ip_addresses,
                mac_addresses,
                os,
                os_version,
                arch,
                username,
                hostname,
                recorded_at_ms
            FROM inventory_history
            ORDER BY recorded_at_ms DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&state.db)
        .await
    }
    .map_err(|error| ApiError::internal(format!("failed to list inventory history: {error}")))?;

    let history = records
        .into_iter()
        .map(|r| InventoryHistoryRecord {
            record_id: r.record_id,
            device_id: r.device_id,
            motherboard: r.motherboard,
            cpu: r.cpu,
            ram_total_mb: r.ram_total_mb,
            ip_addresses: r.ip_addresses,
            mac_addresses: r.mac_addresses,
            os: r.os,
            os_version: r.os_version,
            arch: r.arch,
            username: r.username,
            hostname: r.hostname,
            recorded_at_ms: r.recorded_at_ms as u64,
        })
        .collect();

    Ok(Json(ListInventoryHistoryResponse { records: history }))
}

async fn get_device_inventory_history(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
) -> Result<Json<ListInventoryHistoryResponse>, ApiError> {
    let _admin_id = authorize_admin_access_token(&state, &headers).await?;
    let limit = 50i64;

    let records = sqlx::query_as::<_, InventoryRecord>(
        r#"
        SELECT
            record_id,
            device_id,
            motherboard,
            cpu,
            ram_total_mb,
            ip_addresses,
            mac_addresses,
            os,
            os_version,
            arch,
            username,
            hostname,
            recorded_at_ms
        FROM inventory_history
        WHERE device_id = $1
        ORDER BY recorded_at_ms DESC
        LIMIT $2
        "#,
    )
    .bind(&device_id)
    .bind(limit)
    .fetch_all(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to get device inventory history: {error}")))?;

    let history = records
        .into_iter()
        .map(|r| InventoryHistoryRecord {
            record_id: r.record_id,
            device_id: r.device_id,
            motherboard: r.motherboard,
            cpu: r.cpu,
            ram_total_mb: r.ram_total_mb,
            ip_addresses: r.ip_addresses,
            mac_addresses: r.mac_addresses,
            os: r.os,
            os_version: r.os_version,
            arch: r.arch,
            username: r.username,
            hostname: r.hostname,
            recorded_at_ms: r.recorded_at_ms as u64,
        })
        .collect();

    Ok(Json(ListInventoryHistoryResponse { records: history }))
}

async fn record_inventory_change(
    db: &PgPool,
    device_id: &str,
    motherboard: Option<&str>,
    cpu: Option<&str>,
    ram_total_mb: Option<i64>,
    ip_addresses: Option<&str>,
    mac_addresses: Option<&str>,
    os: &str,
    os_version: &str,
    arch: &str,
    username: &str,
    hostname: &str,
) -> Result<(), ApiError> {
    let record_id = format!("inv_{}", short_id());
    let recorded_at_ms = now_ms_i64();

    sqlx::query(
        r#"
        INSERT INTO inventory_history (
            record_id,
            device_id,
            motherboard,
            cpu,
            ram_total_mb,
            ip_addresses,
            mac_addresses,
            os,
            os_version,
            arch,
            username,
            hostname,
            recorded_at_ms
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
        "#,
    )
    .bind(&record_id)
    .bind(device_id)
    .bind(motherboard)
    .bind(cpu)
    .bind(ram_total_mb)
    .bind(ip_addresses)
    .bind(mac_addresses)
    .bind(os)
    .bind(os_version)
    .bind(arch)
    .bind(username)
    .bind(hostname)
    .bind(recorded_at_ms)
    .execute(db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to record inventory change: {error}")))?;

    Ok(())
}

async fn handle_signal_socket(state: Arc<AppState>, mut socket: WebSocket, actor: SignalActor) {
    let connection_id = Uuid::new_v4().to_string();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let actor_key = actor.connection_key();
    register_signal_connection(&state, actor_key.clone(), &connection_id, tx).await;

    if let Err(error) = flush_pending_signals(&state, &actor_key).await {
        let payload = json!({
            "type": "error",
            "code": error.code,
            "message": error.message,
        });
        let _ = socket.send(Message::Text(payload.to_string().into())).await;
    }

    loop {
        tokio::select! {
            outbound = rx.recv() => {
                match outbound {
                    Some(message) => {
                        if socket.send(message).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            inbound = socket.recv() => {
                match inbound {
                    Some(Ok(Message::Text(text))) => {
                        if let Err(error) = process_signal_message(&state, &actor, text.to_string()).await {
                            let payload = json!({
                                "type": "error",
                                "code": error.code,
                                "message": error.message,
                            });
                            if socket.send(Message::Text(payload.to_string().into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {}
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) => break,
                    Some(Err(_)) => break,
                    None => break,
                }
            }
        }
    }

    unregister_signal_connection(&state, &actor_key, &connection_id).await;
}

async fn handle_media_socket(
    state: Arc<AppState>,
    mut socket: WebSocket,
    actor: SignalActor,
    session_id: String,
) {
    let connection_id = Uuid::new_v4().to_string();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let actor_key = actor.connection_key();
    register_media_connection(&state, &session_id, actor_key.clone(), &connection_id, tx).await;

    loop {
        tokio::select! {
            outbound = rx.recv() => {
                match outbound {
                    Some(message) => {
                        if socket.send(message).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            inbound = socket.recv() => {
                match inbound {
                    Some(Ok(Message::Binary(bytes))) => {
                        if relay_media_message(&state, &session_id, &actor, bytes.to_vec()).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) => break,
                    Some(Ok(Message::Text(_))) => {}
                    Some(Err(_)) => break,
                    None => break,
                }
            }
        }
    }

    unregister_media_connection(&state, &session_id, &actor_key, &connection_id).await;
}

async fn process_signal_message(
    state: &Arc<AppState>,
    actor: &SignalActor,
    raw_message: String,
) -> Result<(), ApiError> {
    let parsed = parse_signal_message(&raw_message)?;

    let session = sqlx::query_as::<_, SessionRecord>(
        r#"
        SELECT
            session_id,
            target_device_id,
            user_id,
            expires_at_ms
        FROM sessions
        WHERE session_id = $1
        "#,
    )
    .bind(&parsed.session_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to read session: {error}")))?
    .ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "SESSION_NOT_FOUND",
            "session does not exist",
        )
    })?;

    if session.expires_at_ms < now_ms_i64() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "SESSION_EXPIRED",
            "session is expired",
        ));
    }

    let recipient = match actor {
        SignalActor::User { user_id } if session.user_id == *user_id => SignalActor::Device {
            device_id: session.target_device_id.clone(),
        },
        SignalActor::Device { device_id } if session.target_device_id == *device_id => {
            SignalActor::User {
                user_id: session.user_id.clone(),
            }
        }
        _ => {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "DEVICE_PERMISSION_DENIED",
                "actor is not allowed to use this session",
            ));
        }
    };

    if parsed.message_type == "session.closed" {
        sqlx::query(
            r#"
            UPDATE sessions
            SET expires_at_ms = LEAST(expires_at_ms, $2)
            WHERE session_id = $1
            "#,
        )
        .bind(&session.session_id)
        .bind(now_ms_i64())
        .execute(&state.db)
        .await
        .map_err(|error| ApiError::internal(format!("failed to close session: {error}")))?;
    } else {
        sqlx::query(
            r#"
            UPDATE sessions
            SET expires_at_ms = GREATEST(expires_at_ms, $2)
            WHERE session_id = $1
            "#,
        )
        .bind(&session.session_id)
        .bind(now_ms_i64() + SESSION_TTL_MS as i64)
        .execute(&state.db)
        .await
        .map_err(|error| ApiError::internal(format!("failed to extend session ttl: {error}")))?;
    }

    send_or_queue_signal(state, &recipient.connection_key(), parsed.payload.clone()).await?;
    record_audit_event(
        &state.db,
        actor.actor_type(),
        actor.actor_id(),
        &parsed.message_type,
        "session",
        &session.session_id,
    )
    .await?;

    Ok(())
}

async fn authorize_access_token(state: &AppState, headers: &HeaderMap) -> Result<String, ApiError> {
    let token = bearer_token(headers)?;
    sqlx::query(
        r#"
        SELECT access_tokens.user_id
        FROM access_tokens
        INNER JOIN users ON users.user_id = access_tokens.user_id
        WHERE access_tokens.token = $1 AND users.blocked = FALSE
        "#,
    )
    .bind(token)
    .fetch_optional(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to authorize access token: {error}")))?
    .map(|row| row.get::<String, _>("user_id"))
    .ok_or_else(|| ApiError::unauthorized("access token is invalid or expired"))
}

async fn authorize_admin_access_token(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<String, ApiError> {
    let token = bearer_token(headers)?;
    sqlx::query(
        r#"
        SELECT admin_access_tokens.admin_id
        FROM admin_access_tokens
        INNER JOIN admins ON admins.admin_id = admin_access_tokens.admin_id
        WHERE admin_access_tokens.token = $1 AND admins.blocked = FALSE
        "#,
    )
    .bind(token)
    .fetch_optional(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to authorize admin token: {error}")))?
    .map(|row| row.get::<String, _>("admin_id"))
    .ok_or_else(|| ApiError::unauthorized("admin access token is invalid or expired"))
}

struct ApiActor<'a> {
    actor_type: &'a str,
    actor_id: String,
}

enum SignalActor {
    User { user_id: String },
    Device { device_id: String },
}

impl SignalActor {
    fn actor_type(&self) -> &'static str {
        match self {
            Self::User { .. } => "user",
            Self::Device { .. } => "device",
        }
    }

    fn actor_id(&self) -> &str {
        match self {
            Self::User { user_id } => user_id,
            Self::Device { device_id } => device_id,
        }
    }

    fn connection_key(&self) -> String {
        match self {
            Self::User { user_id } => format!("user:{user_id}"),
            Self::Device { device_id } => format!("device:{device_id}"),
        }
    }
}

async fn authorize_api_actor<'a>(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<ApiActor<'a>, ApiError> {
    let token = bearer_token(headers)?;

    if let Some(row) = sqlx::query(
        r#"
        SELECT admin_access_tokens.admin_id
        FROM admin_access_tokens
        INNER JOIN admins ON admins.admin_id = admin_access_tokens.admin_id
        WHERE admin_access_tokens.token = $1 AND admins.blocked = FALSE
        "#,
    )
    .bind(&token)
    .fetch_optional(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to authorize admin token: {error}")))?
    {
        return Ok(ApiActor {
            actor_type: "admin",
            actor_id: row.get::<String, _>("admin_id"),
        });
    }

    if let Some(row) = sqlx::query(
        r#"
        SELECT access_tokens.user_id
        FROM access_tokens
        INNER JOIN users ON users.user_id = access_tokens.user_id
        WHERE access_tokens.token = $1 AND users.blocked = FALSE
        "#,
    )
    .bind(&token)
    .fetch_optional(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to authorize access token: {error}")))?
    {
        return Ok(ApiActor {
            actor_type: "user",
            actor_id: row.get::<String, _>("user_id"),
        });
    }

    Err(ApiError::unauthorized("access token is invalid or expired"))
}

async fn authorize_signal_actor(state: &AppState, token: &str) -> Result<SignalActor, ApiError> {
    if let Some(row) = sqlx::query(
        r#"
        SELECT access_tokens.user_id
        FROM access_tokens
        INNER JOIN users ON users.user_id = access_tokens.user_id
        WHERE access_tokens.token = $1 AND users.blocked = FALSE
        "#,
    )
    .bind(token)
    .fetch_optional(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to authorize signal user: {error}")))?
    {
        return Ok(SignalActor::User {
            user_id: row.get::<String, _>("user_id"),
        });
    }

    if let Some(row) =
        sqlx::query("SELECT device_id FROM devices WHERE device_token = $1 AND blocked = FALSE")
            .bind(token)
            .fetch_optional(&state.db)
            .await
            .map_err(|error| {
                ApiError::internal(format!("failed to authorize signal device: {error}"))
            })?
    {
        return Ok(SignalActor::Device {
            device_id: row.get::<String, _>("device_id"),
        });
    }

    Err(ApiError::unauthorized("signal token is invalid or expired"))
}

async fn authorize_media_session_actor(
    state: &AppState,
    actor: &SignalActor,
    session_id: &str,
) -> Result<(), ApiError> {
    let session = sqlx::query_as::<_, SessionRecord>(
        r#"
        SELECT
            session_id,
            target_device_id,
            user_id,
            expires_at_ms
        FROM sessions
        WHERE session_id = $1
        "#,
    )
    .bind(session_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to read media session: {error}")))?
    .ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "SESSION_NOT_FOUND",
            "session does not exist",
        )
    })?;

    if session.expires_at_ms < now_ms_i64() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "SESSION_EXPIRED",
            "session is expired",
        ));
    }

    let allowed = match actor {
        SignalActor::User { user_id } => session.user_id == *user_id,
        SignalActor::Device { device_id } => session.target_device_id == *device_id,
    };

    if allowed {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "DEVICE_PERMISSION_DENIED",
            "actor is not allowed to use this session",
        ))
    }
}

async fn authorize_device_token(state: &AppState, headers: &HeaderMap) -> Result<String, ApiError> {
    let token = bearer_token(headers)?;
    sqlx::query("SELECT device_id FROM devices WHERE device_token = $1")
        .bind(token)
        .fetch_optional(&state.db)
        .await
        .map_err(|error| ApiError::internal(format!("failed to authorize device token: {error}")))?
        .map(|row| row.get::<String, _>("device_id"))
        .ok_or_else(|| ApiError::unauthorized("device token is invalid or expired"))
}

fn bearer_token(headers: &HeaderMap) -> Result<String, ApiError> {
    let raw = headers
        .get(axum::http::header::AUTHORIZATION)
        .ok_or_else(|| ApiError::unauthorized("authorization header is required"))?;

    let value = raw
        .to_str()
        .map_err(|_| ApiError::unauthorized("authorization header must be valid utf-8"))?;

    value
        .strip_prefix("Bearer ")
        .map(ToOwned::to_owned)
        .ok_or_else(|| ApiError::unauthorized("authorization header must use Bearer token"))
}

fn bearer_token_or_query(
    headers: &HeaderMap,
    query_token: Option<String>,
) -> Result<String, ApiError> {
    if let Ok(token) = bearer_token(headers) {
        return Ok(token);
    }

    query_token
        .filter(|token| !token.trim().is_empty())
        .map(|token| token.trim().to_owned())
        .ok_or_else(|| {
            ApiError::unauthorized("authorization header or token query parameter is required")
        })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_millis() as u64
}

fn now_ms_i64() -> i64 {
    now_ms() as i64
}

fn short_id() -> String {
    Uuid::new_v4().simple().to_string()[..8].to_owned()
}

async fn next_device_id(db: &PgPool) -> Result<String, ApiError> {
    let next_value = sqlx::query_scalar::<_, Option<i64>>(
        r#"
        SELECT COALESCE(MAX(CASE WHEN device_id ~ '^[0-9]+$' THEN device_id::BIGINT END), 0) + 1
        FROM devices
        "#,
    )
    .fetch_one(db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to allocate next device id: {error}")))?
    .unwrap_or(1);

    Ok(format!("{next_value:03}"))
}

fn current_connect_code(device_id: &str, unix_ms: u64) -> (String, u64) {
    let slot_ms = 30_u64 * 60 * 1000;
    let slot = unix_ms / slot_ms;
    let expires_at_ms = ((slot + 1) * slot_ms).max(unix_ms + 1);
    let hash = stable_code_hash(&format!("{device_id}:{slot}"));
    let code = format!("{:06}", hash % 1_000_000);
    (code, expires_at_ms)
}

fn stable_code_hash(input: &str) -> u32 {
    let mut hash: u32 = 2_166_136_261;
    for byte in input.as_bytes() {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(16_777_619);
    }
    hash
}

async fn fetch_visible_devices(db: &PgPool) -> Result<Vec<DeviceRecord>, ApiError> {
    sqlx::query_as::<_, DeviceRecord>(
        r#"
        SELECT
            device_id,
            device_name,
            owner_user_id,
            hostname,
            os,
            os_version,
            arch,
            username,
            motherboard,
            cpu,
            ram_total_mb,
            ip_addresses,
            mac_addresses,
            group_name,
            department,
            location,
            CASE
                WHEN online = TRUE AND last_seen_ms >= $1 THEN TRUE
                ELSE FALSE
            END AS online,
            last_seen_ms,
            screen_capture,
            input_control,
            accessibility,
            file_transfer,
            blocked
        FROM devices
        WHERE blocked = FALSE
        ORDER BY last_seen_ms DESC, device_name ASC
        "#,
    )
    .bind(device_online_cutoff_ms())
    .fetch_all(db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to list devices: {error}")))
}

async fn find_target_device(db: &PgPool, target: &str) -> Result<DeviceRecord, ApiError> {
    let direct = sqlx::query_as::<_, DeviceRecord>(
        r#"
        SELECT
            device_id,
            device_name,
            owner_user_id,
            hostname,
            os,
            os_version,
            arch,
            username,
            motherboard,
            cpu,
            ram_total_mb,
            ip_addresses,
            mac_addresses,
            group_name,
            department,
            location,
            online,
            last_seen_ms,
            screen_capture,
            input_control,
            accessibility,
            file_transfer,
            blocked
        FROM devices
        WHERE device_id = $1
        "#,
    )
    .bind(target.trim())
    .fetch_optional(db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to read device: {error}")))?;

    if let Some(device) = direct {
        return Ok(device);
    }

    let devices = sqlx::query_as::<_, DeviceRecord>(
        r#"
        SELECT
            device_id,
            device_name,
            owner_user_id,
            hostname,
            os,
            os_version,
            arch,
            username,
            motherboard,
            cpu,
            ram_total_mb,
            ip_addresses,
            mac_addresses,
            group_name,
            department,
            location,
            online,
            last_seen_ms,
            screen_capture,
            input_control,
            accessibility,
            file_transfer,
            blocked
        FROM devices
        "#,
    )
    .fetch_all(db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to resolve target device: {error}")))?;

    let now = now_ms();
    devices
        .into_iter()
        .find(|device| current_connect_code(&device.device_id, now).0 == target.trim())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "DEVICE_NOT_FOUND",
                "device is not registered",
            )
        })
}

fn device_online_cutoff_ms() -> i64 {
    now_ms_i64() - DEVICE_OFFLINE_AFTER_MS
}

fn is_device_online(last_seen_ms: i64, online_flag: bool) -> bool {
    online_flag && last_seen_ms >= device_online_cutoff_ms()
}

async fn register_signal_connection(
    state: &AppState,
    actor_key: String,
    connection_id: &str,
    tx: mpsc::UnboundedSender<Message>,
) {
    let mut connections = state.signal_connections.write().await;
    connections.insert(
        actor_key,
        SignalConnection {
            connection_id: connection_id.to_owned(),
            tx,
        },
    );
}

async fn unregister_signal_connection(state: &AppState, actor_key: &str, connection_id: &str) {
    let mut connections = state.signal_connections.write().await;
    if let Some(entry) = connections.get(actor_key) {
        if entry.connection_id == connection_id {
            connections.remove(actor_key);
        }
    }
}

async fn register_media_connection(
    state: &AppState,
    session_id: &str,
    actor_key: String,
    connection_id: &str,
    tx: mpsc::UnboundedSender<Message>,
) {
    let mut sessions = state.media_connections.write().await;
    let session_connections = sessions.entry(session_id.to_owned()).or_default();
    session_connections.insert(
        actor_key,
        MediaConnection {
            connection_id: connection_id.to_owned(),
            tx,
        },
    );
}

async fn unregister_media_connection(
    state: &AppState,
    session_id: &str,
    actor_key: &str,
    connection_id: &str,
) {
    let mut sessions = state.media_connections.write().await;
    if let Some(session_connections) = sessions.get_mut(session_id) {
        let should_remove = session_connections
            .get(actor_key)
            .map(|connection| connection.connection_id == connection_id)
            .unwrap_or(false);
        if should_remove {
            session_connections.remove(actor_key);
        }
        if session_connections.is_empty() {
            sessions.remove(session_id);
        }
    }
}

async fn relay_media_message(
    state: &AppState,
    session_id: &str,
    actor: &SignalActor,
    bytes: Vec<u8>,
) -> Result<(), ApiError> {
    let session = sqlx::query_as::<_, SessionRecord>(
        r#"
        SELECT
            session_id,
            target_device_id,
            user_id,
            expires_at_ms
        FROM sessions
        WHERE session_id = $1
        "#,
    )
    .bind(session_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to read media session: {error}")))?
    .ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "SESSION_NOT_FOUND",
            "session does not exist",
        )
    })?;

    let recipient_key = match actor {
        SignalActor::User { user_id } if session.user_id == *user_id => SignalActor::Device {
            device_id: session.target_device_id,
        }
        .connection_key(),
        SignalActor::Device { device_id } if session.target_device_id == *device_id => {
            SignalActor::User {
                user_id: session.user_id,
            }
            .connection_key()
        }
        _ => {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "DEVICE_PERMISSION_DENIED",
                "actor is not allowed to use this session",
            ));
        }
    };

    let tx = {
        let sessions = state.media_connections.read().await;
        sessions
            .get(session_id)
            .and_then(|session_connections| session_connections.get(&recipient_key))
            .map(|connection| connection.tx.clone())
    };

    if let Some(tx) = tx {
        tx.send(Message::Binary(bytes.into()))
            .map_err(|_| ApiError::internal("failed to relay media packet"))?;
    }

    Ok(())
}

async fn send_signal_to_actor(
    state: &AppState,
    actor_key: &str,
    payload: Value,
) -> Result<(), ApiError> {
    let tx = {
        let connections = state.signal_connections.read().await;
        connections.get(actor_key).map(|entry| entry.tx.clone())
    };

    if let Some(tx) = tx {
        tx.send(Message::Text(payload.to_string().into()))
            .map_err(|_| {
                ApiError::internal("failed to deliver signaling message to active websocket")
            })?;
    }

    Ok(())
}

async fn send_or_queue_signal(
    state: &AppState,
    actor_key: &str,
    payload: Value,
) -> Result<(), ApiError> {
    let is_connected = {
        let connections = state.signal_connections.read().await;
        connections.contains_key(actor_key)
    };

    if is_connected {
        send_signal_to_actor(state, actor_key, payload).await
    } else {
        queue_pending_signal(&state.db, actor_key, payload).await
    }
}

async fn queue_pending_signal(
    db: &PgPool,
    actor_key: &str,
    payload: Value,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO pending_signals (
            message_id,
            actor_key,
            payload_json,
            created_at_ms
        )
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(format!("msg_{}", short_id()))
    .bind(actor_key)
    .bind(payload.to_string())
    .bind(now_ms_i64())
    .execute(db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to queue signaling message: {error}")))?;

    Ok(())
}

async fn flush_pending_signals(state: &AppState, actor_key: &str) -> Result<(), ApiError> {
    let pending = sqlx::query_as::<_, PendingSignalRecord>(
        r#"
        SELECT
            message_id,
            payload_json
        FROM pending_signals
        WHERE actor_key = $1
        ORDER BY created_at_ms ASC
        "#,
    )
    .bind(actor_key)
    .fetch_all(&state.db)
    .await
    .map_err(|error| {
        ApiError::internal(format!(
            "failed to read pending signaling messages: {error}"
        ))
    })?;

    for message in pending {
        let payload: Value = serde_json::from_str(&message.payload_json).map_err(|error| {
            ApiError::internal(format!(
                "failed to decode queued signaling message: {error}"
            ))
        })?;

        send_signal_to_actor(state, actor_key, payload).await?;

        sqlx::query("DELETE FROM pending_signals WHERE message_id = $1")
            .bind(&message.message_id)
            .execute(&state.db)
            .await
            .map_err(|error| {
                ApiError::internal(format!(
                    "failed to delete delivered signaling message: {error}"
                ))
            })?;
    }

    Ok(())
}

impl From<DeviceRecord> for DeviceSummary {
    fn from(value: DeviceRecord) -> Self {
        let (connect_code, connect_code_expires_at_ms) =
            current_connect_code(&value.device_id, now_ms());
        Self {
            device_id: value.device_id,
            device_name: value.device_name,
            connect_code,
            connect_code_expires_at_ms,
            host_info: HostInfo {
                hostname: value.hostname,
                os: value.os,
                os_version: value.os_version,
                arch: value.arch,
                username: value.username,
                motherboard: value.motherboard.unwrap_or_default(),
                cpu: value.cpu.unwrap_or_default(),
                ram_total_mb: value.ram_total_mb.unwrap_or_default() as u64,
                ip_addresses: parse_string_list(value.ip_addresses.as_deref()),
                mac_addresses: parse_string_list(value.mac_addresses.as_deref()),
            },
            group_name: value.group_name,
            department: value.department,
            location: value.location,
            online: value.online,
            last_seen_ms: value.last_seen_ms as u64,
            permissions: PermissionStatus {
                screen_capture: value.screen_capture,
                input_control: value.input_control,
                accessibility: value.accessibility,
                file_transfer: value.file_transfer,
            },
        }
    }
}

fn serialize_string_list(values: &[String]) -> Option<String> {
    let filtered: Vec<&str> = values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect();

    if filtered.is_empty() {
        None
    } else {
        serde_json::to_string(&filtered).ok()
    }
}

fn parse_string_list(value: Option<&str>) -> Vec<String> {
    value
        .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
        .unwrap_or_default()
}

fn null_if_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn csv_escape(value: &str) -> String {
    let escaped = value.replace('"', "\"\"");
    format!("\"{escaped}\"")
}


impl From<UserRecord> for UserSummary {
    fn from(value: UserRecord) -> Self {
        Self {
            user_id: value.user_id,
            login: value.login,
            role: value.role,
            blocked: value.blocked,
            last_login_at_ms: value.last_login_at_ms as u64,
            desktop_version: DesktopVersion {
                version: value.desktop_version,
                commit: value.desktop_commit,
            },
        }
    }
}

impl From<EnrollmentTokenRecord> for EnrollmentTokenDetailsSummary {
    fn from(value: EnrollmentTokenRecord) -> Self {
        Self {
            token_id: value.token_id,
            token: value.token,
            comment: value.comment,
            expires_at_ms: value.expires_at_ms as u64,
            single_use: value.single_use,
            created_by_user_id: value.created_by_user_id,
            created_at_ms: value.created_at_ms as u64,
            used_at_ms: value.used_at_ms.map(|value| value as u64),
        }
    }
}

async fn run_migrations(db: &PgPool) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            user_id TEXT PRIMARY KEY,
            login TEXT NOT NULL UNIQUE,
            role TEXT NOT NULL DEFAULT 'operator',
            blocked BOOLEAN NOT NULL DEFAULT FALSE,
            last_login_at_ms BIGINT NOT NULL,
            desktop_version TEXT NOT NULL,
            desktop_commit TEXT NOT NULL
        );
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS admins (
            admin_id TEXT PRIMARY KEY,
            login TEXT NOT NULL UNIQUE,
            role TEXT NOT NULL DEFAULT 'admin',
            blocked BOOLEAN NOT NULL DEFAULT FALSE,
            last_login_at_ms BIGINT NOT NULL
        );
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query("ALTER TABLE admins ADD COLUMN IF NOT EXISTS role TEXT NOT NULL DEFAULT 'admin';")
        .execute(db)
        .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS access_tokens (
            token TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            refresh_token TEXT NOT NULL,
            desktop_version TEXT NOT NULL,
            desktop_commit TEXT NOT NULL,
            created_at_ms BIGINT NOT NULL
        );
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO users (user_id, login, role, blocked, last_login_at_ms, desktop_version, desktop_commit)
        SELECT
            user_id,
            user_id,
            'operator',
            FALSE,
            MAX(created_at_ms) AS last_login_at_ms,
            'unknown',
            'unknown'
        FROM access_tokens
        WHERE NOT EXISTS (
            SELECT 1
            FROM users
            WHERE users.user_id = access_tokens.user_id
        )
        GROUP BY user_id
        ON CONFLICT (user_id) DO NOTHING;
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS admin_access_tokens (
            token TEXT PRIMARY KEY,
            admin_id TEXT NOT NULL,
            refresh_token TEXT NOT NULL,
            created_at_ms BIGINT NOT NULL
        );
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO admins (admin_id, login, role, blocked, last_login_at_ms)
        SELECT
            admin_id,
            admin_id,
            'admin',
            FALSE,
            MAX(created_at_ms) AS last_login_at_ms
        FROM admin_access_tokens
        WHERE NOT EXISTS (
            SELECT 1
            FROM admins
            WHERE admins.admin_id = admin_access_tokens.admin_id
        )
        GROUP BY admin_id
        ON CONFLICT (admin_id) DO NOTHING;
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS devices (
            device_id TEXT PRIMARY KEY,
            device_name TEXT NOT NULL,
            owner_user_id TEXT NULL,
            hostname TEXT NOT NULL,
            os TEXT NOT NULL,
            os_version TEXT NOT NULL,
            arch TEXT NOT NULL,
            username TEXT NOT NULL,
            motherboard TEXT NULL,
            cpu TEXT NULL,
            ram_total_mb BIGINT NULL,
            ip_addresses TEXT NULL,
            mac_addresses TEXT NULL,
            group_name TEXT NULL,
            department TEXT NULL,
            location TEXT NULL,
            online BOOLEAN NOT NULL,
            last_seen_ms BIGINT NOT NULL,
            screen_capture BOOLEAN NOT NULL,
            input_control BOOLEAN NOT NULL,
            accessibility BOOLEAN NOT NULL,
            file_transfer BOOLEAN NOT NULL,
            blocked BOOLEAN NOT NULL DEFAULT FALSE,
            device_token TEXT NOT NULL UNIQUE,
            desktop_version TEXT NOT NULL,
            desktop_commit TEXT NOT NULL
        );
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query("ALTER TABLE devices ADD COLUMN IF NOT EXISTS owner_user_id TEXT NULL;")
        .execute(db)
        .await?;
    sqlx::query("ALTER TABLE devices ADD COLUMN IF NOT EXISTS motherboard TEXT NULL;")
        .execute(db)
        .await?;
    sqlx::query("ALTER TABLE devices ADD COLUMN IF NOT EXISTS cpu TEXT NULL;")
        .execute(db)
        .await?;
    sqlx::query("ALTER TABLE devices ADD COLUMN IF NOT EXISTS ram_total_mb BIGINT NULL;")
        .execute(db)
        .await?;
    sqlx::query("ALTER TABLE devices ADD COLUMN IF NOT EXISTS ip_addresses TEXT NULL;")
        .execute(db)
        .await?;
    sqlx::query("ALTER TABLE devices ADD COLUMN IF NOT EXISTS mac_addresses TEXT NULL;")
        .execute(db)
        .await?;
    sqlx::query("ALTER TABLE devices ADD COLUMN IF NOT EXISTS group_name TEXT NULL;")
        .execute(db)
        .await?;
    sqlx::query("ALTER TABLE devices ADD COLUMN IF NOT EXISTS department TEXT NULL;")
        .execute(db)
        .await?;
    sqlx::query("ALTER TABLE devices ADD COLUMN IF NOT EXISTS location TEXT NULL;")
        .execute(db)
        .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS sessions (
            session_id TEXT PRIMARY KEY,
            target_device_id TEXT NOT NULL REFERENCES devices(device_id) ON DELETE CASCADE,
            user_id TEXT NOT NULL,
            session_token TEXT NOT NULL UNIQUE,
            expires_at_ms BIGINT NOT NULL,
            created_at_ms BIGINT NOT NULL
        );
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS enrollment_tokens (
            token_id TEXT PRIMARY KEY,
            token TEXT NOT NULL UNIQUE,
            comment TEXT NOT NULL,
            expires_at_ms BIGINT NOT NULL,
            single_use BOOLEAN NOT NULL,
            created_by_user_id TEXT NOT NULL,
            created_at_ms BIGINT NOT NULL,
            used_at_ms BIGINT NULL
        );
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS audit_events (
            event_id TEXT PRIMARY KEY,
            actor_type TEXT NOT NULL,
            actor_id TEXT NOT NULL,
            action TEXT NOT NULL,
            target_type TEXT NOT NULL,
            target_id TEXT NOT NULL,
            created_at_ms BIGINT NOT NULL
        );
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS inventory_history (
            record_id TEXT PRIMARY KEY,
            device_id TEXT NOT NULL REFERENCES devices(device_id) ON DELETE CASCADE,
            motherboard TEXT NULL,
            cpu TEXT NULL,
            ram_total_mb BIGINT NULL,
            ip_addresses TEXT NULL,
            mac_addresses TEXT NULL,
            os TEXT NOT NULL,
            os_version TEXT NOT NULL,
            arch TEXT NOT NULL,
            username TEXT NOT NULL,
            hostname TEXT NOT NULL,
            recorded_at_ms BIGINT NOT NULL
        );
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS pending_signals (
            message_id TEXT PRIMARY KEY,
            actor_key TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            created_at_ms BIGINT NOT NULL
        );
        "#,
    )
    .execute(db)
    .await?;

    Ok(())
}

async fn record_audit_event(
    db: &PgPool,
    actor_type: &str,
    actor_id: &str,
    action: &str,
    target_type: &str,
    target_id: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            event_id,
            actor_type,
            actor_id,
            action,
            target_type,
            target_id,
            created_at_ms
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(format!("evt_{}", short_id()))
    .bind(actor_type)
    .bind(actor_id)
    .bind(action)
    .bind(target_type)
    .bind(target_id)
    .bind(now_ms_i64())
    .execute(db)
    .await
    .map_err(|error| ApiError::internal(format!("failed to write audit event: {error}")))?;

    Ok(())
}

impl From<AuditEventRecord> for AuditEventSummary {
    fn from(value: AuditEventRecord) -> Self {
        Self {
            event_id: value.event_id,
            actor_type: value.actor_type,
            actor_id: value.actor_id,
            action: value.action,
            target_type: value.target_type,
            target_id: value.target_id,
            created_at_ms: value.created_at_ms as u64,
        }
    }
}

fn normalize_user_role(role: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(role) = role.map(str::trim) else {
        return Ok(None);
    };

    if role.is_empty() {
        return Ok(None);
    }

    match role {
        "operator" | "viewer" => Ok(Some(role.to_owned())),
        _ => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "DEVICE_PERMISSION_DENIED",
            "role must be either operator or viewer",
        )),
    }
}

fn parse_signal_message(raw_message: &str) -> Result<ParsedSignalMessage, ApiError> {
    let payload: Value = serde_json::from_str(raw_message).map_err(|error| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "SIGNAL_INVALID_MESSAGE",
            format!("invalid signaling payload: {error}"),
        )
    })?;

    let message_type = payload.get("type").and_then(Value::as_str).ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "SIGNAL_INVALID_MESSAGE",
            "signaling payload must contain type",
        )
    })?;

    if !matches!(
        message_type,
        "session.offer"
            | "session.answer"
            | "session.ice_candidate"
            | "session.frame_request"
            | "session.frame"
            | "session.cursor"
            | "session.media_feedback"
            | "session.input_mouse"
            | "session.input_key"
            | "session.accepted"
            | "session.rejected"
            | "session.closed"
    ) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "SIGNAL_INVALID_MESSAGE",
            "unsupported signaling message type",
        ));
    }

    let session_id = payload
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "SIGNAL_INVALID_MESSAGE",
                "signaling payload must contain sessionId",
            )
        })?;

    Ok(ParsedSignalMessage {
        message_type: message_type.to_owned(),
        session_id: session_id.to_owned(),
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderValue, header::AUTHORIZATION};

    #[test]
    fn normalize_user_role_accepts_supported_values() {
        assert_eq!(
            normalize_user_role(Some("operator")).expect("operator role should be accepted"),
            Some("operator".to_owned())
        );
        assert_eq!(
            normalize_user_role(Some("viewer")).expect("viewer role should be accepted"),
            Some("viewer".to_owned())
        );
        assert_eq!(
            normalize_user_role(Some("")).expect("empty role should be ignored"),
            None
        );
    }

    #[test]
    fn normalize_user_role_rejects_unknown_values() {
        let error = normalize_user_role(Some("admin")).expect_err("unknown role must fail");
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(error.code, "DEVICE_PERMISSION_DENIED");
    }

    #[test]
    fn parse_signal_message_accepts_supported_payload() {
        let parsed =
            parse_signal_message(r#"{"type":"session.offer","sessionId":"ses_123","sdp":"v=0"}"#)
                .expect("valid signaling payload should parse");

        assert_eq!(parsed.message_type, "session.offer");
        assert_eq!(parsed.session_id, "ses_123");
        assert_eq!(parsed.payload["sdp"], "v=0");
    }

    #[test]
    fn parse_signal_message_rejects_unknown_type() {
        let error = parse_signal_message(r#"{"type":"session.unknown","sessionId":"ses_123"}"#)
            .expect_err("unsupported signaling type must fail");

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(error.code, "SIGNAL_INVALID_MESSAGE");
    }

    #[test]
    fn parse_signal_message_rejects_missing_session_id() {
        let error = parse_signal_message(r#"{"type":"session.answer"}"#)
            .expect_err("missing sessionId must fail");

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(error.code, "SIGNAL_INVALID_MESSAGE");
    }

    #[test]
    fn bearer_token_or_query_prefers_authorization_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer access_header_token"),
        );

        let token = bearer_token_or_query(&headers, Some("query_token".to_owned()))
            .expect("header token should be accepted");

        assert_eq!(token, "access_header_token");
    }

    #[test]
    fn bearer_token_or_query_uses_query_when_header_missing() {
        let headers = HeaderMap::new();
        let token = bearer_token_or_query(&headers, Some("query_token".to_owned()))
            .expect("query token should be accepted");

        assert_eq!(token, "query_token");
    }

    #[test]
    fn signal_actor_connection_key_matches_actor_kind() {
        let user = SignalActor::User {
            user_id: "usr_1".to_owned(),
        };
        let device = SignalActor::Device {
            device_id: "dev_1".to_owned(),
        };

        assert_eq!(user.connection_key(), "user:usr_1");
        assert_eq!(device.connection_key(), "device:dev_1");
    }

    #[test]
    fn device_online_requires_recent_heartbeat() {
        let recent = now_ms_i64() - 1_000;
        let stale = now_ms_i64() - DEVICE_OFFLINE_AFTER_MS - 1_000;

        assert!(is_device_online(recent, true));
        assert!(!is_device_online(stale, true));
        assert!(!is_device_online(recent, false));
    }
}
