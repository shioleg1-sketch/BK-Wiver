use crossbeam_channel::{Receiver, Sender, unbounded};
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use eframe::{
    App, NativeOptions,
    egui::{self, Align, Color32, Layout, RichText, Stroke, ViewportCommand},
};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env, fs,
    path::PathBuf,
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use screenshots::Screen;
use tray_icon::{
    TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem},
};

use crate::{
    api::{self, DesktopVersion, DeviceRegistration, HostInfoPayload, PermissionStatusPayload},
    logging,
    media,
    signal::{self, SignalEvent},
};

#[cfg(windows)]
const HOST_SERVICE_NAME: &str = "BKHostService";
const HOST_SERVICE_DISPLAY_NAME: &str = "BK-Host Service";
const HOST_AGENT_TASK_NAME: &str = "BK-Host Agent";
const HOST_STATE_REFRESH_MS: u64 = 2_000;
const HOST_RUNTIME_PUBLISH_MS: u64 = 5_000;
const HOST_AUTO_CONNECT_RETRY_MS: u64 = 10_000;
const DEFAULT_SERVER_URL: &str = "http://wiver.bk.local";
const DEFAULT_SERVER_FALLBACK_URL: &str = "http://172.16.100.164";
const DEFAULT_OPERATOR_LOGIN: &str = "operator";
const DEFAULT_OPERATOR_PASSWORD: &str = "bk-wiver-auto";

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Default)]
enum AppLanguage {
    #[default]
    Ru,
    En,
}

