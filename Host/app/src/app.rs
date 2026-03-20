use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::{
    App, NativeOptions,
    egui::{self, Align, Color32, Layout, RichText, Stroke, ViewportCommand},
};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::PathBuf,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tray_icon::{
    TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem},
};

use crate::{
    api::{self, DesktopVersion, DeviceRegistration, HostInfoPayload, PermissionStatusPayload},
    signal::{self, SignalEvent},
};

const HOST_SERVICE_NAME: &str = "BKWiverHostService";
const HOST_SERVICE_DISPLAY_NAME: &str = "BK-Host Service";
const HOST_AGENT_TASK_NAME: &str = "BK-Host Agent";
const HOST_STATE_REFRESH_MS: u64 = 2_000;
const HOST_RUNTIME_PUBLISH_MS: u64 = 5_000;
const HOST_AUTO_CONNECT_RETRY_MS: u64 = 10_000;
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
    loop {
        let _ = publish_agent_status(
            "running",
            "Агент Host работает в интерактивном пользовательском сеансе.",
        );
        thread::sleep(Duration::from_millis(HOST_RUNTIME_PUBLISH_MS));
    }
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
            server_url_input: "http://172.16.100.164:8080".to_owned(),
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
        if !self.registration.server_url.trim().is_empty() {
            self.server_url_input = self.registration.server_url.clone();
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
        self.server_url_input
            .trim()
            .trim_end_matches('/')
            .to_owned()
    }

    fn desktop_version(&self) -> DesktopVersion {
        DesktopVersion {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            commit: option_env!("BK_WIVER_COMMIT").unwrap_or("dev").to_owned(),
        }
    }

    fn host_info_payload(&self) -> HostInfoPayload {
        HostInfoPayload {
            hostname: env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown-host".to_owned()),
            os: env::consts::OS.to_owned(),
            os_version: env::var("OS").unwrap_or_else(|_| "unknown".to_owned()),
            arch: env::consts::ARCH.to_owned(),
            username: env::var("USERNAME").unwrap_or_else(|_| "unknown".to_owned()),
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

        let registration = api::connect_host(
            &self.client,
            &server_url,
            login,
            password,
            self.enrollment_token_input.trim(),
            self.desktop_version(),
            self.host_info_payload(),
            self.permissions_payload(),
        )?;

        save_json("device-registration.json", &registration)
            .map_err(|error| format!("Не удалось сохранить регистрацию: {error}"))?;

        self.registration = registration;
        self.server_url_input = self.registration.server_url.clone();
        self.last_heartbeat_attempt_at_ms = 0;
        self.signal_listener_key = None;
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
            self.status_line = error;
        }
    }

    fn send_heartbeat(&mut self) -> Result<(), String> {
        api::send_heartbeat(
            &self.client,
            &self.registration,
            &self.normalized_server_url(),
            self.permissions_payload(),
            now_ms(),
        )?;
        self.last_heartbeat_attempt_at_ms = now_ms();
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
        let server_url = if self.registration.server_url.trim().is_empty() {
            self.normalized_server_url()
        } else {
            self.registration
                .server_url
                .trim()
                .trim_end_matches('/')
                .to_owned()
        };

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
                SignalEvent::Disconnected => {
                    let current = self.agent_status.clone().unwrap_or_default();
                    self.update_agent_runtime(
                        "running",
                        "Signal channel переподключается.",
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
                        &self.registration.server_url,
                        &self.registration.device_token,
                        &session_id,
                    ) {
                        self.status_line =
                            format!("Не удалось подтвердить сеанс {session_id}: {error}");
                    }
                }
                SignalEvent::SessionClosed { session_id } => {
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
            }
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
                            self.status_line = error;
                        }
                        self.refresh();
                    }
                    if ui.button(self.tr("Отправить heartbeat", "Send heartbeat")).clicked() {
                        self.status_line = match self.send_heartbeat() {
                            Ok(()) => self
                                .tr("Heartbeat отправлен на сервер.", "Heartbeat sent to the server.")
                                .to_owned(),
                            Err(error) => error,
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
