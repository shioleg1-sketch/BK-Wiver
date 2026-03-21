use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopLoginRequest {
    login: String,
    password: String,
    desktop_version: DesktopVersion,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopLoginResponse {
    pub access_token: String,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    pub hostname: String,
    pub os: String,
    pub os_version: String,
    pub arch: String,
    pub username: String,
}

#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct PermissionStatus {
    pub screen_capture: bool,
    pub input_control: bool,
    pub accessibility: bool,
    pub file_transfer: bool,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSummary {
    pub device_id: String,
    pub device_name: String,
    pub connect_code: String,
    pub connect_code_expires_at_ms: u64,
    pub host_info: HostInfo,
    pub online: bool,
    pub last_seen_ms: u64,
    pub permissions: PermissionStatus,
}

#[derive(Deserialize)]
pub struct ListDevicesResponse {
    pub devices: Vec<DeviceSummary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateSessionRequest {
    device_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionResponse {
    pub session_id: String,
    pub expires_at_ms: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DesktopVersion {
    version: String,
    commit: String,
}

pub fn sign_in(
    client: &Client,
    server_url: &str,
    login: &str,
    password: &str,
) -> Result<DesktopLoginResponse, String> {
    let response = client
        .post(format!("{server_url}/api/v1/auth/login"))
        .json(&DesktopLoginRequest {
            login: login.to_owned(),
            password: password.to_owned(),
            desktop_version: DesktopVersion {
                version: env!("CARGO_PKG_VERSION").to_owned(),
                commit: option_env!("BK_WIVER_COMMIT").unwrap_or("dev").to_owned(),
            },
        })
        .send()
        .map_err(|error| format!("Не удалось выполнить вход: {error}"))?;

    if !response.status().is_success() {
        let code = response.status();
        let body = response
            .text()
            .unwrap_or_else(|_| "пустой ответ".to_owned());
        return Err(format!("Ошибка входа ({code}): {}", summarize_text(&body)));
    }

    response
        .json::<DesktopLoginResponse>()
        .map_err(|error| format!("Вход выполнен, но не удалось разобрать ответ сервера: {error}"))
}

pub fn fetch_devices(
    client: &Client,
    server_url: &str,
    access_token: &str,
) -> Result<ListDevicesResponse, String> {
    let response = client
        .get(format!("{server_url}/api/v1/devices"))
        .bearer_auth(access_token)
        .send()
        .map_err(|error| format!("Не удалось обновить список устройств: {error}"))?;

    if !response.status().is_success() {
        let code = response.status();
        let body = response
            .text()
            .unwrap_or_else(|_| "пустой ответ".to_owned());
        return Err(format!(
            "Ошибка обновления списка устройств ({code}): {}",
            summarize_text(&body)
        ));
    }

    response
        .json::<ListDevicesResponse>()
        .map_err(|error| format!("Не удалось разобрать список устройств: {error}"))
}

pub fn create_session(
    client: &Client,
    server_url: &str,
    access_token: &str,
    device_id: &str,
) -> Result<CreateSessionResponse, String> {
    let response = client
        .post(format!("{server_url}/api/v1/sessions"))
        .bearer_auth(access_token)
        .json(&CreateSessionRequest {
            device_id: device_id.to_owned(),
        })
        .send()
        .map_err(|error| format!("Не удалось создать сеанс: {error}"))?;

    if !response.status().is_success() {
        let code = response.status();
        let body = response
            .text()
            .unwrap_or_else(|_| "пустой ответ".to_owned());
        return Err(format!(
            "Ошибка создания сеанса ({code}): {}",
            summarize_text(&body)
        ));
    }

    response
        .json::<CreateSessionResponse>()
        .map_err(|error| format!("Сеанс создан, но не удалось разобрать ответ: {error}"))
}

fn summarize_text(body: &str) -> String {
    let compact = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.len() > 120 {
        format!("{}...", &compact[..120])
    } else if compact.is_empty() {
        "пустой ответ".to_owned()
    } else {
        compact
    }
}