impl AppLanguage {
    fn text(self, ru: &'static str, en: &'static str) -> &'static str {
        match self {
            Self::Ru => ru,
            Self::En => en,
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::Ru => "RU",
            Self::En => "EN",
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
struct HostUiSettings {
    language: AppLanguage,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct ServiceRuntimeStatus {
    mode: String,
    state: String,
    message: String,
    #[serde(rename = "updatedAtMs")]
    updated_at_ms: u64,
    #[serde(rename = "lastHeartbeatAtMs")]
    last_heartbeat_at_ms: u64,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct AgentRuntimeStatus {
    mode: String,
    state: String,
    message: String,
    #[serde(rename = "updatedAtMs")]
    updated_at_ms: u64,
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "sessionRole")]
    session_role: String,
    #[serde(rename = "sessionPeer")]
    session_peer: String,
    #[serde(rename = "signalStatus")]
    signal_status: String,
}

struct HostMediaSession {
    stop_flag: Arc<AtomicBool>,
    profile: Arc<Mutex<media::StreamProfile>>,
    codec_preference: Arc<Mutex<media::StreamCodec>>,
    last_profile_change_at_ms: Arc<Mutex<u64>>,
}

enum HostUiCommand {
    ShowWindow,
    HideWindow,
    Exit,
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    ensure_state_dir()?;

    match runtime_mode() {
        RuntimeMode::Service => run_service_mode(),
        RuntimeMode::Agent => {
            run_agent_mode();
            Ok(())
        }
        RuntimeMode::Tray => run_tray_app(),
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RuntimeMode {
    Tray,
    Agent,
    Service,
}

fn runtime_mode() -> RuntimeMode {
    if env::args_os().any(|arg| arg == "--service") {
        RuntimeMode::Service
    } else if env::args_os().any(|arg| arg == "--agent") {
        RuntimeMode::Agent
    } else {
        RuntimeMode::Tray
    }
}

fn app_build_label() -> String {
    let commit = option_env!("BK_WIVER_COMMIT").unwrap_or("dev");
    let build_id = option_env!("BK_WIVER_BUILD_ID").unwrap_or("local");
    format!("build {} ({})", shorten_commit(commit), build_id)
}

fn shorten_commit(commit: &str) -> String {
    if commit.len() <= 8 {
        commit.to_owned()
    } else {
        commit[..8].to_owned()
    }
}

fn run_tray_app() -> Result<(), Box<dyn std::error::Error>> {
    let (command_tx, command_rx) = unbounded::<HostUiCommand>();
    spawn_tray(command_tx);
    let _ = try_run_agent_task();

    let options = NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("BK-Host")
            .with_inner_size([560.0, 470.0])
            .with_min_inner_size([520.0, 420.0])
            .with_visible(false),
        ..Default::default()
    };

    eframe::run_native(
        "BK-Host",
        options,
        Box::new(|_cc| Ok(Box::new(HostApp::new(command_rx)))),
    )?;

    Ok(())
}

#[cfg(windows)]
fn run_service_mode() -> Result<(), Box<dyn std::error::Error>> {
    windows_host_service::run()
}

#[cfg(not(windows))]
fn run_service_mode() -> Result<(), Box<dyn std::error::Error>> {
    run_service_loop(Arc::new(AtomicBool::new(false)));
    Ok(())
}

fn run_service_loop(stop_flag: Arc<AtomicBool>) {
    let _ = publish_service_status("starting", "Сервис Host запускается.");
    let _ = try_run_agent_task();

    while !stop_flag.load(Ordering::SeqCst) {
        let message = format!(
            "{HOST_SERVICE_DISPLAY_NAME} работает и контролирует интерактивный агент хоста."
        );
        let _ = publish_service_status("running", &message);
        thread::sleep(Duration::from_millis(HOST_RUNTIME_PUBLISH_MS));
    }

    let _ = publish_service_status("stopped", "Сервис Host остановлен.");
}

fn run_agent_mode() {
    #[cfg(windows)]
    let session_details = current_session_details();

    #[cfg(windows)]
    logging::append_log("INFO", "agent.session", &session_details);

    loop {
        let _ = publish_agent_status(
            "running",
            {
                #[cfg(windows)]
                {
                    &session_details
                }
                #[cfg(not(windows))]
                {
                    "Агент Host работает в интерактивном пользовательском сеансе."
                }
            },
        );
        thread::sleep(Duration::from_millis(HOST_RUNTIME_PUBLISH_MS));
    }
}

#[cfg(windows)]
fn current_session_details() -> String {
    use windows_sys::Win32::{
        Foundation::FALSE,
        System::RemoteDesktop::{ProcessIdToSessionId, WTSGetActiveConsoleSessionId},
        UI::WindowsAndMessaging::{GetSystemMetrics, SM_REMOTESESSION},
    };

    let process_id = std::process::id();
    let mut session_id = 0u32;
    let process_session = unsafe { ProcessIdToSessionId(process_id, &mut session_id) };
    let active_console_session = unsafe { WTSGetActiveConsoleSessionId() };
    let remote_session = unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0;

    if process_session == FALSE {
        format!(
            "Агент Host работает. session_id=unknown active_console_session={active_console_session} remote_session={remote_session}"
        )
    } else {
        format!(
            "Агент Host работает. session_id={session_id} active_console_session={active_console_session} remote_session={remote_session}"
        )
    }
}

#[derive(Default)]
struct HostInventoryInfo {
    motherboard: String,
    cpu: String,
    ram_total_mb: u64,
    ip_addresses: Vec<String>,
    mac_addresses: Vec<String>,
}

struct HostApp {
    language: AppLanguage,
    client: Client,
    server_url_input: String,
    login_input: String,
    password_input: String,
    enrollment_token_input: String,
    registration: DeviceRegistration,
    service_status: Option<ServiceRuntimeStatus>,
    agent_status: Option<AgentRuntimeStatus>,
    status_line: String,
    show_id_window: bool,
    main_window_visible: bool,
    last_refresh_at_ms: u64,
    last_heartbeat_attempt_at_ms: u64,
    last_auto_connect_attempt_at_ms: u64,
    signal_listener_key: Option<String>,
    media_sessions: BTreeMap<String, HostMediaSession>,
    command_rx: Receiver<HostUiCommand>,
    signal_rx: Receiver<SignalEvent>,
    signal_tx: Sender<SignalEvent>,
}

impl HostApp {
    fn new(command_rx: Receiver<HostUiCommand>) -> Self {
        let settings = load_json::<HostUiSettings>("ui-settings.json").unwrap_or_default();
        let (signal_tx, signal_rx) = unbounded::<SignalEvent>();
        let mut app = Self {
            language: settings.language,
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| Client::new()),
            server_url_input: DEFAULT_SERVER_URL.to_owned(),
            login_input: DEFAULT_OPERATOR_LOGIN.to_owned(),
            password_input: DEFAULT_OPERATOR_PASSWORD.to_owned(),
            enrollment_token_input: String::new(),
            registration: DeviceRegistration::default(),
            service_status: None,
            agent_status: None,
            status_line: String::new(),
            show_id_window: false,
            main_window_visible: false,
            last_refresh_at_ms: 0,
            last_heartbeat_attempt_at_ms: 0,
            last_auto_connect_attempt_at_ms: 0,
            signal_listener_key: None,
            media_sessions: BTreeMap::new(),
            command_rx,
            signal_rx,
            signal_tx,
        };
        app.refresh();
        app.maybe_auto_connect();
        app
    }

    fn tr(&self, ru: &'static str, en: &'static str) -> &'static str {
        self.language.text(ru, en)
    }

    fn set_language(&mut self, language: AppLanguage) {
        if self.language != language {
            self.language = language;
            let _ = save_json("ui-settings.json", &HostUiSettings { language });
            self.refresh();
        }
    }

    fn refresh(&mut self) {
        self.registration =
            load_json::<DeviceRegistration>("device-registration.json").unwrap_or_default();
        self.service_status = load_json::<ServiceRuntimeStatus>("service-status.json");
        self.agent_status = load_json::<AgentRuntimeStatus>("agent-status.json");
        let registration_server_url = normalize_server_url(&self.registration.server_url);
        if self.server_url_input.trim().is_empty()
            && !registration_server_url.is_empty()
            && !is_loopback_server_url(&registration_server_url)
        {
            self.server_url_input = registration_server_url;
        }
        self.last_refresh_at_ms = now_ms();
        self.status_line = if self.registration.device_id.is_empty() {
            self.tr(
                "Хост ещё не зарегистрирован. Установите и зарегистрируйте его на этом ПК.",
                "Host is not registered yet. Install and register the host on this PC.",
            )
            .to_owned()
        } else {
            self.tr(
                "Хост работает в трее. Нажмите \"Показать ID\", чтобы увидеть текущий идентификатор хоста.",
                "Host runs in the tray. Left-click Show ID to display the current host identifier.",
            )
            .to_owned()
        };
    }

    fn normalized_server_url(&self) -> String {
        normalize_server_url(&self.server_url_input)
    }

    fn active_server_url(&self) -> String {
        let current = self.normalized_server_url();
        if !current.is_empty() {
            return current;
        }

        normalize_server_url(&self.registration.server_url)
    }

    fn desktop_version(&self) -> DesktopVersion {
        DesktopVersion {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            commit: option_env!("BK_WIVER_COMMIT").unwrap_or("dev").to_owned(),
        }
    }

    fn host_info_payload(&self) -> HostInfoPayload {
        let inventory = collect_host_inventory();
        HostInfoPayload {
            hostname: env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown-host".to_owned()),
            os: env::consts::OS.to_owned(),
            os_version: env::var("OS").unwrap_or_else(|_| "unknown".to_owned()),
            arch: env::consts::ARCH.to_owned(),
            username: env::var("USERNAME").unwrap_or_else(|_| "unknown".to_owned()),
            motherboard: inventory.motherboard,
            cpu: inventory.cpu,
            ram_total_mb: inventory.ram_total_mb,
            ip_addresses: inventory.ip_addresses,
            mac_addresses: inventory.mac_addresses,
        }
    }

    fn permissions_payload(&self) -> PermissionStatusPayload {
        PermissionStatusPayload {
            screen_capture: true,
            input_control: true,
            accessibility: true,
            file_transfer: true,
        }
    }

    fn connect_host(&mut self) -> Result<(), String> {
        self.stop_all_media_streams();
        let server_url = self.normalized_server_url();
        let login = if self.login_input.trim().is_empty() {
            DEFAULT_OPERATOR_LOGIN
        } else {
            self.login_input.trim()
        };
        let password = if self.password_input.trim().is_empty() {
            DEFAULT_OPERATOR_PASSWORD
        } else {
            self.password_input.trim()
        };

        if server_url.is_empty() {
            return Err(self
                .tr("Укажите адрес сервера.", "Enter the server URL.")
                .to_owned());
        }

        let mut last_error = None;
        let desktop_version = self.desktop_version();
        let host_info = self.host_info_payload();
        let permissions = self.permissions_payload();
        let mut registration = None;

        for candidate in server_url_candidates(&server_url) {
            match api::connect_host(
                &self.client,
                &candidate,
                login,
                password,
                self.enrollment_token_input.trim(),
                desktop_version.clone(),
                host_info.clone(),
                permissions.clone(),
            ) {
                Ok(value) => {
                    registration = Some(value);
                    break;
                }
                Err(error) => {
                    last_error = Some(error);
                }
            }
        }

        let registration = registration
            .ok_or_else(|| last_error.unwrap_or_else(|| "Не удалось подключиться к серверу.".to_owned()))?;

        save_json("device-registration.json", &registration)
            .map_err(|error| format!("Не удалось сохранить регистрацию: {error}"))?;

        self.registration = registration;
        self.server_url_input = normalize_server_url(&self.registration.server_url);
        self.last_heartbeat_attempt_at_ms = 0;
        self.signal_listener_key = None;
        logging::append_log(
            "INFO",
            "host.connect",
            format!(
                "device_id={} server_url={}",
                self.registration.device_id, self.registration.server_url
            ),
        );
        self.status_line = self
            .tr(
                "Host зарегистрирован на сервере и готов к подключениям.",
                "The host is registered on the server and ready for connections.",
            )
            .to_owned();
        self.ensure_signal_listener();
        Ok(())
    }

    fn maybe_auto_connect(&mut self) {
        if !self.registration.device_id.trim().is_empty() {
            return;
        }

        let now = now_ms();
        if now.saturating_sub(self.last_auto_connect_attempt_at_ms) < HOST_AUTO_CONNECT_RETRY_MS {
            return;
        }
        self.last_auto_connect_attempt_at_ms = now;

        self.status_line = self
            .tr(
                "Host автоматически подключается к серверу.",
                "Host is connecting to the server automatically.",
            )
            .to_owned();

        if let Err(error) = self.connect_host() {
            logging::append_log("WARN", "host.autoconnect", &error);
            self.status_line = error;
        }
    }

    fn send_heartbeat(&mut self) -> Result<(), String> {
        api::send_heartbeat(
            &self.client,
            &self.registration,
            &self.active_server_url(),
            self.permissions_payload(),
            now_ms(),
        )?;
        self.last_heartbeat_attempt_at_ms = now_ms();
        logging::append_log("DEBUG", "host.heartbeat", format!("heartbeat sent device_id={}", self.registration.device_id));
        Ok(())
    }

    fn heartbeat_interval_ms(&self) -> u64 {
        let interval_sec = match self.registration.heartbeat_interval_sec {
            0 => 15,
            value => value,
        };
        (interval_sec as u64) * 1_000
    }

    fn maybe_send_heartbeat(&mut self) {
        if self.registration.device_id.trim().is_empty()
            || self.registration.device_token.trim().is_empty()
        {
            return;
        }

        let now = now_ms();
        if now.saturating_sub(self.last_heartbeat_attempt_at_ms) < self.heartbeat_interval_ms() {
            return;
        }

        if let Err(error) = self.send_heartbeat() {
            self.status_line = error;
        }
    }

    fn ensure_signal_listener(&mut self) {
        let server_url = self.active_server_url();

        if server_url.is_empty() || self.registration.device_token.trim().is_empty() {
            return;
        }

        let key = format!(
            "{}|{}|{}",
            server_url, self.registration.device_id, self.registration.device_token
        );
        if self.signal_listener_key.as_deref() == Some(key.as_str()) {
            return;
        }

        signal::spawn_listener(
            server_url,
            self.registration.device_token.clone(),
            self.signal_tx.clone(),
        );
        self.signal_listener_key = Some(key);
    }

    fn process_signal_events(&mut self) {
        while let Ok(event) = self.signal_rx.try_recv() {
            match event {
                SignalEvent::Connected => {
                    logging::append_log("INFO", "signal", "connected");
                    let current = self.agent_status.clone().unwrap_or_default();
                    self.update_agent_runtime(
                        "running",
                        "Signal channel подключён.",
                        current.session_id,
                        current.session_role,
                        current.session_peer,
                        "connected".to_owned(),
                    );
                }
                SignalEvent::Disconnected { reason } => {
                    logging::append_log(
                        "WARN",
                        "signal",
                        format!("disconnected, reconnecting: {}", reason),
                    );
                    let current = self.agent_status.clone().unwrap_or_default();
                    let runtime_message = format!("Signal channel переподключается: {}", reason);
                    self.update_agent_runtime(
                        "running",
                        &runtime_message,
                        current.session_id,
                        current.session_role,
                        current.session_peer,
                        "reconnecting".to_owned(),
                    );
                }
                SignalEvent::SessionRequested {
                    session_id,
                    from_user_id,
                } => {
                    logging::append_log(
                        "INFO",
                        "session.request",
                        format!("session_id={} from_user_id={}", session_id, from_user_id),
                    );
                    self.status_line =
                        format!("Входящий запрос сеанса {session_id} от {from_user_id}.");
                    self.update_agent_runtime(
                        "running",
                        "Входящий сеанс автоматически подтверждён.",
                        session_id.clone(),
                        "host".to_owned(),
                        from_user_id.clone(),
                        "connected".to_owned(),
                    );

                    if let Err(error) = signal::send_session_accepted(
                        &self.active_server_url(),
                        &self.registration.device_token,
                        &session_id,
                    ) {
                        logging::append_log(
                            "ERROR",
                            "session.accept",
                            format!("session_id={} error={}", session_id, error),
                        );
                        self.status_line =
                            format!("Не удалось подтвердить сеанс {session_id}: {error}");
                    } else {
                        logging::append_log(
                            "INFO",
                            "session.accept",
                            format!("session_id={} accepted", session_id),
                        );
                        self.start_media_stream(&session_id);
                    }
                }
                SignalEvent::SessionClosed { session_id } => {
                    logging::append_log(
                        "INFO",
                        "session.closed",
                        format!("session_id={}", session_id),
                    );
                    self.stop_media_stream(&session_id);
                    let current_signal_status = self
                        .agent_status
                        .as_ref()
                        .map(|value| value.signal_status.clone())
                        .unwrap_or_else(|| "connected".to_owned());
                    self.status_line = format!("Сеанс {session_id} завершён.");
                    self.update_agent_runtime(
                        "running",
                        "Сеанс завершён удалённой стороной.",
                        String::new(),
                        String::new(),
                        String::new(),
                        current_signal_status,
                    );
                }
                SignalEvent::MouseInput {
                    session_id,
                    action,
                    button,
                    x_norm,
                    y_norm,
                    scroll_x,
                    scroll_y,
                } => {
                    match self.apply_mouse_input(
                        x_norm,
                        y_norm,
                        &button,
                        &action,
                        scroll_x,
                        scroll_y,
                    ) {
                        Ok(()) => {
                            self.status_line = format!(
                                "Выполнена команда мыши для сеанса {session_id}."
                            );
                        }
                        Err(error) => {
                            self.status_line = format!(
                                "Не удалось применить мышь для сеанса {session_id}: {error}"
                            );
                        }
                    }
                }
                SignalEvent::KeyInput {
                    session_id,
                    kind,
                    key,
                    text,
                    modifiers,
                } => match self.apply_key_input(&kind, &key, &text, &modifiers) {
                    Ok(()) => {
                        self.status_line =
                            format!("Выполнена клавиатурная команда для сеанса {session_id}.");
                    }
                    Err(error) => {
                        self.status_line = format!(
                            "Не удалось применить клавиатуру для сеанса {session_id}: {error}"
                        );
                    }
                },
                SignalEvent::MediaFeedback {
                    session_id,
                    profile,
                    codec,
                } => {
                    logging::append_log(
                        "INFO",
                        "media.feedback",
                        format!(
                            "session_id={} profile={} codec={}",
                            session_id,
                            profile,
                            codec.clone().unwrap_or_else(|| "none".to_owned())
                        ),
                    );
                    self.update_media_preferences(&session_id, &profile, codec.as_deref());
                }
            }
        }
    }

    fn apply_mouse_input(
        &self,
        x_norm: f32,
        y_norm: f32,
        button: &str,
        action: &str,
        scroll_x: f32,
        scroll_y: f32,
    ) -> Result<(), String> {
        let screen = select_primary_screen().ok_or_else(|| "экран не найден".to_owned())?;
        let info = screen.display_info;
        let width = info.width.max(1) as f32;
        let height = info.height.max(1) as f32;
        let x = info.x + (x_norm.clamp(0.0, 1.0) * width).round() as i32;
        let y = info.y + (y_norm.clamp(0.0, 1.0) * height).round() as i32;

        let mut enigo = Enigo::new(&Settings::default()).map_err(|error| error.to_string())?;
        enigo
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(|error| error.to_string())?;

        if action == "move" {
            return Ok(());
        }

        if action == "scroll" {
            let horizontal = scroll_x.round() as i32;
            let vertical = scroll_y.round() as i32;
            if horizontal != 0 {
                enigo
                    .scroll(horizontal, Axis::Horizontal)
                    .map_err(|error| error.to_string())?;
            }
            if vertical != 0 {
                enigo
                    .scroll(vertical, Axis::Vertical)
                    .map_err(|error| error.to_string())?;
            }
            return Ok(());
        }

        let button = match button {
            "right" => Button::Right,
            "middle" => Button::Middle,
            _ => Button::Left,
        };
        let direction = match action {
            "press" => Direction::Press,
            "release" => Direction::Release,
            _ => Direction::Click,
        };

        enigo
            .button(button, direction)
            .map_err(|error| error.to_string())
    }

    fn apply_key_input(
        &self,
        kind: &str,
        key: &str,
        text: &str,
        modifiers: &[String],
    ) -> Result<(), String> {
        let mut enigo = Enigo::new(&Settings::default()).map_err(|error| error.to_string())?;
        match kind {
            "text" => {
                if !text.is_empty() {
                    enigo.text(text).map_err(|error| error.to_string())?;
                }
            }
            _ => {
                let Some(key) = remote_named_key(key) else {
                    return Ok(());
                };

                let modifier_keys = modifier_keys(modifiers);
                press_modifier_keys(&mut enigo, &modifier_keys)?;
                let key_result = enigo.key(key, Direction::Click);
                let release_result = release_modifier_keys(&mut enigo, &modifier_keys);

                key_result.map_err(|error| error.to_string())?;
                release_result?;
            }
        }

        Ok(())
    }

    fn start_media_stream(&mut self, session_id: &str) {
        if self.media_sessions.contains_key(session_id)
            || self.active_server_url().trim().is_empty()
            || self.registration.device_token.trim().is_empty()
        {
            return;
        }

        let stop_flag = Arc::new(AtomicBool::new(false));
        let profile = Arc::new(Mutex::new(media::StreamProfile::Sharp));
        let codec_preference = Arc::new(Mutex::new(media::StreamCodec::H264));
        media::spawn_stream(
            self.active_server_url(),
            self.registration.device_token.clone(),
            session_id.to_owned(),
            stop_flag.clone(),
            profile.clone(),
            codec_preference.clone(),
        );
        self.media_sessions.insert(
            session_id.to_owned(),
            HostMediaSession {
                stop_flag,
                profile,
                codec_preference,
                last_profile_change_at_ms: Arc::new(Mutex::new(now_ms())),
            },
        );
    }

    fn update_media_preferences(&mut self, session_id: &str, profile: &str, codec: Option<&str>) {
        if let Some(session) = self.media_sessions.get(session_id) {
            let mut status_parts = Vec::new();
            let requested_profile = media::StreamProfile::from_wire(profile);
            let now = now_ms();

            if let Ok(mut current_profile) = session.profile.lock() {
                let current_profile_value = *current_profile;
                let allow_profile_change = if requested_profile == current_profile_value {
                    true
                } else if let Ok(last_change_at_ms) = session.last_profile_change_at_ms.lock() {
                    now.saturating_sub(*last_change_at_ms) >= 5_000
                } else {
                    true
                };

                if allow_profile_change {
                    *current_profile = requested_profile;
                    if requested_profile != current_profile_value
                        && let Ok(mut last_change_at_ms) = session.last_profile_change_at_ms.lock()
                    {
                        *last_change_at_ms = now;
                    }
                    status_parts.push(format!("качество {}", current_profile.wire_name()));
                } else {
                    logging::append_log(
                        "INFO",
                        "media.feedback",
                        format!(
                            "session_id={} ignored_profile_change current={} requested={} cooldown_ms={}",
                            session_id,
                            current_profile_value.wire_name(),
                            requested_profile.wire_name(),
                            5_000
                        ),
                    );
                }
            }

            if let Some(codec) = codec
                && let Ok(mut current_codec) = session.codec_preference.lock()
            {
                *current_codec = media::StreamCodec::from_wire(codec);
                status_parts.push(format!("codec {codec}"));
            }

            if !status_parts.is_empty() {
                self.status_line = format!(
                    "Параметры media для сеанса {session_id}: {}.",
                    status_parts.join(", ")
                );
            }
        }
    }

    fn stop_media_stream(&mut self, session_id: &str) {
        if let Some(session) = self.media_sessions.remove(session_id) {
            session.stop_flag.store(true, Ordering::Relaxed);
        }
    }

    fn stop_all_media_streams(&mut self) {
        for (_, session) in std::mem::take(&mut self.media_sessions) {
            session.stop_flag.store(true, Ordering::Relaxed);
        }
    }

    fn update_agent_runtime(
        &mut self,
        state: &str,
        message: &str,
        session_id: String,
        session_role: String,
        session_peer: String,
        signal_status: String,
    ) {
        let updated = AgentRuntimeStatus {
            mode: "agent".to_owned(),
            state: state.to_owned(),
            message: message.to_owned(),
            updated_at_ms: now_ms(),
            session_id,
            session_role,
            session_peer,
            signal_status,
        };
        let _ = save_json("agent-status.json", &updated);
        self.agent_status = Some(updated);
    }

    fn status_badge(ui: &mut egui::Ui, label: &str, ok: bool) {
        let fill = if ok {
            Color32::from_rgb(215, 238, 219)
        } else {
            Color32::from_rgb(246, 226, 226)
        };
        let text = if ok {
            Color32::from_rgb(39, 105, 54)
        } else {
            Color32::from_rgb(145, 47, 47)
        };
        egui::Frame::new()
            .fill(fill)
            .stroke(Stroke::new(1.0, text))
            .corner_radius(4)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.label(RichText::new(label).color(text).strong());
            });
    }

    fn handle_commands(&mut self, ctx: &egui::Context) {
        while let Ok(command) = self.command_rx.try_recv() {
            match command {
                HostUiCommand::ShowWindow => {
                    self.main_window_visible = true;
                    self.show_id_window = true;
                    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }
                HostUiCommand::HideWindow => {
                    self.main_window_visible = false;
                    self.show_id_window = false;
                    ctx.send_viewport_cmd(ViewportCommand::Visible(false));
                }
                HostUiCommand::Exit => {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
    }
}

fn normalize_server_url(value: &str) -> String {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }

    if trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("ws://")
        || trimmed.starts_with("wss://")
    {
        trimmed.to_owned()
    } else {
        format!("http://{trimmed}")
    }
}

fn is_loopback_server_url(value: &str) -> bool {
    let normalized = normalize_server_url(value).to_ascii_lowercase();
    normalized.starts_with("http://127.0.0.1")
        || normalized.starts_with("https://127.0.0.1")
        || normalized.starts_with("ws://127.0.0.1")
        || normalized.starts_with("wss://127.0.0.1")
        || normalized.starts_with("http://localhost")
        || normalized.starts_with("https://localhost")
        || normalized.starts_with("ws://localhost")
        || normalized.starts_with("wss://localhost")
        || normalized == "127.0.0.1"
        || normalized == "localhost"
}

fn server_url_candidates(primary: &str) -> Vec<String> {
    let primary = normalize_server_url(primary);
    if primary.is_empty() {
        return Vec::new();
    }

    let mut candidates = vec![primary.clone()];
    let fallback = normalize_server_url(DEFAULT_SERVER_FALLBACK_URL);

    if primary == normalize_server_url(DEFAULT_SERVER_URL) && primary != fallback {
        candidates.push(fallback);
    } else if primary == fallback {
        let domain = normalize_server_url(DEFAULT_SERVER_URL);
        if domain != primary {
            candidates.push(domain);
        }
    }

    candidates
}

impl App for HostApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_secs(2));
        self.handle_commands(ctx);
        self.maybe_auto_connect();
        self.ensure_signal_listener();
        self.process_signal_events();
        self.maybe_send_heartbeat();

        if now_ms().saturating_sub(self.last_refresh_at_ms) > HOST_STATE_REFRESH_MS {
            self.refresh();
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("BK-Host");
            ui.label(
                RichText::new(app_build_label())
                    .monospace()
                    .color(Color32::from_rgb(108, 118, 128)),
            );
            ui.label(self.tr(
                "BK-Host устанавливается на ПК пользователя, работает в трее и показывает ID хоста для подключений оператора.",
                "BK-Host is installed on the user PC, runs in the tray, and exposes the host ID for operator connections.",
            ));
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                if ui.button(self.tr("Обновить", "Refresh")).clicked() {
                    self.refresh();
                }
                if ui.button(self.tr("Показать ID", "Show ID")).clicked() {
                    self.show_id_window = true;
                    self.main_window_visible = true;
                    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }
                if ui.button(self.tr("Сохранить лог", "Save log")).clicked() {
                    self.status_line = match logging::export_diagnostic_report() {
                        Ok(path) => {
                            logging::append_log(
                                "INFO",
                                "host.log_export",
                                format!("saved_to={}", path.display()),
                            );
                            format!("Лог сохранён: {}", path.display())
                        }
                        Err(error) => {
                            logging::append_log("ERROR", "host.log_export", &error);
                            format!("Не удалось сохранить лог: {error}")
                        }
                    };
                }
                if ui.button(self.tr("Скрыть", "Hide")).clicked() {
                    self.show_id_window = false;
                    self.main_window_visible = false;
                    ctx.send_viewport_cmd(ViewportCommand::Visible(false));
                }
                if ui.button(self.tr("Запустить агент", "Run Agent")).clicked() {
                    self.status_line = match try_run_agent_task() {
                        Ok(message) => message,
                        Err(message) => message,
                    };
                    self.refresh();
                }
                if ui.button(self.tr("Копировать ID", "Copy ID")).clicked()
                    && !self.registration.device_id.is_empty()
                {
                    ui.ctx().copy_text(self.registration.device_id.clone());
                    self.status_line = self
                        .tr("ID устройства скопирован в буфер обмена.", "Device ID copied to clipboard.")
                        .to_owned();
                }
                if ui.button(self.tr("Копировать код", "Copy Code")).clicked()
                    && !self.registration.connect_code.is_empty()
                {
                    ui.ctx().copy_text(self.registration.connect_code.clone());
                    self.status_line = self
                        .tr("Код подключения скопирован в буфер обмена.", "Connect code copied to clipboard.")
                        .to_owned();
                }
                ui.separator();
                if ui
                    .selectable_label(self.language == AppLanguage::Ru, AppLanguage::Ru.code())
                    .clicked()
                {
                    self.set_language(AppLanguage::Ru);
                }
                if ui
                    .selectable_label(self.language == AppLanguage::En, AppLanguage::En.code())
                    .clicked()
                {
                    self.set_language(AppLanguage::En);
                }
            });

            ui.add_space(12.0);
            ui.label(&self.status_line);
            ui.add_space(12.0);

            ui.group(|ui| {
                ui.label(RichText::new(self.tr("Подключение к серверу", "Server connection")).strong());
                ui.separator();
                labeled_edit(ui, self.tr("Сервер", "Server"), &mut self.server_url_input, false);
                labeled_edit(
                    ui,
                    self.tr("Токен подключения", "Enrollment token"),
                    &mut self.enrollment_token_input,
                    false,
                );
                ui.horizontal(|ui| {
                    if ui.button(self.tr("Подключить Host", "Connect Host")).clicked() {
                        let result = self.connect_host();
                        if let Err(error) = result {
                            logging::append_log("ERROR", "host.connect", &error);
                            self.status_line = error;
                        }
                        self.refresh();
                    }
                    if ui.button(self.tr("Отправить heartbeat", "Send heartbeat")).clicked() {
                        self.status_line = match self.send_heartbeat() {
                            Ok(()) => self
                                .tr("Heartbeat отправлен на сервер.", "Heartbeat sent to the server.")
                                .to_owned(),
                            Err(error) => {
                                logging::append_log("ERROR", "host.heartbeat", &error);
                                error
                            }
                        };
                        self.refresh();
                    }
                });
            });

            ui.add_space(12.0);

            ui.columns(2, |columns| {
                columns[0].group(|ui| {
                    ui.label(RichText::new(self.tr("Устройство", "Device")).strong());
                    ui.separator();
                    field_row(ui, self.tr("ID устройства", "Device ID"), value_or_placeholder(&self.registration.device_id, self.tr("ещё не зарегистрирован", "not registered")));
                    field_row(ui, self.tr("Код подключения", "Connect Code"), value_or_placeholder(&self.registration.connect_code, self.tr("недоступен", "not available")));
                    field_row(ui, self.tr("Имя устройства", "Device Name"), value_or_placeholder(&self.registration.device_name, self.tr("неизвестно", "unknown")));
                    field_row(ui, self.tr("Сервер", "Server"), value_or_placeholder(&self.registration.server_url, self.tr("неизвестен", "unknown")));
                    let heartbeat_interval = if self.registration.heartbeat_interval_sec == 0 {
                        self.tr("не задан", "not set").to_owned()
                    } else {
                        format!("{} {}", self.registration.heartbeat_interval_sec, self.tr("сек", "sec"))
                    };
                    field_row(ui, self.tr("Интервал heartbeat", "Heartbeat interval"), &heartbeat_interval);
                });

                columns[1].group(|ui| {
                    ui.label(RichText::new(self.tr("Состояние", "Runtime")).strong());
                    ui.separator();
                    ui.horizontal(|ui| {
                        Self::status_badge(
                            ui,
                            if self.service_status.is_some() {
                                self.tr("Сервис в сети", "Service online")
                            } else {
                                self.tr("Сервис не в сети", "Service offline")
                            },
                            self.service_status.is_some(),
                        );
                        Self::status_badge(
                            ui,
                            if self.agent_status.is_some() {
                                self.tr("Агент в сети", "Agent online")
                            } else {
                                self.tr("Агент не в сети", "Agent offline")
                            },
                            self.agent_status.is_some(),
                        );
                    });
                    ui.add_space(10.0);
                    if let Some(service) = &self.service_status {
                        field_row(ui, self.tr("Режим сервиса", "Service mode"), &service.mode);
                        field_row(ui, self.tr("Состояние сервиса", "Service state"), &service.state);
                        field_row(ui, self.tr("Обновление сервиса", "Service updated"), &format_ms(service.updated_at_ms, self.tr("никогда", "never")));
                        field_row(ui, self.tr("Heartbeat", "Heartbeat"), &format_ms(service.last_heartbeat_at_ms, self.tr("никогда", "never")));
                        field_row(ui, self.tr("Сообщение сервиса", "Service msg"), &service.message);
                    } else {
                        ui.label(self.tr("Статус сервиса ещё не опубликован.", "Service status is not published yet."));
                    }
                    ui.add_space(8.0);
                    if let Some(agent) = &self.agent_status {
                        field_row(ui, self.tr("Режим агента", "Agent mode"), &agent.mode);
                        field_row(ui, self.tr("Состояние агента", "Agent state"), &agent.state);
                        field_row(ui, self.tr("Обновление агента", "Agent updated"), &format_ms(agent.updated_at_ms, self.tr("никогда", "never")));
                        field_row(ui, self.tr("Сеанс", "Session"), value_or_placeholder(&agent.session_id, self.tr("нет", "none")));
                        field_row(ui, self.tr("Роль", "Role"), value_or_placeholder(&agent.session_role, self.tr("нет", "none")));
                        field_row(ui, self.tr("Пир", "Peer"), value_or_placeholder(&agent.session_peer, self.tr("нет", "none")));
                        field_row(ui, self.tr("Сигнал", "Signal"), value_or_placeholder(&agent.signal_status, self.tr("нет", "none")));
                        field_row(ui, self.tr("Сообщение агента", "Agent msg"), value_or_placeholder(&agent.message, self.tr("нет", "none")));
                    } else {
                        ui.label(self.tr("Статус агента ещё не опубликован.", "Agent status is not published yet."));
                    }
                });
            });

            ui.add_space(12.0);
            ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                ui.colored_label(
                    Color32::from_rgb(53, 90, 156),
                    self.tr(
                        "Host теперь держит постоянный signaling-канал и автоматически подтверждает входящий handshake request/accepted/closed.",
                        "Host now keeps a persistent signaling channel and automatically acknowledges the request/accepted/closed handshake.",
                    ),
                );
            });
        });

        if self.show_id_window {
            let not_registered = self.tr("не зарегистрирован", "not registered");
            let connect_code_label = self.tr("Код подключения", "Connect Code");
            let not_available = self.tr("недоступен", "not available");
            let copy_id_label = self.tr("Копировать ID", "Copy ID");
            let copy_code_label = self.tr("Копировать код", "Copy Code");
            let host_id_window_title = self.tr("ID хоста", "Host ID");
            egui::Window::new(host_id_window_title)
                .collapsible(false)
                .resizable(false)
                .open(&mut self.show_id_window)
                .show(ctx, |ui| {
                    ui.heading(value_or_placeholder(
                        &self.registration.device_id,
                        not_registered,
                    ));
                    ui.add_space(8.0);
                    ui.label(RichText::new(connect_code_label).strong());
                    ui.label(value_or_placeholder(
                        &self.registration.connect_code,
                        not_available,
                    ));
                    ui.add_space(12.0);
                    if ui.button(copy_id_label).clicked() && !self.registration.device_id.is_empty()
                    {
                        ui.ctx().copy_text(self.registration.device_id.clone());
                    }
                    if ui.button(copy_code_label).clicked()
                        && !self.registration.connect_code.is_empty()
                    {
                        ui.ctx().copy_text(self.registration.connect_code.clone());
                    }
                });
        }
    }
}

