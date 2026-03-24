use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

fn normalize_server_url(value: &str) -> String {
    value.trim().trim_end_matches('/').to_owned()
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct DeviceRegistration {
    #[serde(rename = "deviceId")]
    pub device_id: String,
    #[serde(rename = "deviceName")]
    pub device_name: String,
    #[serde(rename = "connectCode")]
    pub connect_code: String,
    #[serde(rename = "serverUrl")]
    pub server_url: String,
    #[serde(rename = "deviceToken", default)]
    pub device_token: String,
    #[serde(rename = "heartbeatIntervalSec", default)]
    pub heartbeat_interval_sec: u32,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopVersion {
    pub version: String,
    pub commit: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfoPayload {
    pub hostname: String,
    pub os: String,
    pub os_version: String,
    pub arch: String,
    pub username: String,
    pub motherboard: String,
    pub cpu: String,
    pub ram_total_mb: u64,
    pub ip_addresses: Vec<String>,
    pub mac_addresses: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionStatusPayload {
    pub screen_capture: bool,
    pub input_control: bool,
    pub accessibility: bool,
    pub file_transfer: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopLoginRequest {
    login: String,
    password: String,
    desktop_version: DesktopVersion,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesktopLoginResponse {
    access_token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceRegistrationRequest {
    enrollment_token: String,
    desktop_version: DesktopVersion,
    host_info: HostInfoPayload,
    permissions: PermissionStatusPayload,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceHeartbeatRequest {
    device_id: String,
    permissions: PermissionStatusPayload,
    unix_time_ms: u64,
}

pub fn connect_host(
    client: &Client,
    server_url: &str,
    login: &str,
    password: &str,
    enrollment_token: &str,
    desktop_version: DesktopVersion,
    host_info: HostInfoPayload,
    permissions: PermissionStatusPayload,
) -> Result<DeviceRegistration, String> {
    let fallback_server_url = normalize_server_url(server_url);
    let login_response = client
        .post(format!("{server_url}/api/v1/auth/login"))
        .json(&DesktopLoginRequest {
            login: login.to_owned(),
            password: password.to_owned(),
            desktop_version: desktop_version.clone(),
        })
        .send()
        .map_err(|error| format!("Не удалось выполнить вход на сервер: {error}"))?;

    if !login_response.status().is_success() {
        return Err(format!(
            "Сервер отклонил вход: {}",
            response_error_text(login_response)
        ));
    }

    let access_token = login_response
        .json::<DesktopLoginResponse>()
        .map_err(|error| format!("Некорректный ответ входа: {error}"))?
        .access_token;

    let register_response = client
        .post(format!("{server_url}/api/v1/devices/register"))
        .bearer_auth(access_token)
        .json(&DeviceRegistrationRequest {
            enrollment_token: enrollment_token.to_owned(),
            desktop_version,
            host_info,
            permissions,
        })
        .send()
        .map_err(|error| format!("Не удалось зарегистрировать Host: {error}"))?;

    if !register_response.status().is_success() {
        return Err(format!(
            "Сервер отклонил регистрацию Host: {}",
            response_error_text(register_response)
        ));
    }

    let mut registration = register_response
        .json::<DeviceRegistration>()
        .map_err(|error| format!("Некорректный ответ регистрации: {error}"))?;

    if !fallback_server_url.is_empty() {
        registration.server_url = fallback_server_url;
    }

    Ok(registration)
}

pub fn send_heartbeat(
    client: &Client,
    registration: &DeviceRegistration,
    fallback_server_url: &str,
    permissions: PermissionStatusPayload,
    unix_time_ms: u64,
) -> Result<(), String> {
    if registration.device_id.trim().is_empty() || registration.device_token.trim().is_empty() {
        return Ok(());
    }

    let fallback_server_url = normalize_server_url(fallback_server_url);
    let server_url = if !fallback_server_url.is_empty() {
        fallback_server_url
    } else {
        normalize_server_url(&registration.server_url)
    };

    if server_url.is_empty() {
        return Err("Не указан адрес сервера для heartbeat.".to_owned());
    }

    let response = client
        .post(format!("{server_url}/api/v1/devices/heartbeat"))
        .bearer_auth(registration.device_token.trim())
        .json(&DeviceHeartbeatRequest {
            device_id: registration.device_id.clone(),
            permissions,
            unix_time_ms,
        })
        .send()
        .map_err(|error| format!("Не удалось отправить heartbeat: {error}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "Сервер отклонил heartbeat: {}",
            response_error_text(response)
        ));
    }

    Ok(())
}

fn response_error_text(response: reqwest::blocking::Response) -> String {
    let status = response.status();
    let body = response.text().unwrap_or_default();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
        if let Some(message) = value
            .get("error")
            .and_then(|item| item.get("message"))
            .and_then(|item| item.as_str())
        {
            return format!("{status}: {message}");
        }
        if let Some(code) = value
            .get("error")
            .and_then(|item| item.get("code"))
            .and_then(|item| item.as_str())
        {
            return format!("{status}: {code}");
        }
    }
    if body.trim().is_empty() {
        status.to_string()
    } else {
        format!("{status}: {}", body.trim())
    }
}