fn labeled_edit(ui: &mut egui::Ui, label: &str, value: &mut String, password: bool) {
    ui.horizontal(|ui| {
        ui.set_min_width(120.0);
        ui.label(RichText::new(label).strong());
        let mut edit = egui::TextEdit::singleline(value).desired_width(f32::INFINITY);
        if password {
            edit = edit.password(true);
        }
        ui.add(edit);
    });
}

fn field_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.set_min_width(120.0);
        ui.label(RichText::new(label).strong());
        ui.label(value);
    });
}

fn value_or_placeholder<'a>(value: &'a str, placeholder: &'a str) -> &'a str {
    if value.trim().is_empty() {
        placeholder
    } else {
        value
    }
}

#[cfg(windows)]
fn collect_host_inventory() -> HostInventoryInfo {
    let motherboard = powershell_first_line(
        "(Get-CimInstance Win32_BaseBoard | Select-Object -First 1 -ExpandProperty Product)",
    );
    let cpu = powershell_first_line(
        "(Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name)",
    );
    let ram_total_mb = powershell_first_line(
        "[math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1MB)",
    )
    .parse::<u64>()
    .unwrap_or(0);
    let ip_addresses = powershell_lines(
        "Get-NetIPAddress -AddressFamily IPv4 | Where-Object { $_.IPAddress -and $_.IPAddress -ne '127.0.0.1' -and -not $_.IPAddress.StartsWith('169.254.') } | Select-Object -ExpandProperty IPAddress -Unique",
    );
    let mac_addresses = powershell_lines(
        "Get-NetAdapter | Where-Object { $_.MacAddress -and $_.Status -ne 'Disabled' } | Select-Object -ExpandProperty MacAddress -Unique",
    );

    HostInventoryInfo {
        motherboard,
        cpu,
        ram_total_mb,
        ip_addresses,
        mac_addresses,
    }
}

#[cfg(not(windows))]
fn collect_host_inventory() -> HostInventoryInfo {
    HostInventoryInfo::default()
}

#[cfg(windows)]
fn powershell_first_line(script: &str) -> String {
    powershell_lines(script).into_iter().next().unwrap_or_default()
}

#[cfg(windows)]
fn powershell_lines(script: &str) -> Vec<String> {
    let output = Command::new("powershell")
        .arg("-NoProfile")
        .arg("-Command")
        .arg(script)
        .output();

    let Ok(output) = output else {
        return Vec::new();
    };

    if !output.status.success() {
        return Vec::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn load_json<T: for<'de> Deserialize<'de>>(name: &str) -> Option<T> {
    let path = app_state_dir().join(name);
    let body = fs::read_to_string(path).ok()?;
    serde_json::from_str(&body).ok()
}

fn save_json<T: Serialize>(name: &str, value: &T) -> Result<(), String> {
    let path = app_state_dir().join(name);
    let body = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    fs::write(path, body).map_err(|error| error.to_string())
}

fn ensure_state_dir() -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(app_state_dir())?;
    Ok(())
}

fn app_state_dir() -> PathBuf {
    let local_app_data = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    local_app_data.join("BK-Wiver").join("state")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn format_ms(value: u64, zero_label: &str) -> String {
    if value == 0 {
        zero_label.to_owned()
    } else {
        value.to_string()
    }
}

fn select_primary_screen() -> Option<Screen> {
    let screens = Screen::all().ok()?;
    screens
        .iter()
        .find(|screen| screen.display_info.is_primary)
        .copied()
        .or_else(|| screens.into_iter().next())
}

fn modifier_keys(modifiers: &[String]) -> Vec<Key> {
    let mut keys = Vec::new();

    for modifier in modifiers {
        let key = match modifier.as_str() {
            "ctrl" => Key::Control,
            "alt" => Key::Alt,
            "shift" => Key::Shift,
            "meta" => Key::Meta,
            _ => continue,
        };

        if !keys.contains(&key) {
            keys.push(key);
        }
    }

    keys
}

fn press_modifier_keys(enigo: &mut Enigo, modifiers: &[Key]) -> Result<(), String> {
    for key in modifiers {
        enigo
            .key(*key, Direction::Press)
            .map_err(|error| error.to_string())?;
    }

    Ok(())
}

fn release_modifier_keys(enigo: &mut Enigo, modifiers: &[Key]) -> Result<(), String> {
    for key in modifiers.iter().rev() {
        enigo
            .key(*key, Direction::Release)
            .map_err(|error| error.to_string())?;
    }

    Ok(())
}

#[cfg(windows)]
fn remote_named_key(key: &str) -> Option<Key> {
    match key {
        "enter" => Some(Key::Return),
        "tab" => Some(Key::Tab),
        "backspace" => Some(Key::Backspace),
        "escape" => Some(Key::Escape),
        "space" => Some(Key::Space),
        "insert" => Some(Key::Insert),
        "arrow_up" => Some(Key::UpArrow),
        "arrow_down" => Some(Key::DownArrow),
        "arrow_left" => Some(Key::LeftArrow),
        "arrow_right" => Some(Key::RightArrow),
        "delete" => Some(Key::Delete),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "page_up" => Some(Key::PageUp),
        "page_down" => Some(Key::PageDown),
        "a" => Some(Key::A),
        "b" => Some(Key::B),
        "c" => Some(Key::C),
        "d" => Some(Key::D),
        "e" => Some(Key::E),
        "f" => Some(Key::F),
        "g" => Some(Key::G),
        "h" => Some(Key::H),
        "i" => Some(Key::I),
        "j" => Some(Key::J),
        "k" => Some(Key::K),
        "l" => Some(Key::L),
        "m" => Some(Key::M),
        "n" => Some(Key::N),
        "o" => Some(Key::O),
        "p" => Some(Key::P),
        "q" => Some(Key::Q),
        "r" => Some(Key::R),
        "s" => Some(Key::S),
        "t" => Some(Key::T),
        "u" => Some(Key::U),
        "v" => Some(Key::V),
        "w" => Some(Key::W),
        "x" => Some(Key::X),
        "y" => Some(Key::Y),
        "z" => Some(Key::Z),
        "0" => Some(Key::Num0),
        "1" => Some(Key::Num1),
        "2" => Some(Key::Num2),
        "3" => Some(Key::Num3),
        "4" => Some(Key::Num4),
        "5" => Some(Key::Num5),
        "6" => Some(Key::Num6),
        "7" => Some(Key::Num7),
        "8" => Some(Key::Num8),
        "9" => Some(Key::Num9),
        "f1" => Some(Key::F1),
        "f2" => Some(Key::F2),
        "f3" => Some(Key::F3),
        "f4" => Some(Key::F4),
        "f5" => Some(Key::F5),
        "f6" => Some(Key::F6),
        "f7" => Some(Key::F7),
        "f8" => Some(Key::F8),
        "f9" => Some(Key::F9),
        "f10" => Some(Key::F10),
        "f11" => Some(Key::F11),
        "f12" => Some(Key::F12),
        _ => None,
    }
}

#[cfg(not(windows))]
fn remote_named_key(_key: &str) -> Option<Key> {
    None
}

fn publish_service_status(state: &str, message: &str) -> Result<(), String> {
    let now = now_ms();
    save_json(
        "service-status.json",
        &ServiceRuntimeStatus {
            mode: "service".to_owned(),
            state: state.to_owned(),
            message: message.to_owned(),
            updated_at_ms: now,
            last_heartbeat_at_ms: now,
        },
    )
}

fn publish_agent_status(state: &str, message: &str) -> Result<(), String> {
    save_json(
        "agent-status.json",
        &AgentRuntimeStatus {
            mode: "agent".to_owned(),
            state: state.to_owned(),
            message: message.to_owned(),
            updated_at_ms: now_ms(),
            session_id: String::new(),
            session_role: String::new(),
            session_peer: String::new(),
            signal_status: "idle".to_owned(),
        },
    )
}

fn try_run_agent_task() -> Result<String, String> {
    let output = Command::new("schtasks.exe")
        .args(["/Run", "/TN", HOST_AGENT_TASK_NAME])
        .output()
        .map_err(|error| format!("Не удалось запросить задачу агента Host: {error}"))?;

    if output.status.success() {
        Ok(format!(
            "Задача агента успешно запрошена: {HOST_AGENT_TASK_NAME}"
        ))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "Ошибка запуска задачи агента: {}",
            stderr.trim().if_empty_then("неизвестная ошибка schtasks")
        ))
    }
}

trait StringFallback {
    fn if_empty_then(self, fallback: &str) -> String;
}

impl StringFallback for &str {
    fn if_empty_then(self, fallback: &str) -> String {
        if self.trim().is_empty() {
            fallback.to_owned()
        } else {
            self.to_owned()
        }
    }
}

fn spawn_tray(command_tx: Sender<HostUiCommand>) {
    thread::spawn(move || {
        let menu = Menu::new();
        let show_item = MenuItem::new("Показать ID", true, None);
        let hide_item = MenuItem::new("Скрыть", true, None);
        let exit_item = MenuItem::new("Выход", true, None);
        let _ = menu.append(&show_item);
        let _ = menu.append(&hide_item);
        let _ = menu.append(&exit_item);

        let icon = tray_icon::Icon::from_rgba(simple_icon_rgba(), 32, 32)
            .expect("tray icon creation failed");

        let _tray_icon = TrayIconBuilder::new()
            .with_tooltip("BK-Host")
            .with_menu(Box::new(menu))
            .with_icon(icon)
            .build()
            .expect("tray icon build failed");

        let menu_rx = MenuEvent::receiver();
        loop {
            if let Ok(event) = menu_rx.recv() {
                if event.id == show_item.id() {
                    let _ = command_tx.send(HostUiCommand::ShowWindow);
                } else if event.id == hide_item.id() {
                    let _ = command_tx.send(HostUiCommand::HideWindow);
                } else if event.id == exit_item.id() {
                    let _ = command_tx.send(HostUiCommand::Exit);
                    break;
                }
            }
        }
    });
}

fn simple_icon_rgba() -> Vec<u8> {
    let mut rgba = vec![0u8; 32 * 32 * 4];
    for y in 0..32usize {
        for x in 0..32usize {
            let offset = (y * 32 + x) * 4;
            let is_frame = x < 2 || y < 2 || x > 29 || y > 29;
            let is_cross = (x > 12 && x < 20) || (y > 12 && y < 20);
            let (r, g, b, a) = if is_frame {
                (28, 63, 108, 255)
            } else if is_cross {
                (73, 132, 198, 255)
            } else {
                (228, 236, 245, 255)
            };
            rgba[offset] = r;
            rgba[offset + 1] = g;
            rgba[offset + 2] = b;
            rgba[offset + 3] = a;
        }
    }
    rgba
}

#[cfg(windows)]
mod windows_host_service {
    use super::{HOST_SERVICE_DISPLAY_NAME, HOST_SERVICE_NAME, run_service_loop};
    use std::{
        ffi::OsString,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    use windows_service::{
        define_windows_service,
        service::{
            ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
            ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult},
        service_dispatcher,
    };

    define_windows_service!(ffi_service_main, service_main);

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        service_dispatcher::start(HOST_SERVICE_NAME, ffi_service_main)?;
        Ok(())
    }

    fn service_main(_arguments: Vec<OsString>) {
        if let Err(error) = run_inner() {
            eprintln!("{HOST_SERVICE_DISPLAY_NAME} failed: {error}");
        }
    }

    fn run_inner() -> Result<(), Box<dyn std::error::Error>> {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_for_handler = Arc::clone(&stop_flag);

        let status_handle =
            service_control_handler::register(HOST_SERVICE_NAME, move |control| match control {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    stop_flag_for_handler.store(true, Ordering::SeqCst);
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            })?;

        status_handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Default::default(),
            process_id: None,
        })?;

        run_service_loop(Arc::clone(&stop_flag));

        status_handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Default::default(),
            process_id: None,
        })?;

        Ok(())
    }
}
