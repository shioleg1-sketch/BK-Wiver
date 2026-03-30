use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::{
    App, CreationContext, NativeOptions,
    egui::{
        self, Align, Button, CentralPanel, Color32, Context, CornerRadius, Frame, Layout,
        RichText, ScrollArea, Sense, SidePanel, Stroke, TextEdit, TextureHandle, TopBottomPanel,
        Ui, Vec2, ViewportBuilder,
    },
};
use reqwest::blocking::Client;

use crate::{
    api::{self, DeviceSummary, PermissionStatus},
    logging,
    media::{self, MediaCodec, MediaEvent},
    signal::{self, SignalEvent},
};

const CONSOLE_AUTO_SIGN_IN_RETRY_MS: u64 = 10_000;
const DEFAULT_OPERATOR_LOGIN: &str = "operator";
const DEFAULT_OPERATOR_PASSWORD: &str = "bk-wiver-auto";

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let options = NativeOptions {
        viewport: ViewportBuilder::default()
            .with_inner_size([1360.0, 820.0])
            .with_min_inner_size([1120.0, 700.0]),
        ..Default::default()
    };

    eframe::run_native(
        app_brand_name(),
        options,
        Box::new(|cc| Ok(Box::new(ConsoleApp::new(cc)))),
    )?;

    Ok(())
}

fn app_build_label() -> String {
    let commit = option_env!("BK_WIVER_COMMIT").unwrap_or("dev");
    let build_id = option_env!("BK_WIVER_BUILD_ID").unwrap_or("local");
    format!("build {} ({})", shorten_commit(commit), build_id)
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum HostState {
    Online,
    Busy,
    Offline,
}

impl HostState {
    fn label(self) -> &'static str {
        match self {
            Self::Online => "В сети",
            Self::Busy => "Готов",
            Self::Offline => "Не в сети",
        }
    }

    fn tint(self) -> Color32 {
        match self {
            Self::Online => Color32::from_rgb(40, 140, 70),
            Self::Busy => Color32::from_rgb(40, 90, 150),
            Self::Offline => Color32::from_rgb(140, 148, 158),
        }
    }

    fn fill(self) -> Color32 {
        match self {
            Self::Online => Color32::from_rgb(220, 240, 225),
            Self::Busy => Color32::from_rgb(218, 230, 245),
            Self::Offline => Color32::from_rgb(230, 234, 240),
        }
    }
}

#[derive(Clone)]
struct HostRecord {
    id: String,
    name: String,
    group: String,
    endpoint: String,
    connect_code: String,
    os: String,
    owner: String,
    note: String,
    last_seen: String,
    state: HostState,
    permissions: PermissionStatus,
}

#[derive(Clone)]
struct NavGroup {
    title: String,
    online: usize,
    total: usize,
}

#[derive(Default, Clone)]
struct SessionPreview {
    session_id: String,
    expires_at_ms: u64,
    state: String,
    target_host_id: String,
    target_host_name: String,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum StreamQualityProfile {
    Fast,
    Balanced,
    Sharp,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum StreamCodecPreference {
    Auto,
    H264,
    Vp8,
}

impl StreamCodecPreference {
    fn wire_name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::H264 => "h264",
            Self::Vp8 => "vp8",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::H264 => "H.264",
            Self::Vp8 => "VP8",
        }
    }
}

fn default_stream_codec_preference() -> StreamCodecPreference {
    StreamCodecPreference::Auto
}

impl StreamQualityProfile {
    fn wire_name(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Balanced => "balanced",
            Self::Sharp => "sharp",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Fast => "Fast",
            Self::Balanced => "Balanced",
            Self::Sharp => "Sharp",
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, Eq, PartialEq)]
enum AppLanguage {
    Ru,
    En,
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
enum TextKey {
    AllComputers,
    Connect,
    Refresh,
    Properties,
    Statistics,
    Tools,
    SearchHosts,
    DemoProfile,
    SignedIn,
    Offline,
    Ready,
    HostsCount,
    Connection,
    Server,
    Login,
    Password,
    Reconnect,
    SignIn,
    Demo,
    AddressBook,
    ShowOnlyOnline,
    Activity,
    DemoAddressBook,
    LiveAddressBook,
    LastSession,
    Hosts,
    NoHosts,
    SelectedHost,
    HostId,
    ConnectCode,
    Group,
    Endpoint,
    Platform,
    Owner,
    LastSeen,
    SessionPolicy,
    InteractiveControlAllowed,
    ViewOnlyRestricted,
    View,
    Files,
    CopyId,
    Permissions,
    OperatorNotes,
    DemoRecordNote,
    LiveRecordNote,
    SelectHostPrompt,
    Screen,
    Input,
    Access,
    State,
    Computer,
    ActivityShort,
}

impl AppLanguage {
    fn text(self, key: TextKey) -> &'static str {
        match self {
            Self::Ru => match key {
                TextKey::AllComputers => "Все компьютеры",
                TextKey::Connect => "Подключиться",
                TextKey::Refresh => "Обновить",
                TextKey::Properties => "Свойства",
                TextKey::Statistics => "Статистика",
                TextKey::Tools => "Инструменты",
                TextKey::SearchHosts => "Поиск хостов",
                TextKey::DemoProfile => "Демо-профиль",
                TextKey::SignedIn => "Выполнен вход",
                TextKey::Offline => "Не подключено",
                TextKey::Ready => "Готово",
                TextKey::HostsCount => "Хостов",
                TextKey::Connection => "Подключение",
                TextKey::Server => "Сервер",
                TextKey::Login => "Логин",
                TextKey::Password => "Пароль",
                TextKey::Reconnect => "Переподключить",
                TextKey::SignIn => "Войти",
                TextKey::Demo => "Демо",
                TextKey::AddressBook => "Адресная книга",
                TextKey::ShowOnlyOnline => "Показывать только хосты в сети",
                TextKey::Activity => "Активность",
                TextKey::DemoAddressBook => "Демо-адресная книга",
                TextKey::LiveAddressBook => "Живая адресная книга",
                TextKey::LastSession => "Последний сеанс",
                TextKey::Hosts => "Хосты",
                TextKey::NoHosts => "Нет хостов, подходящих под текущий фильтр.",
                TextKey::SelectedHost => "Выбранный хост",
                TextKey::HostId => "ID хоста",
                TextKey::ConnectCode => "Код подключения",
                TextKey::Group => "Группа",
                TextKey::Endpoint => "Узел",
                TextKey::Platform => "Платформа",
                TextKey::Owner => "Владелец",
                TextKey::LastSeen => "Последняя активность",
                TextKey::SessionPolicy => "Политика сеанса",
                TextKey::InteractiveControlAllowed => "Интерактивное управление разрешено",
                TextKey::ViewOnlyRestricted => "Сначала просмотр или есть ограничения",
                TextKey::View => "Просмотр",
                TextKey::Files => "Файлы",
                TextKey::CopyId => "Копировать ID",
                TextKey::Permissions => "Разрешения",
                TextKey::OperatorNotes => "Заметки оператора",
                TextKey::DemoRecordNote => {
                    "Эта запись загружена из локального встроенного демо-каталога."
                }
                TextKey::LiveRecordNote => {
                    "Эта запись загружена из живого серверного реестра устройств BK-Wiver."
                }
                TextKey::SelectHostPrompt => "Выберите хост, чтобы посмотреть подробности.",
                TextKey::Screen => "Экран",
                TextKey::Input => "Ввод",
                TextKey::Access => "Доступ",
                TextKey::State => "Состояние",
                TextKey::Computer => "Компьютер",
                TextKey::ActivityShort => "Активность",
            },
            Self::En => match key {
                TextKey::AllComputers => "All computers",
                TextKey::Connect => "Connect",
                TextKey::Refresh => "Refresh",
                TextKey::Properties => "Properties",
                TextKey::Statistics => "Statistics",
                TextKey::Tools => "Tools",
                TextKey::SearchHosts => "Search hosts",
                TextKey::DemoProfile => "Demo profile",
                TextKey::SignedIn => "Signed in",
                TextKey::Offline => "Offline",
                TextKey::Ready => "Ready",
                TextKey::HostsCount => "Hosts",
                TextKey::Connection => "Connection",
                TextKey::Server => "Server",
                TextKey::Login => "Login",
                TextKey::Password => "Password",
                TextKey::Reconnect => "Reconnect",
                TextKey::SignIn => "Sign in",
                TextKey::Demo => "Demo",
                TextKey::AddressBook => "Address book",
                TextKey::ShowOnlyOnline => "Show only online hosts",
                TextKey::Activity => "Activity",
                TextKey::DemoAddressBook => "Demo address book",
                TextKey::LiveAddressBook => "Live address book",
                TextKey::LastSession => "Last session",
                TextKey::Hosts => "Hosts",
                TextKey::NoHosts => "No hosts match the current filter.",
                TextKey::SelectedHost => "Selected host",
                TextKey::HostId => "Host ID",
                TextKey::ConnectCode => "Connect code",
                TextKey::Group => "Group",
                TextKey::Endpoint => "Endpoint",
                TextKey::Platform => "Platform",
                TextKey::Owner => "Owner",
                TextKey::LastSeen => "Last seen",
                TextKey::SessionPolicy => "Session policy",
                TextKey::InteractiveControlAllowed => "Interactive control allowed",
                TextKey::ViewOnlyRestricted => "View-first or restricted",
                TextKey::View => "View",
                TextKey::Files => "Files",
                TextKey::CopyId => "Copy ID",
                TextKey::Permissions => "Permissions",
                TextKey::OperatorNotes => "Operator notes",
                TextKey::DemoRecordNote => "This record is loaded from the built-in demo catalog.",
                TextKey::LiveRecordNote => {
                    "This record is loaded from the live BK-Wiver server device registry."
                }
                TextKey::SelectHostPrompt => "Select a host to inspect its details.",
                TextKey::Screen => "Screen",
                TextKey::Input => "Input",
                TextKey::Access => "Access",
                TextKey::State => "State",
                TextKey::Computer => "Computer",
                TextKey::ActivityShort => "Activity",
            },
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::Ru => "RU",
            Self::En => "EN",
        }
    }
}

struct ConsoleApp {
    language: AppLanguage,
    client: Client,
    server_url: String,
    login: String,
    password: String,
    search_query: String,
    quick_connect_id: String,
    status_line: String,
    connection_note: String,
    access_token: Option<String>,
    using_demo_data: bool,
    show_only_online: bool,
    active_group: usize,
    selected_host: usize,
    nav_groups: Vec<NavGroup>,
    hosts: Vec<HostRecord>,
    activity_log: Vec<String>,
    last_session: Option<SessionPreview>,
    show_session_window: bool,
    media_listener_key: Option<String>,
    media_stop_flag: Option<Arc<AtomicBool>>,
    media_connected_session_id: Option<String>,
    media_codec: Option<MediaCodec>,
    media_rx: Receiver<MediaEvent>,
    media_tx: Sender<MediaEvent>,
    session_texture: Option<TextureHandle>,
    media_frame_count: u64,
    media_changed_frame_count: u64,
    media_last_frame_at_ms: u64,
    stream_quality_profile: StreamQualityProfile,
    stream_codec_preference: StreamCodecPreference,
    last_synced_stream_session_id: Option<String>,
    last_synced_stream_profile: Option<StreamQualityProfile>,
    last_synced_stream_codec: Option<StreamCodecPreference>,
    remote_input_captured: bool,
    remote_pointer_button_down: Option<egui::PointerButton>,
    last_remote_pointer_pos: Option<egui::Pos2>,
    remote_last_move_sent_at_ms: u64,
    last_auto_sign_in_attempt_at_ms: u64,
    signal_listener_key: Option<String>,
    signal_tx: Sender<SignalEvent>,
    signal_rx: Receiver<SignalEvent>,
}

const DEFAULT_SERVER_URL: &str = "http://wiver.bk.local";
const REMOTE_MOUSE_SCROLL_SCALE: f32 = 24.0;
const REMOTE_MOVE_INTERVAL_IDLE_MS: u64 = 16;
const REMOTE_MOVE_INTERVAL_DRAG_MS: u64 = 8;
const REMOTE_MOVE_INTERVAL_LOW_FPS_IDLE_MS: u64 = 4;
const REMOTE_MOVE_INTERVAL_LOW_FPS_DRAG_MS: u64 = 0;
const REMOTE_LOW_FPS_THRESHOLD_MS: u64 = 80;

impl ConsoleApp {
    fn new(cc: &CreationContext<'_>) -> Self {
        apply_console_theme(&cc.egui_ctx);

        let hosts = demo_hosts();
        let nav_groups = build_nav_groups(&hosts, AppLanguage::Ru);
        let (signal_tx, signal_rx) = unbounded::<SignalEvent>();
        let (media_tx, media_rx) = unbounded::<MediaEvent>();

        let mut app = Self {
            language: AppLanguage::Ru,
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| Client::new()),
            server_url: DEFAULT_SERVER_URL.to_owned(),
            login: DEFAULT_OPERATOR_LOGIN.to_owned(),
            password: DEFAULT_OPERATOR_PASSWORD.to_owned(),
            search_query: String::new(),
            quick_connect_id: String::new(),
            status_line: "Готово к работе. Автоматическое подключение к серверу.".to_owned(),
            connection_note: "Сейчас используется демо-режим, пока консоль автоматически подключается к серверу BK-Wiver.".to_owned(),
            access_token: None,
            using_demo_data: true,
            show_only_online: false,
            active_group: 0,
            selected_host: 0,
            nav_groups,
            hosts,
            activity_log: vec![
                "09:12  Консоль запущена".to_owned(),
                "09:14  Подключена демо-адресная книга".to_owned(),
                "09:18  Ожидание автоматического входа на сервер".to_owned(),
            ],
            last_session: None,
            show_session_window: false,
            media_listener_key: None,
            media_stop_flag: None,
            media_connected_session_id: None,
            media_codec: None,
            media_rx,
            media_tx,
            session_texture: None,
            media_frame_count: 0,
            media_changed_frame_count: 0,
            media_last_frame_at_ms: 0,
            stream_quality_profile: StreamQualityProfile::Balanced,
            stream_codec_preference: default_stream_codec_preference(),
            last_synced_stream_session_id: None,
            last_synced_stream_profile: None,
            last_synced_stream_codec: None,
            remote_input_captured: false,
            remote_pointer_button_down: None,
            last_remote_pointer_pos: None,
            remote_last_move_sent_at_ms: 0,
            last_auto_sign_in_attempt_at_ms: 0,
            signal_listener_key: None,
            signal_tx,
            signal_rx,
        };
        app.maybe_auto_sign_in();
        app
    }

    fn tr(&self, key: TextKey) -> &'static str {
        self.language.text(key)
    }

    fn set_language(&mut self, language: AppLanguage) {
        if self.language != language {
            self.language = language;
            self.nav_groups = build_nav_groups(&self.hosts, self.language);
        }
    }

    fn signed_in(&self) -> bool {
        self.access_token.is_some()
    }

    fn filtered_host_indices(&self) -> Vec<usize> {
        let group_filter = self
            .nav_groups
            .get(self.active_group)
            .map(|group| group.title.as_str())
            .unwrap_or("Все компьютеры");
        let search = self.search_query.trim().to_ascii_lowercase();

        self.hosts
            .iter()
            .enumerate()
            .filter(|(_, host)| {
                let in_group = group_filter == "Все компьютеры" || host.group == group_filter;
                let passes_online = !self.show_only_online || host.state != HostState::Offline;
                let passes_search = search.is_empty()
                    || host.name.to_ascii_lowercase().contains(&search)
                    || host.id.to_ascii_lowercase().contains(&search)
                    || host.endpoint.to_ascii_lowercase().contains(&search)
                    || host.owner.to_ascii_lowercase().contains(&search);
                in_group && passes_online && passes_search
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn ensure_selection_visible(&mut self) {
        let filtered = self.filtered_host_indices();
        if filtered.is_empty() {
            self.selected_host = 0;
        } else if !filtered.contains(&self.selected_host) {
            self.selected_host = filtered[0];
        }
    }

    fn selected_host(&self) -> Option<&HostRecord> {
        self.hosts.get(self.selected_host)
    }

    fn add_activity(&mut self, message: impl Into<String>) {
        let timestamp = format_clock(now_ms());
        self.activity_log
            .insert(0, format!("{timestamp}  {}", message.into()));
        self.activity_log.truncate(20);
    }

    fn apply_hosts(&mut self, hosts: Vec<HostRecord>, using_demo_data: bool, note: String) {
        self.hosts = hosts;
        self.using_demo_data = using_demo_data;
        self.connection_note = note;
        self.nav_groups = build_nav_groups(&self.hosts, self.language);
        self.active_group = 0;
        self.selected_host = 0;
        self.ensure_selection_visible();
    }

    fn sign_in(&mut self) {
        let server_url = normalize_server_url(&self.server_url);
        let login = if self.login.trim().is_empty() {
            DEFAULT_OPERATOR_LOGIN.to_owned()
        } else {
            self.login.trim().to_owned()
        };
        let password = if self.password.trim().is_empty() {
            DEFAULT_OPERATOR_PASSWORD.to_owned()
        } else {
            self.password.clone()
        };

        match api::sign_in(&self.client, &server_url, &login, &password) {
            Ok(body) => {
                self.server_url = server_url;
                self.access_token = Some(body.access_token);
                self.signal_listener_key = None;
                logging::append_log(
                    "INFO",
                    "console.sign_in",
                    format!("server_url={} login={}", self.server_url, login),
                );
                self.status_line =
                    "Вход выполнен успешно. Загружаю список устройств с сервера.".to_owned();
                self.add_activity("Оператор вошёл в систему");
                self.ensure_signal_listener();
                self.refresh_devices();
            }
            Err(error) => {
                logging::append_log("ERROR", "console.sign_in", &error);
                self.status_line = error;
                self.add_activity("Попытка входа завершилась ошибкой");
            }
        }
    }

    fn maybe_auto_sign_in(&mut self) {
        if self.access_token.is_some() {
            return;
        }

        let now = now_ms();
        if now.saturating_sub(self.last_auto_sign_in_attempt_at_ms) < CONSOLE_AUTO_SIGN_IN_RETRY_MS
        {
            return;
        }
        self.last_auto_sign_in_attempt_at_ms = now;
        self.sign_in();
    }

    fn ensure_signal_listener(&mut self) {
        let Some(token) = self.access_token.clone() else {
            return;
        };
        let server_url = normalize_server_url(&self.server_url);
        if server_url.is_empty() {
            return;
        }

        let key = format!("{server_url}|{token}");
        if self.signal_listener_key.as_deref() == Some(key.as_str()) {
            return;
        }

        signal::spawn_listener(server_url, token, self.signal_tx.clone());
        self.signal_listener_key = Some(key);
    }

    fn process_signal_events(&mut self) {
        while let Ok(event) = self.signal_rx.try_recv() {
            match event {
                SignalEvent::Connected => {
                    logging::append_log("INFO", "signal", "connected");
                    if !self.using_demo_data {
                        self.connection_note = format!(
                            "Подключено к {}. Signal channel активен.",
                            normalize_server_url(&self.server_url)
                        );
                    }
                }
                SignalEvent::Disconnected { reason } => {
                    logging::append_log(
                        "WARN",
                        "signal",
                        format!("disconnected, reconnecting: {}", reason),
                    );
                    if !self.using_demo_data {
                        self.connection_note = format!(
                            "Подключено к {}. Signal channel переподключается: {}.",
                            normalize_server_url(&self.server_url),
                            reason
                        );
                    }
                }
                SignalEvent::SessionAccepted { session_id } => {
                    let mut matched = false;
                    if let Some(session) = &mut self.last_session {
                        if session.session_id == session_id {
                            session.state = "connected".to_owned();
                            matched = true;
                        }
                    }
                    if matched {
                        logging::append_log(
                            "INFO",
                            "session.accepted",
                            format!("session_id={}", session_id),
                        );
                        self.show_session_window = true;
                        self.status_line = format!("Сеанс {session_id} подтверждён хостом.");
                        self.add_activity(format!("Хост подтвердил сеанс {session_id}"));
                        if let Some(session) = self.last_session.clone() {
                            self.sync_stream_profile(&session, "session.accepted");
                        }
                    }
                }
                SignalEvent::SessionRejected { session_id } => {
                    let mut matched = false;
                    if let Some(session) = &mut self.last_session {
                        if session.session_id == session_id {
                            session.state = "rejected".to_owned();
                            matched = true;
                        }
                    }
                    if matched {
                        logging::append_log(
                            "WARN",
                            "session.rejected",
                            format!("session_id={}", session_id),
                        );
                        self.stop_media_listener();
                        self.media_connected_session_id = None;
                        self.media_codec = None;
                        self.remote_input_captured = false;
                        self.remote_pointer_button_down = None;
                        self.last_remote_pointer_pos = None;
                        self.remote_last_move_sent_at_ms = 0;
                        self.status_line = format!("Сеанс {session_id} отклонён хостом.");
                        self.add_activity(format!("Хост отклонил сеанс {session_id}"));
                    }
                }
                SignalEvent::SessionClosed { session_id } => {
                    let mut matched = false;
                    if let Some(session) = &mut self.last_session {
                        if session.session_id == session_id {
                            session.state = "closed".to_owned();
                            matched = true;
                        }
                    }
                    if matched {
                        logging::append_log(
                            "INFO",
                            "session.closed",
                            format!("session_id={}", session_id),
                        );
                        self.stop_media_listener();
                        self.media_connected_session_id = None;
                        self.media_codec = None;
                        self.remote_input_captured = false;
                        self.remote_pointer_button_down = None;
                        self.last_remote_pointer_pos = None;
                        self.remote_last_move_sent_at_ms = 0;
                        self.status_line = format!("Сеанс {session_id} завершён.");
                        self.add_activity(format!("Сеанс {session_id} закрыт"));
                    }
                }
            }
        }
    }

    fn refresh_devices(&mut self) {
        let Some(token) = self.access_token.clone() else {
            self.status_line =
                "Сначала выполните вход, чтобы загрузить список устройств с сервера.".to_owned();
            return;
        };

        let server_url = normalize_server_url(&self.server_url);
        match api::fetch_devices(&self.client, &server_url, &token) {
            Ok(body) => {
                let count = body.devices.len();
                logging::append_log(
                    "INFO",
                    "devices.refresh",
                    format!("count={} server_url={}", count, server_url),
                );
                let hosts = body.devices.into_iter().map(HostRecord::from).collect();
                self.apply_hosts(
                    hosts,
                    false,
                    format!("Подключено к {server_url}. Список устройств теперь загружается из живого реестра сервера."),
                );
                self.status_line = format!("Загружено устройств с сервера: {count}.");
                self.add_activity(format!("Список устройств обновлён с {server_url}"));
            }
            Err(error) => {
                logging::append_log("ERROR", "devices.refresh", &error);
                self.status_line = error;
                self.add_activity("Не удалось обновить список устройств");
            }
        }
    }

    fn switch_to_demo_mode(&mut self) {
        self.stop_media_listener();
        self.access_token = None;
        self.signal_listener_key = None;
        self.last_session = None;
        self.show_session_window = false;
        self.media_connected_session_id = None;
        self.media_codec = None;
        self.session_texture = None;
        self.media_frame_count = 0;
        self.media_changed_frame_count = 0;
        self.media_last_frame_at_ms = 0;
        self.remote_input_captured = false;
        self.remote_pointer_button_down = None;
        self.last_remote_pointer_pos = None;
        self.remote_last_move_sent_at_ms = 0;
        self.apply_hosts(
            demo_hosts(),
            true,
            "Подключены локальные демо-данные. Войдите снова, чтобы загрузить реальный реестр устройств."
                .to_owned(),
        );
        self.status_line = "Демо-режим восстановлен.".to_owned();
        self.add_activity("Возврат к демо-адресной книге");
    }

    fn request_session(&mut self, view_only: bool) {
        let Some(host) = self.selected_host().cloned() else {
            self.status_line = "Выберите хост перед началом сеанса.".to_owned();
            return;
        };

        self.stop_media_listener();
        self.media_connected_session_id = None;
        self.media_codec = None;
        self.session_texture = None;
        self.media_frame_count = 0;
        self.media_changed_frame_count = 0;
        self.media_last_frame_at_ms = 0;
        self.stream_quality_profile = StreamQualityProfile::Balanced;
        self.stream_codec_preference = default_stream_codec_preference();
        self.last_synced_stream_session_id = None;
        self.last_synced_stream_profile = None;
        self.last_synced_stream_codec = None;
        self.remote_input_captured = false;
        self.remote_pointer_button_down = None;
        self.last_remote_pointer_pos = None;
        self.remote_last_move_sent_at_ms = 0;

        if self.using_demo_data || !self.signed_in() {
            self.status_line = if view_only {
                format!(
                    "Запрошен демонстрационный сеанс просмотра для {}.",
                    host.name
                )
            } else {
                format!(
                    "Запрошен демонстрационный сеанс управления для {}.",
                    host.name
                )
            };
            self.last_session = Some(SessionPreview {
                session_id: format!("demo_{}", host.id.replace(' ', "")),
                expires_at_ms: now_ms() + 300_000,
                state: "demo".to_owned(),
                target_host_id: host.id.clone(),
                target_host_name: host.name.clone(),
            });
            self.show_session_window = true;
            self.add_activity(format!("Открыт демонстрационный сеанс для {}", host.name));
            return;
        }

        let Some(token) = self.access_token.clone() else {
            return;
        };

        match api::create_session(
            &self.client,
            &normalize_server_url(&self.server_url),
            &token,
            &host.id,
        ) {
            Ok(body) => {
                logging::append_log(
                    "INFO",
                    "session.create",
                    format!("session_id={} host={}", body.session_id, host.id),
                );
                self.last_session = Some(SessionPreview {
                    session_id: body.session_id.clone(),
                    expires_at_ms: body.expires_at_ms,
                    state: "pending".to_owned(),
                    target_host_id: host.id.clone(),
                    target_host_name: host.name.clone(),
                });
                self.show_session_window = true;
                self.status_line = format!(
                    "Сеанс {} создан для {}. Ожидаем handshake.",
                    body.session_id, host.name
                );
                self.add_activity(format!("Создан сеанс для {}", host.name));
            }
            Err(error) => {
                logging::append_log("ERROR", "session.create", &error);
                self.status_line = error;
                self.add_activity(format!("Не удалось создать сеанс для {}", host.name));
            }
        }
    }

    fn close_session(&mut self) {
        let Some(session) = self.last_session.clone() else {
            return;
        };

        if self.using_demo_data || session.state == "demo" {
            self.status_line = format!("Демонстрационный сеанс {} завершён.", session.session_id);
            if let Some(active) = &mut self.last_session {
                active.state = "closed".to_owned();
            }
            self.stop_media_listener();
            self.media_connected_session_id = None;
            self.media_codec = None;
            self.media_frame_count = 0;
            self.media_changed_frame_count = 0;
            self.media_last_frame_at_ms = 0;
            self.session_texture = None;
            self.remote_input_captured = false;
            self.remote_pointer_button_down = None;
            self.last_remote_pointer_pos = None;
            self.remote_last_move_sent_at_ms = 0;
            return;
        }

        let Some(token) = self.access_token.clone() else {
            return;
        };

        match signal::send_session_closed(
            &normalize_server_url(&self.server_url),
            &token,
            &session.session_id,
        ) {
            Ok(()) => {
                self.status_line =
                    format!("Команда завершения отправлена для {}.", session.session_id);
                if let Some(active) = &mut self.last_session {
                    active.state = "closed".to_owned();
                }
                self.stop_media_listener();
                self.media_connected_session_id = None;
                self.media_codec = None;
                self.media_frame_count = 0;
                self.media_changed_frame_count = 0;
                self.media_last_frame_at_ms = 0;
                self.session_texture = None;
                self.remote_input_captured = false;
                self.remote_pointer_button_down = None;
                self.last_remote_pointer_pos = None;
                self.remote_last_move_sent_at_ms = 0;
                self.add_activity(format!(
                    "Отправлено завершение сеанса {}",
                    session.session_id
                ));
            }
            Err(error) => {
                self.status_line =
                    format!("Не удалось завершить сеанс {}: {error}", session.session_id);
            }
        }
    }

    fn stop_media_listener(&mut self) {
        if let Some(stop_flag) = self.media_stop_flag.take() {
            stop_flag.store(true, Ordering::Relaxed);
        }
        self.media_listener_key = None;
    }

    fn ensure_media_listener(&mut self) {
        if self.using_demo_data {
            self.stop_media_listener();
            return;
        }

        let Some(session) = self.last_session.clone() else {
            self.stop_media_listener();
            return;
        };

        if matches!(session.state.as_str(), "closed" | "rejected") {
            self.stop_media_listener();
            return;
        }

        let Some(token) = self.access_token.clone() else {
            self.stop_media_listener();
            return;
        };

        let server_url = normalize_server_url(&self.server_url);
        let key = format!("{server_url}|{}|{}", session.session_id, token);
        if self.media_listener_key.as_deref() == Some(key.as_str()) {
            return;
        }

        self.stop_media_listener();
        let stop_flag = Arc::new(AtomicBool::new(false));
        media::spawn_listener(
            server_url,
            token,
            session.session_id,
            stop_flag.clone(),
            self.media_tx.clone(),
        );
        self.media_stop_flag = Some(stop_flag);
        self.media_listener_key = Some(key);
    }

    fn process_media_events(&mut self, ctx: &Context) {
        while let Ok(event) = self.media_rx.try_recv() {
            match event {
                MediaEvent::Connected { session_id } => {
                    logging::append_log(
                        "INFO",
                        "media",
                        format!("connected session_id={}", session_id),
                    );
                    if self
                        .last_session
                        .as_ref()
                        .map(|session| session.session_id.as_str())
                        == Some(session_id.as_str())
                    {
                        self.media_connected_session_id = Some(session_id);
                    }
                }
                MediaEvent::Disconnected { session_id } => {
                    logging::append_log(
                        "WARN",
                        "media",
                        format!("disconnected session_id={}", session_id),
                    );
                    if self.media_connected_session_id.as_deref() == Some(session_id.as_str()) {
                        self.media_connected_session_id = None;
                        self.media_codec = None;
                        self.media_frame_count = 0;
                        self.media_changed_frame_count = 0;
                        self.media_last_frame_at_ms = 0;
                        self.session_texture = None;
                    }
                }
                MediaEvent::Frame {
                    session_id,
                    codec,
                    bytes,
                    width,
                    height,
                } => {
                    if self
                        .last_session
                        .as_ref()
                        .map(|session| session.session_id.as_str())
                        != Some(session_id.as_str())
                    {
                        continue;
                    }

                    self.media_codec = Some(codec);
                    self.media_frame_count = self.media_frame_count.saturating_add(1);
                    self.media_last_frame_at_ms = now_ms();
                    self.media_changed_frame_count =
                        self.media_changed_frame_count.saturating_add(1);
                    if self.media_changed_frame_count <= 5
                        || self.media_changed_frame_count % 120 == 0
                    {
                        logging::append_log(
                            "DEBUG",
                            "media.frame",
                            format!(
                                "session_id={} codec={:?} bytes={} changed_frames={}",
                                session_id,
                                codec,
                                bytes.len(),
                                self.media_changed_frame_count
                            ),
                        );
                    }
                    match codec {
                        MediaCodec::H264 => {
                            let (Some(width), Some(height)) = (width, height) else {
                                continue;
                            };
                            let color_image = egui::ColorImage::from_rgba_unmultiplied(
                                [width as usize, height as usize],
                                &bytes,
                            );
                            if let Some(texture) = &mut self.session_texture {
                                texture.set(color_image, egui::TextureOptions::LINEAR);
                            } else {
                                self.session_texture = Some(ctx.load_texture(
                                    "remote-session-frame",
                                    color_image,
                                    egui::TextureOptions::LINEAR,
                                ));
                            }
                            self.media_connected_session_id = Some(session_id);
                            ctx.request_repaint();
                        }
                        MediaCodec::Vp8 => {
                            let (Some(width), Some(height)) = (width, height) else {
                                continue;
                            };
                            let color_image = egui::ColorImage::from_rgba_unmultiplied(
                                [width as usize, height as usize],
                                &bytes,
                            );
                            if let Some(texture) = &mut self.session_texture {
                                texture.set(color_image, egui::TextureOptions::LINEAR);
                            } else {
                                self.session_texture = Some(ctx.load_texture(
                                    "remote-session-frame",
                                    color_image,
                                    egui::TextureOptions::LINEAR,
                                ));
                            }
                            self.media_connected_session_id = Some(session_id);
                            ctx.request_repaint();
                        }
                    }
                }
            }
        }
    }

    fn session_status_label(session_state: &str) -> &'static str {
        match session_state {
            "pending" => "Ожидает подтверждения",
            "connected" => "Подключено",
            "closed" => "Завершён",
            "rejected" => "Отклонён",
            "demo" => "Демо-сеанс",
            _ => "Неизвестно",
        }
    }

    fn session_status_colors(session_state: &str) -> (Color32, Color32) {
        match session_state {
            "connected" => (
                Color32::from_rgb(40, 120, 65),
                Color32::from_rgb(220, 240, 225),
            ),
            "pending" => (
                Color32::from_rgb(40, 95, 155),
                Color32::from_rgb(218, 232, 248),
            ),
            "demo" => (
                Color32::from_rgb(150, 110, 50),
                Color32::from_rgb(248, 240, 225),
            ),
            "rejected" | "closed" => (
                Color32::from_rgb(150, 80, 55),
                Color32::from_rgb(248, 230, 222),
            ),
            _ => (
                Color32::from_rgb(110, 118, 130),
                Color32::from_rgb(232, 236, 242),
            ),
        }
    }

    fn session_stage_note(session_state: &str) -> &'static str {
        match session_state {
            "pending" => {
                "Сеанс создан. Ожидаем подтверждение хоста перед запуском удалённого рабочего стола."
            }
            "connected" => {
                "Handshake завершён. Это подготовленное окно сеанса: сюда следующим шагом можно выводить кадры экрана и принимать ввод."
            }
            "closed" => "Сеанс уже завершён. Окно можно закрыть или создать новый сеанс.",
            "rejected" => {
                "Хост отклонил подключение. Проверьте состояние устройства и повторите попытку."
            }
            "demo" => {
                "Это локальный демонстрационный сеанс. Он помогает проверить UI без живого соединения."
            }
            _ => "Состояние сеанса получено, но ещё не отображается отдельным сценарием.",
        }
    }

    fn remote_view_placeholder(
        &self,
        ui: &mut Ui,
        session: &SessionPreview,
    ) -> (egui::Response, egui::Rect) {
        let available = ui.available_size_before_wrap();
        let desired_height = available.y.max(260.0);
        let desired_size = Vec2::new(available.x.max(320.0), desired_height);
        let (rect, response) = ui.allocate_exact_size(desired_size, Sense::click_and_drag());
        let has_live_frame = self.session_texture.is_some();
        let image_rect = rect.shrink2(Vec2::new(8.0, 40.0));

        let fill = match session.state.as_str() {
            "connected" => Color32::from_rgb(230, 235, 245),
            "pending" => Color32::from_rgb(240, 243, 250),
            "demo" => Color32::from_rgb(248, 245, 235),
            "closed" | "rejected" => Color32::from_rgb(240, 238, 235),
            _ => Color32::from_rgb(235, 238, 245),
        };

        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(8), fill);
        if let Some(texture) = &self.session_texture {
            painter.image(
                texture.id(),
                image_rect,
                egui::Rect::from_min_max(
                    egui::Pos2::new(0.0, 0.0),
                    egui::Pos2::new(1.0, 1.0),
                ),
                Color32::WHITE,
            );
        }
        if has_live_frame
            && self.remote_input_captured
            && let Some(pointer_pos) = self.last_remote_pointer_pos
            && image_rect.contains(pointer_pos)
        {
            painter.circle_filled(
                pointer_pos,
                7.0,
                Color32::from_rgba_unmultiplied(30, 40, 55, 72),
            );
            painter.circle_stroke(
                pointer_pos,
                7.0,
                Stroke::new(1.5, Color32::WHITE),
            );
            painter.line_segment(
                [
                    pointer_pos + Vec2::new(-11.0, 0.0),
                    pointer_pos + Vec2::new(11.0, 0.0),
                ],
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 180)),
            );
            painter.line_segment(
                [
                    pointer_pos + Vec2::new(0.0, -11.0),
                    pointer_pos + Vec2::new(0.0, 11.0),
                ],
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 180)),
            );
        }
        painter.rect_stroke(
            rect,
            CornerRadius::same(8),
            Stroke::new(1.0, Color32::from_rgb(200, 208, 218)),
            egui::StrokeKind::Inside,
        );

        let header_rect = egui::Rect::from_min_size(rect.min, Vec2::new(rect.width(), 34.0));
        painter.rect_filled(
            header_rect,
            CornerRadius {
                nw: 8,
                ne: 8,
                sw: 0,
                se: 0,
            },
            Color32::from_rgba_unmultiplied(0, 0, 0, 15),
        );
        painter.text(
            header_rect.center(),
            egui::Align2::CENTER_CENTER,
            "BK Remote Session",
            egui::FontId::proportional(15.0),
            Color32::from_rgb(60, 72, 90),
        );

        let (accent, accent_fill) = Self::session_status_colors(&session.state);
        let status_rect = egui::Rect::from_center_size(
            if has_live_frame {
                rect.left_top() + Vec2::new(110.0, 52.0)
            } else {
                rect.center_top() + Vec2::new(0.0, 85.0)
            },
            Vec2::new(200.0, 30.0),
        );
        painter.rect_filled(status_rect, CornerRadius::same(15), accent_fill);
        painter.text(
            status_rect.center(),
            egui::Align2::CENTER_CENTER,
            Self::session_status_label(&session.state),
            egui::FontId::proportional(14.0),
            accent,
        );

        if has_live_frame {
            painter.text(
                rect.left_top() + Vec2::new(22.0, 82.0),
                egui::Align2::LEFT_TOP,
                &session.target_host_name,
                egui::FontId::proportional(17.0),
                Color32::from_rgb(40, 52, 70),
            );
        } else {
            painter.text(
                rect.center_top() + Vec2::new(0.0, 135.0),
                egui::Align2::CENTER_CENTER,
                &session.target_host_name,
                egui::FontId::proportional(24.0),
                Color32::from_rgb(40, 52, 70),
            );

            painter.text(
                rect.center_top() + Vec2::new(0.0, 172.0),
                egui::Align2::CENTER_CENTER,
                "Полотно удалённого экрана",
                egui::FontId::proportional(15.0),
                Color32::from_rgb(120, 130, 148),
            );

            painter.text(
                rect.center_top() + Vec2::new(0.0, 205.0),
                egui::Align2::CENTER_CENTER,
                match session.state.as_str() {
                    "connected" => "Ожидаем первые кадры.",
                    "pending" => "Ожидаем подтверждение хоста.",
                    "demo" => "Локальный демонстрационный режим.",
                    "closed" => "Сеанс закрыт.",
                    "rejected" => "Подключение отклонено.",
                    _ => "Ожидаем состояние.",
                },
                egui::FontId::proportional(14.0),
                Color32::from_rgb(140, 148, 162),
            );
        }

        (response, image_rect)
    }

    fn handle_remote_input(
        &mut self,
        ctx: &Context,
        session: &SessionPreview,
        response: &egui::Response,
        image_rect: egui::Rect,
    ) {
        if session.state != "connected" || self.using_demo_data {
            return;
        }

        let Some(token) = self.access_token.clone() else {
            return;
        };

        let server_url = normalize_server_url(&self.server_url);
        let mut pending_move: Option<egui::Pos2> = None;

        let events = ctx.input(|input| input.events.clone());
        for event in events {
            match event {
                egui::Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    ..
                } if image_rect.contains(pos) || (self.remote_input_captured && !pressed) => {
                    self.remote_input_captured = true;
                    self.last_remote_pointer_pos = Some(pos);
                    response.request_focus();

                    let x_norm = ((pos.x - image_rect.left()) / image_rect.width()).clamp(0.0, 1.0);
                    let y_norm = ((pos.y - image_rect.top()) / image_rect.height()).clamp(0.0, 1.0);
                    let button_name = match button {
                        egui::PointerButton::Primary => "left",
                        egui::PointerButton::Secondary => "right",
                        egui::PointerButton::Middle => "middle",
                        egui::PointerButton::Extra1 => "back",
                        egui::PointerButton::Extra2 => "forward",
                    };
                    let action = if pressed { "press" } else { "release" };
                    if pressed {
                        self.remote_pointer_button_down = Some(button);
                    } else if self.remote_pointer_button_down == Some(button) {
                        self.remote_pointer_button_down = None;
                    }
                    if let Err(error) = signal::send_mouse_event(
                        &server_url,
                        &token,
                        &session.session_id,
                        action,
                        button_name,
                        x_norm,
                        y_norm,
                        0.0,
                        0.0,
                    ) {
                        self.status_line = format!("Не удалось отправить мышь: {error}");
                    }
                }
                egui::Event::PointerMoved(pos)
                    if self.remote_input_captured
                        && (image_rect.contains(pos) || self.remote_pointer_button_down.is_some()) =>
                {
                    self.last_remote_pointer_pos = Some(pos);
                    let move_interval_ms = self.remote_move_interval_ms();
                    if now_ms().saturating_sub(self.remote_last_move_sent_at_ms) >= move_interval_ms
                    {
                        if let Err(error) =
                            self.send_remote_mouse_move(&server_url, &token, &session.session_id, pos, image_rect)
                        {
                            self.status_line =
                                format!("Не удалось отправить движение мыши: {error}");
                        }
                    } else {
                        pending_move = Some(pos);
                    }
                }
                egui::Event::MouseWheel { delta, .. } if self.remote_input_captured => {
                    let pointer_pos = ctx
                        .input(|input| input.pointer.hover_pos())
                        .or(self.last_remote_pointer_pos);
                    let Some(pointer_pos) = pointer_pos else {
                        continue;
                    };
                    if !image_rect.contains(pointer_pos) && self.remote_pointer_button_down.is_none() {
                        continue;
                    }

                    let x_norm = ((pointer_pos.x - image_rect.left()) / image_rect.width())
                        .clamp(0.0, 1.0);
                    let y_norm =
                        ((pointer_pos.y - image_rect.top()) / image_rect.height()).clamp(0.0, 1.0);
                    let scroll_x = (delta.x / REMOTE_MOUSE_SCROLL_SCALE).clamp(-6.0, 6.0);
                    let scroll_y = (-delta.y / REMOTE_MOUSE_SCROLL_SCALE).clamp(-6.0, 6.0);
                    if scroll_x != 0.0 || scroll_y != 0.0 {
                        if let Err(error) = signal::send_mouse_event(
                            &server_url,
                            &token,
                            &session.session_id,
                            "scroll",
                            "left",
                            x_norm,
                            y_norm,
                            scroll_x,
                            scroll_y,
                        ) {
                            self.status_line = format!("Не удалось отправить прокрутку: {error}");
                        }
                    }
                }
                egui::Event::PointerGone if self.remote_input_captured => {
                    if self.remote_pointer_button_down.is_none() {
                        self.remote_input_captured = false;
                        self.last_remote_pointer_pos = None;
                        self.remote_last_move_sent_at_ms = 0;
                        self.send_remote_input_reset(&server_url, &token, &session.session_id);
                    }
                }
                egui::Event::Text(text) => {
                    if !self.remote_input_captured {
                        continue;
                    }
                    if text.chars().any(|character| !character.is_control())
                        && let Err(error) = signal::send_key_text(
                            &server_url,
                            &token,
                            &session.session_id,
                            &text,
                        )
                    {
                        self.status_line = format!("Не удалось отправить текст: {error}");
                    }
                }
                egui::Event::Key {
                    key,
                    pressed,
                    repeat,
                    modifiers,
                    ..
                } => {
                    if !self.remote_input_captured {
                        continue;
                    }
                    if key == egui::Key::Escape && pressed && !repeat {
                        self.remote_input_captured = false;
                        self.remote_pointer_button_down = None;
                        self.last_remote_pointer_pos = None;
                        self.remote_last_move_sent_at_ms = 0;
                        self.send_remote_input_reset(&server_url, &token, &session.session_id);
                        self.status_line = "Ввод освобождён от удалённого сеанса.".to_owned();
                        continue;
                    }
                    if let Some(named_key) = map_egui_key(key)
                        && let modifiers = map_egui_modifiers(modifiers)
                        && let Err(error) = signal::send_key_named(
                            &server_url,
                            &token,
                            &session.session_id,
                            named_key,
                            if pressed {
                                if repeat { "repeat" } else { "press" }
                            } else {
                                "release"
                            },
                            &modifiers,
                        )
                    {
                        self.status_line = format!("Не удалось отправить клавишу: {error}");
                    }
                }
                _ => {}
            }
        }

        if let Some(pos) = pending_move {
            if let Err(error) =
                self.send_remote_mouse_move(&server_url, &token, &session.session_id, pos, image_rect)
            {
                self.status_line = format!("Не удалось отправить движение мыши: {error}");
            }
        }
    }

    fn send_remote_mouse_move(
        &mut self,
        server_url: &str,
        token: &str,
        session_id: &str,
        pos: egui::Pos2,
        image_rect: egui::Rect,
    ) -> Result<(), String> {
        let x_norm = ((pos.x - image_rect.left()) / image_rect.width()).clamp(0.0, 1.0);
        let y_norm = ((pos.y - image_rect.top()) / image_rect.height()).clamp(0.0, 1.0);
        signal::send_mouse_event(
            server_url,
            token,
            session_id,
            "move",
            "left",
            x_norm,
            y_norm,
            0.0,
            0.0,
        )?;
        self.remote_last_move_sent_at_ms = now_ms();
        Ok(())
    }

    fn remote_move_interval_ms(&self) -> u64 {
        let degraded_video =
            now_ms().saturating_sub(self.media_last_frame_at_ms) > REMOTE_LOW_FPS_THRESHOLD_MS;
        if self.remote_pointer_button_down.is_some() {
            if degraded_video {
                REMOTE_MOVE_INTERVAL_LOW_FPS_DRAG_MS
            } else {
                REMOTE_MOVE_INTERVAL_DRAG_MS
            }
        } else if degraded_video {
            REMOTE_MOVE_INTERVAL_LOW_FPS_IDLE_MS
        } else {
            REMOTE_MOVE_INTERVAL_IDLE_MS
        }
    }

    fn send_remote_input_reset(&mut self, server_url: &str, token: &str, session_id: &str) {
        if let Err(error) = signal::send_input_reset(server_url, token, session_id) {
            self.status_line = format!("Не удалось сбросить состояние ввода: {error}");
        }
    }

    fn sync_stream_profile(&mut self, session: &SessionPreview, reason: &str) {
        if self.using_demo_data || session.state == "demo" {
            return;
        }
        let Some(token) = self.access_token.clone() else {
            return;
        };

        if self.last_synced_stream_session_id.as_deref() == Some(session.session_id.as_str())
            && self.last_synced_stream_profile == Some(self.stream_quality_profile)
            && self.last_synced_stream_codec == Some(self.stream_codec_preference)
        {
            return;
        }

        if let Err(error) = signal::send_media_feedback(
            &normalize_server_url(&self.server_url),
            &token,
            &session.session_id,
            self.stream_quality_profile.wire_name(),
            self.stream_codec_preference.wire_name(),
        ) {
            self.status_line = format!("Не удалось переключить профиль качества: {error}");
        } else {
            logging::append_log(
                "INFO",
                "stream.profile_sync",
                format!(
                    "session_id={} reason={} profile={} codec={}",
                    session.session_id,
                    reason,
                    self.stream_quality_profile.wire_name(),
                    self.stream_codec_preference.wire_name()
                ),
            );
            self.last_synced_stream_session_id = Some(session.session_id.clone());
            self.last_synced_stream_profile = Some(self.stream_quality_profile);
            self.last_synced_stream_codec = Some(self.stream_codec_preference);
            self.status_line = format!(
                "Параметры потока: {} / {}.",
                self.stream_quality_profile.label(),
                self.stream_codec_preference.label()
            );
        }
    }

    fn session_window(&mut self, ctx: &Context) {
        if !self.show_session_window {
            return;
        }

        let Some(session) = self.last_session.clone() else {
            self.show_session_window = false;
            return;
        };

        let mut open = true;
        egui::Window::new("Сеанс")
            .id(egui::Id::new("session_window"))
            .open(&mut open)
            .default_size(Vec2::new(1280.0, 820.0))
            .min_size(Vec2::new(920.0, 640.0))
            .resizable(true)
            .frame(
                Frame::new()
                    .fill(Color32::from_rgb(248, 250, 254))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(210, 218, 228)))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(0)),
            )
            .show(ctx, |ui| {
                // Session header bar
                Frame::new()
                    .fill(Color32::from_rgb(240, 243, 248))
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(&session.target_host_name)
                                    .size(16.0)
                                    .strong()
                                    .color(Color32::from_rgb(35, 50, 72)),
                            );
                            ui.add_space(10.0);
                            let (status_tint, status_fill) =
                                Self::session_status_colors(&session.state);
                            status_chip(
                                ui,
                                Self::session_status_label(&session.state),
                                status_tint,
                                status_fill,
                            );
                            ui.add_space(14.0);
                            ui.label(
                                RichText::new(format!("ID: {}", session.session_id))
                                    .size(13.0)
                                    .monospace()
                                    .color(Color32::from_rgb(120, 130, 148)),
                            );
                            ui.separator();
                            ui.label(
                                RichText::new(format!(
                                    "Истекает {}",
                                    format_ms(session.expires_at_ms)
                                ))
                                .size(13.0)
                                .color(Color32::from_rgb(120, 130, 148)),
                            );

                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                // Quality selector
                                let previous_profile = self.stream_quality_profile;
                                egui::ComboBox::from_id_salt("session_quality_profile")
                                    .selected_text(
                                        RichText::new(self.stream_quality_profile.label())
                                            .size(13.0)
                                            .color(Color32::from_rgb(60, 72, 90)),
                                    )
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut self.stream_quality_profile,
                                            StreamQualityProfile::Fast,
                                            StreamQualityProfile::Fast.label(),
                                        );
                                        ui.selectable_value(
                                            &mut self.stream_quality_profile,
                                            StreamQualityProfile::Balanced,
                                            StreamQualityProfile::Balanced.label(),
                                        );
                                        ui.selectable_value(
                                            &mut self.stream_quality_profile,
                                            StreamQualityProfile::Sharp,
                                            StreamQualityProfile::Sharp.label(),
                                        );
                                    });

                                // Codec selector
                                let previous_codec = self.stream_codec_preference;
                                egui::ComboBox::from_id_salt("session_codec_preference")
                                    .selected_text(
                                        RichText::new(self.stream_codec_preference.label())
                                            .size(13.0)
                                            .color(Color32::from_rgb(60, 72, 90)),
                                    )
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut self.stream_codec_preference,
                                            StreamCodecPreference::Auto,
                                            StreamCodecPreference::Auto.label(),
                                        );
                                        ui.selectable_value(
                                            &mut self.stream_codec_preference,
                                            StreamCodecPreference::H264,
                                            StreamCodecPreference::H264.label(),
                                        );
                                        ui.selectable_value(
                                            &mut self.stream_codec_preference,
                                            StreamCodecPreference::Vp8,
                                            StreamCodecPreference::Vp8.label(),
                                        );
                                    });

                                if self.stream_quality_profile != previous_profile
                                    || self.stream_codec_preference != previous_codec
                                {
                                    self.sync_stream_profile(&session, "ui.change");
                                }
                            });
                        });
                    });

                ui.add_space(6.0);

                // Remote view
                let (response, image_rect) = self.remote_view_placeholder(ui, &session);
                self.handle_remote_input(ctx, &session, &response, image_rect);

                ui.add_space(6.0);

                // Session footer
                Frame::new()
                    .fill(Color32::from_rgb(242, 245, 250))
                    .inner_margin(egui::Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // Media stats
                            ui.label(
                                RichText::new(if self.media_connected_session_id.as_deref()
                                    == Some(session.session_id.as_str())
                                {
                                    "●"
                                } else {
                                    "○"
                                })
                                .size(12.0)
                                .color(if self.media_connected_session_id.is_some() {
                                    Color32::from_rgb(40, 150, 70)
                                } else {
                                    Color32::from_rgb(140, 148, 158)
                                }),
                            );
                            ui.label(
                                RichText::new(format!(
                                    "кадров: {} | изм: {} | codec: {}",
                                    self.media_frame_count,
                                    self.media_changed_frame_count,
                                    match self.media_codec {
                                        Some(MediaCodec::H264) => "h264",
                                        Some(MediaCodec::Vp8) => "vp8",
                                        None => "—",
                                    }
                                ))
                                .size(13.0)
                                .color(Color32::from_rgb(100, 110, 128)),
                            );

                            ui.add_space(10.0);
                            ui.label(
                                RichText::new(if self.remote_input_captured {
                                    "Ввод захвачен · Esc отпускает"
                                } else {
                                    "Кликните для захвата ввода"
                                })
                                .size(13.0)
                                .color(if self.remote_input_captured {
                                    Color32::from_rgb(180, 130, 40)
                                } else {
                                    Color32::from_rgb(120, 130, 148)
                                }),
                            );

                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add_sized(
                                        [90.0, 28.0],
                                        Button::new(
                                            RichText::new("Скрыть")
                                                .size(13.0)
                                                .color(Color32::from_rgb(60, 72, 90)),
                                        )
                                        .fill(Color32::from_rgb(230, 235, 242))
                                        .stroke(Stroke::new(1.0, Color32::from_rgb(200, 208, 218))),
                                    )
                                    .clicked()
                                {
                                    self.show_session_window = false;
                                    self.remote_input_captured = false;
                                    self.remote_pointer_button_down = None;
                                    self.last_remote_pointer_pos = None;
                                    self.remote_last_move_sent_at_ms = 0;
                                    if let Some(token) = self.access_token.clone() {
                                        self.send_remote_input_reset(
                                            &normalize_server_url(&self.server_url),
                                            &token,
                                            &session.session_id,
                                        );
                                    }
                                }

                                let close_enabled =
                                    !matches!(session.state.as_str(), "closed" | "rejected");
                                if ui
                                    .add_enabled(
                                        close_enabled,
                                        Button::new(
                                            RichText::new("Завершить")
                                                .size(13.0)
                                                .color(Color32::from_rgb(180, 70, 50)),
                                        )
                                        .fill(Color32::from_rgb(252, 235, 230))
                                        .stroke(Stroke::new(1.0, Color32::from_rgb(220, 180, 170)))
                                        .min_size(Vec2::new(90.0, 28.0)),
                                    )
                                    .clicked()
                                {
                                    self.close_session();
                                }
                            });
                        });
                    });
            });

        self.show_session_window = open;
        if !open {
            self.remote_input_captured = false;
            self.remote_pointer_button_down = None;
            self.last_remote_pointer_pos = None;
            self.remote_last_move_sent_at_ms = 0;
            if let Some(token) = self.access_token.clone() {
                self.send_remote_input_reset(
                    &normalize_server_url(&self.server_url),
                    &token,
                    &session.session_id,
                );
            }
        }
    }

    fn top_bar(&mut self, ctx: &Context) {
        TopBottomPanel::top("top_bar")
            .frame(
                Frame::new()
                    .fill(Color32::from_rgb(0, 120, 212))
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(app_brand_name())
                            .size(20.0)
                            .strong()
                            .color(Color32::WHITE),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(app_build_label())
                            .size(12.0)
                            .monospace()
                            .color(Color32::from_rgba_unmultiplied(255, 255, 255, 180)),
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        // Language
                        if ui
                            .add(
                                Button::new(
                                    RichText::new("EN")
                                        .size(13.0)
                                        .color(if self.language == AppLanguage::En {
                                            Color32::WHITE
                                        } else {
                                            Color32::from_rgba_unmultiplied(255, 255, 255, 150)
                                        }),
                                )
                                .fill(if self.language == AppLanguage::En {
                                    Color32::from_rgb(0, 90, 170)
                                } else {
                                    Color32::TRANSPARENT
                                })
                                .stroke(Stroke::NONE),
                            )
                            .clicked()
                        {
                            self.set_language(AppLanguage::En);
                        }
                        if ui
                            .add(
                                Button::new(
                                    RichText::new("RU")
                                        .size(13.0)
                                        .color(if self.language == AppLanguage::Ru {
                                            Color32::WHITE
                                        } else {
                                            Color32::from_rgba_unmultiplied(255, 255, 255, 150)
                                        }),
                                )
                                .fill(if self.language == AppLanguage::Ru {
                                    Color32::from_rgb(0, 90, 170)
                                } else {
                                    Color32::TRANSPARENT
                                })
                                .stroke(Stroke::NONE),
                            )
                            .clicked()
                        {
                            self.set_language(AppLanguage::Ru);
                        }

                        ui.add_space(16.0);

                        // Status
                        let (status_text, status_color) = if self.using_demo_data {
                            ("Demo", Color32::from_rgb(255, 200, 50))
                        } else if self.signed_in() {
                            ("Online", Color32::from_rgb(100, 255, 100))
                        } else {
                            ("Offline", Color32::from_rgb(255, 100, 100))
                        };
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 4.0, status_color);
                        ui.label(
                            RichText::new(status_text)
                                .size(13.0)
                                .color(Color32::from_rgba_unmultiplied(255, 255, 255, 200)),
                        );

                        ui.add_space(16.0);

                        // Search
                        let search_hint = self.tr(TextKey::SearchHosts).to_owned();
                        let response = ui.add(
                            TextEdit::singleline(&mut self.search_query)
                                .hint_text(
                                    RichText::new(search_hint)
                                        .size(13.0)
                                        .color(Color32::from_rgba_unmultiplied(255, 255, 255, 120)),
                                )
                                .desired_width(200.0),
                        );
                        if response.changed() {
                            self.ensure_selection_visible();
                        }
                    });
                });
            });
    }

    fn footer(&self, ctx: &Context) {
        TopBottomPanel::bottom("status_bar")
            .frame(
                Frame::new()
                    .fill(Color32::from_rgb(240, 242, 248))
                    .inner_margin(egui::Margin::symmetric(16, 6)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&self.status_line)
                            .size(12.0)
                            .color(Color32::from_rgb(100, 110, 128)),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!(
                                "{}: {}",
                                self.tr(TextKey::HostsCount),
                                self.filtered_host_indices().len()
                            ))
                            .size(12.0)
                            .color(Color32::from_rgb(140, 148, 160)),
                        );
                    });
                });
            });
    }

    fn left_panel(&mut self, ctx: &Context) {
        SidePanel::left("nav_panel")
            .resizable(true)
            .default_width(240.0)
            .width_range(200.0..=300.0)
            .frame(
                Frame::new()
                    .fill(Color32::from_rgb(242, 245, 250))
                    .inner_margin(egui::Margin::symmetric(10, 12)),
            )
            .show(ctx, |ui| {
                // Server connection
                ui.label(
                    RichText::new(self.tr(TextKey::Server))
                        .size(13.0)
                        .strong()
                        .color(Color32::from_rgb(60, 72, 90)),
                );
                ui.add_space(4.0);
                ui.add(
                    TextEdit::singleline(&mut self.server_url)
                        .desired_width(f32::INFINITY)
                        .font(egui::TextStyle::Monospace.resolve(ui.style())),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_sized(
                            [72.0, 30.0],
                            Button::new(
                                RichText::new(if self.signed_in() {
                                    self.tr(TextKey::Reconnect)
                                } else {
                                    self.tr(TextKey::SignIn)
                                })
                                .size(14.0),
                            ),
                        )
                        .clicked()
                    {
                        self.sign_in();
                    }
                    if ui
                        .add_sized(
                            [68.0, 30.0],
                            Button::new(
                                RichText::new(self.tr(TextKey::Refresh)).size(14.0),
                            ),
                        )
                        .clicked()
                    {
                        self.refresh_devices();
                    }
                    if ui
                        .add_sized(
                            [64.0, 30.0],
                            Button::new(RichText::new(self.tr(TextKey::Demo)).size(14.0)),
                        )
                        .clicked()
                    {
                        self.switch_to_demo_mode();
                    }
                });

                ui.add_space(16.0);
                ui.add_space(12.0);

                // Groups
                ui.label(
                    RichText::new(self.tr(TextKey::AddressBook))
                        .size(13.0)
                        .strong()
                        .color(Color32::from_rgb(60, 72, 90)),
                );
                ui.add_space(6.0);

                // Online filter
                let show_only_online_label = self.tr(TextKey::ShowOnlyOnline).to_owned();
                ui.checkbox(
                    &mut self.show_only_online,
                    RichText::new(show_only_online_label)
                        .size(14.0)
                        .color(Color32::from_rgb(80, 90, 108)),
                );
                ui.add_space(8.0);

                // Groups list
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(300.0)
                    .show(ui, |ui| {
                        let mut clicked_group = None;
                        for (index, group) in self.nav_groups.iter().enumerate() {
                            let selected = self.active_group == index;
                            let bg = if selected {
                                Color32::from_rgb(200, 218, 245)
                            } else {
                                Color32::TRANSPARENT
                            };

                            let response = Frame::new()
                                .fill(bg)
                                .corner_radius(CornerRadius::same(4))
                                .inner_margin(egui::Margin::symmetric(10, 6))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(&group.title)
                                                .size(15.0)
                                                .color(if selected {
                                                    Color32::from_rgb(30, 55, 100)
                                                } else {
                                                    Color32::from_rgb(60, 70, 88)
                                                }),
                                        );
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                ui.label(
                                                    RichText::new(format!(
                                                        "{}/{}",
                                                        group.online, group.total
                                                    ))
                                                    .size(13.0)
                                                    .color(if selected {
                                                        Color32::from_rgb(50, 80, 140)
                                                    } else {
                                                        Color32::from_rgb(120, 130, 148)
                                                    }),
                                                );
                                            },
                                        );
                                    });
                                })
                                .response
                                .interact(Sense::click());

                            if response.clicked() {
                                clicked_group = Some(index);
                            }
                        }

                        if let Some(index) = clicked_group {
                            self.active_group = index;
                            self.ensure_selection_visible();
                        }
                    });

                ui.add_space(12.0);
                ui.add_space(12.0);

                // Activity
                ui.label(
                    RichText::new(self.tr(TextKey::Activity))
                        .size(13.0)
                        .strong()
                        .color(Color32::from_rgb(60, 72, 90)),
                );
                ui.add_space(6.0);
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(180.0)
                    .show(ui, |ui| {
                        for item in &self.activity_log {
                            ui.label(
                                RichText::new(item)
                                    .size(13.0)
                                    .color(Color32::from_rgb(100, 110, 128)),
                            );
                        }
                    });
            });
    }

    fn center_panel(&mut self, ctx: &Context) {
        CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(Color32::from_rgb(240, 242, 245))
                    .inner_margin(egui::Margin::same(0)),
            )
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    // Quick connect bar
                    Frame::new()
                        .fill(Color32::WHITE)
                        .inner_margin(egui::Margin::symmetric(20, 14))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(self.tr(TextKey::Connect))
                                        .size(16.0)
                                        .strong()
                                        .color(Color32::from_rgb(40, 50, 65)),
                                );
                                ui.add_space(12.0);
                                let hint = format!("{} (000 000 000)", self.tr(TextKey::HostId));
                                ui.add(
                                    TextEdit::singleline(&mut self.quick_connect_id)
                                        .hint_text(
                                            RichText::new(hint)
                                                .size(14.0)
                                                .color(Color32::from_rgb(170, 178, 190)),
                                        )
                                        .desired_width(200.0)
                                        .font(egui::TextStyle::Monospace.resolve(ui.style())),
                                );
                                ui.add_space(8.0);

                                let btn = ui.add_sized(
                                    [120.0, 34.0],
                                    Button::new(
                                        RichText::new(self.tr(TextKey::Connect))
                                            .size(14.0)
                                            .color(Color32::WHITE),
                                    )
                                    .fill(Color32::from_rgb(0, 120, 212)),
                                );

                                if btn.clicked() {
                                    let id = self.quick_connect_id.trim().to_owned();
                                    if !id.is_empty() {
                                        if let Some(idx) = self.hosts.iter().position(|h| {
                                            h.id.replace(' ', "") == id.replace(' ', "")
                                        }) {
                                            self.selected_host = idx;
                                            self.request_session(false);
                                        } else {
                                            self.status_line =
                                                format!("Хост с ID {} не найден.", id);
                                        }
                                    }
                                }
                            });
                        });

                    // Server bar
                    Frame::new()
                        .fill(Color32::from_rgb(248, 249, 252))
                        .inner_margin(egui::Margin::symmetric(20, 8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(self.tr(TextKey::Server))
                                        .size(12.0)
                                        .color(Color32::from_rgb(120, 130, 145)),
                                );
                                ui.add(
                                    TextEdit::singleline(&mut self.server_url)
                                        .desired_width(250.0)
                                        .font(egui::TextStyle::Monospace.resolve(ui.style())),
                                );
                                ui.add_space(8.0);
                                if ui
                                    .add_sized(
                                        [80.0, 28.0],
                                        Button::new(
                                            RichText::new(if self.signed_in() {
                                                self.tr(TextKey::Reconnect)
                                            } else {
                                                self.tr(TextKey::SignIn)
                                            })
                                            .size(13.0),
                                        ),
                                    )
                                    .clicked()
                                {
                                    self.sign_in();
                                }
                                if ui
                                    .add_sized(
                                        [72.0, 28.0],
                                        Button::new(
                                            RichText::new(self.tr(TextKey::Refresh))
                                                .size(13.0),
                                        ),
                                    )
                                    .clicked()
                                {
                                    self.refresh_devices();
                                }
                                if ui
                                    .add_sized(
                                        [68.0, 28.0],
                                        Button::new(
                                            RichText::new(self.tr(TextKey::Demo)).size(13.0),
                                        ),
                                    )
                                    .clicked()
                                {
                                    self.switch_to_demo_mode();
                                }
                            });
                        });

                    ui.add_space(8.0);

                    // Devices list
                    self.devices_list(ui);
                });
            });
    }

    fn devices_list(&mut self, ui: &mut Ui) {
        let filtered = self.filtered_host_indices();

        if filtered.is_empty() {
            let w = ui.available_width();
            let _ = ui.allocate_ui_with_layout(
                Vec2::new(w, 200.0),
                Layout::centered_and_justified(egui::Direction::TopDown),
                |ui| {
                    ui.label(
                        RichText::new(self.tr(TextKey::NoHosts))
                            .size(16.0)
                            .color(Color32::from_rgb(150, 158, 170)),
                    );
                },
            );
            return;
        }

        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for host_index in &filtered {
                    let host = &self.hosts[*host_index];
                    let selected = self.selected_host == *host_index;

                    let bg = if selected {
                        Color32::from_rgb(230, 240, 255)
                    } else {
                        Color32::WHITE
                    };

                    let response = Frame::new()
                        .fill(bg)
                        .inner_margin(egui::Margin::symmetric(20, 10))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                // Status dot
                                let (dot_r, _) =
                                    ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                                ui.painter().circle_filled(
                                    dot_r.center(),
                                    6.0,
                                    host.state.tint(),
                                );
                                ui.add_space(10.0);

                                // Name and info
                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(&host.name)
                                                .size(16.0)
                                                .strong()
                                                .color(Color32::from_rgb(35, 45, 60)),
                                        );
                                        ui.add_space(8.0);
                                        status_chip(
                                            ui,
                                            host.state.label(),
                                            host.state.tint(),
                                            host.state.fill(),
                                        );
                                    });
                                    ui.add_space(2.0);
                                    ui.label(
                                        RichText::new(format!(
                                            "{} · {} · {}",
                                            host.id, host.endpoint, host.os
                                        ))
                                        .size(13.0)
                                        .color(Color32::from_rgb(120, 130, 148)),
                                    );
                                });

                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(
                                        RichText::new(&host.last_seen)
                                            .size(12.0)
                                            .color(Color32::from_rgb(150, 158, 170)),
                                    );
                                });
                            });
                        })
                        .response
                        .interact(Sense::click());

                    if response.hovered() && !selected {
                        ui.painter().rect_filled(
                            response.rect,
                            CornerRadius::same(2),
                            Color32::from_rgb(240, 245, 252),
                        );
                    }

                    if response.clicked() {
                        self.selected_host = *host_index;
                        self.status_line = format!(
                            "{} {} ({})",
                            self.tr(TextKey::SelectedHost),
                            host.name,
                            host.id
                        );
                    }

                    if response.double_clicked() {
                        self.request_session(false);
                    }

                    ui.separator();
                }
            });
    }

    fn quick_connect_bar(&mut self, ui: &mut Ui) {
        Frame::new()
            .fill(Color32::from_rgb(242, 245, 250))
            .stroke(Stroke::new(1.0, Color32::from_rgb(210, 218, 228)))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("ID")
                            .size(15.0)
                            .strong()
                            .color(Color32::from_rgb(50, 62, 80)),
                    );
                    ui.add_space(8.0);
                    ui.add(
                        TextEdit::singleline(&mut self.quick_connect_id)
                            .hint_text(
                                RichText::new("000 000 000")
                                    .size(14.0)
                                    .color(Color32::from_rgb(160, 168, 180)),
                            )
                            .desired_width(180.0)
                            .font(egui::TextStyle::Monospace.resolve(ui.style())),
                    );
                    ui.add_space(8.0);

                    let connect_btn = ui.add_sized(
                        [130.0, 32.0],
                        Button::new(
                            RichText::new(self.tr(TextKey::Connect))
                                .size(15.0)
                                .color(Color32::WHITE),
                        )
                        .fill(Color32::from_rgb(50, 100, 180)),
                    );

                    if connect_btn.clicked() {
                        let id = self.quick_connect_id.trim().to_owned();
                        if !id.is_empty() {
                            if let Some(host_index) = self
                                .hosts
                                .iter()
                                .position(|h| h.id.replace(' ', "") == id.replace(' ', ""))
                            {
                                self.selected_host = host_index;
                                self.request_session(false);
                            } else {
                                self.status_line =
                                    format!("Хост с ID {} не найден.", id);
                            }
                        }
                    }

                    ui.add_space(8.0);

                    let view_btn = ui.add_sized(
                        [110.0, 32.0],
                        Button::new(
                            RichText::new(self.tr(TextKey::View))
                                .size(15.0)
                                .color(Color32::from_rgb(60, 72, 90)),
                        )
                        .fill(Color32::from_rgb(230, 235, 242))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(200, 208, 218))),
                    );

                    if view_btn.clicked() {
                        let id = self.quick_connect_id.trim().to_owned();
                        if !id.is_empty() {
                            if let Some(host_index) = self
                                .hosts
                                .iter()
                                .position(|h| h.id.replace(' ', "") == id.replace(' ', ""))
                            {
                                self.selected_host = host_index;
                                self.request_session(true);
                            } else {
                                self.status_line =
                                    format!("Хост с ID {} не найден.", id);
                            }
                        }
                    }
                });
            });
    }

    fn summary_strip(&self, ui: &mut Ui) {
        Frame::new()
            .fill(Color32::from_rgb(235, 240, 245))
            .stroke(Stroke::new(1.0, Color32::from_rgb(192, 200, 208)))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    status_chip(
                        ui,
                        if self.using_demo_data {
                            self.tr(TextKey::DemoAddressBook)
                        } else {
                            self.tr(TextKey::LiveAddressBook)
                        },
                        Color32::from_rgb(47, 92, 141),
                        Color32::from_rgb(223, 232, 243),
                    );
                    ui.label(&self.connection_note);
                    if let Some(session) = &self.last_session {
                        ui.separator();
                        ui.label(format!(
                            "{}: {} [{}] ({})",
                            self.tr(TextKey::LastSession),
                            session.session_id,
                            session.state,
                            format_ms(session.expires_at_ms)
                        ));
                    }
                });
            });
    }

    fn host_table(&mut self, ui: &mut Ui) {
        Frame::new()
            .fill(Color32::WHITE)
            .stroke(Stroke::new(1.0, Color32::from_rgb(210, 218, 228)))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin::same(0))
            .show(ui, |ui| {
                let filtered = self.filtered_host_indices();

                // Table header
                Frame::new()
                    .fill(Color32::from_rgb(240, 243, 248))
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            column_text(
                                ui,
                                "",
                                18.0,
                                true,
                                Color32::from_rgb(120, 130, 148),
                            );
                            column_text(
                                ui,
                                self.tr(TextKey::Computer),
                                200.0,
                                true,
                                Color32::from_rgb(50, 62, 80),
                            );
                            column_text(
                                ui,
                                "ID",
                                120.0,
                                true,
                                Color32::from_rgb(50, 62, 80),
                            );
                            column_text(
                                ui,
                                self.tr(TextKey::Endpoint),
                                180.0,
                                true,
                                Color32::from_rgb(50, 62, 80),
                            );
                            column_text(
                                ui,
                                self.tr(TextKey::Platform),
                                150.0,
                                true,
                                Color32::from_rgb(50, 62, 80),
                            );
                            column_text(
                                ui,
                                self.tr(TextKey::ActivityShort),
                                110.0,
                                true,
                                Color32::from_rgb(50, 62, 80),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(self.tr(TextKey::State))
                                        .size(13.0)
                                        .strong()
                                        .color(Color32::from_rgb(50, 62, 80)),
                                );
                            });
                        });
                    });

                ui.separator();

                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if filtered.is_empty() {
                            empty_state(ui, self.tr(TextKey::NoHosts));
                            return;
                        }

                        for host_index in &filtered {
                            let host = &self.hosts[*host_index];
                            let selected = self.selected_host == *host_index;

                            let row_bg = if selected {
                                Color32::from_rgb(210, 225, 248)
                            } else {
                                Color32::TRANSPARENT
                            };

                            let hover_bg = Color32::from_rgb(235, 240, 250);

                            let response = Frame::new()
                                .fill(row_bg)
                                .inner_margin(egui::Margin::symmetric(12, 7))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        state_dot(ui, host.state);
                                        column_text(
                                            ui,
                                            &host.name,
                                            200.0,
                                            true,
                                            if selected {
                                                Color32::from_rgb(30, 50, 90)
                                            } else {
                                                Color32::from_rgb(40, 52, 70)
                                            },
                                        );
                                        column_text(
                                            ui,
                                            &host.id,
                                            120.0,
                                            false,
                                            Color32::from_rgb(100, 110, 128),
                                        );
                                        column_text(
                                            ui,
                                            &host.endpoint,
                                            180.0,
                                            false,
                                            Color32::from_rgb(100, 110, 128),
                                        );
                                        column_text(
                                            ui,
                                            &host.os,
                                            150.0,
                                            false,
                                            Color32::from_rgb(100, 110, 128),
                                        );
                                        column_text(
                                            ui,
                                            &host.last_seen,
                                            110.0,
                                            false,
                                            Color32::from_rgb(100, 110, 128),
                                        );
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                status_chip(
                                                    ui,
                                                    host.state.label(),
                                                    host.state.tint(),
                                                    host.state.fill(),
                                                );
                                            },
                                        );
                                    });
                                })
                                .response
                                .interact(Sense::click());

                            if response.hovered() && !selected {
                                ui.painter().rect_filled(
                                    response.rect,
                                    CornerRadius::same(2),
                                    hover_bg,
                                );
                            }

                            if response.clicked() {
                                self.selected_host = *host_index;
                                self.status_line = format!(
                                    "{} {} ({})",
                                    self.tr(TextKey::SelectedHost),
                                    host.name,
                                    host.id
                                );
                            }

                            if response.double_clicked() {
                                self.request_session(false);
                            }
                        }
                    });
            });
    }

    fn host_details(&mut self, ctx: &Context) {
        SidePanel::right("details_panel")
            .resizable(true)
            .default_width(300.0)
            .width_range(260.0..=380.0)
            .frame(
                Frame::new()
                    .fill(Color32::WHITE)
                    .inner_margin(egui::Margin::symmetric(16, 14)),
            )
            .show_animated(ctx, self.selected_host().is_some(), |ui| {
                if let Some(host) = self.selected_host().cloned() {
                    // Header
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(&host.name)
                                .size(20.0)
                                .strong()
                                .color(Color32::from_rgb(35, 45, 60)),
                        );
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        status_chip(
                            ui,
                            host.state.label(),
                            host.state.tint(),
                            host.state.fill(),
                        );
                    });
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(&host.note)
                            .size(13.0)
                            .color(Color32::from_rgb(120, 130, 148)),
                    );
                    ui.add_space(14.0);

                    // Details
                    ui.label(
                        RichText::new("Информация")
                            .size(13.0)
                            .strong()
                            .color(Color32::from_rgb(60, 72, 90)),
                    );
                    ui.add_space(6.0);

                    detail_row(ui, self.tr(TextKey::HostId), &host.id);
                    detail_row(ui, self.tr(TextKey::ConnectCode), &host.connect_code);
                    detail_row(ui, self.tr(TextKey::Group), &host.group);
                    detail_row(ui, self.tr(TextKey::Endpoint), &host.endpoint);
                    detail_row(ui, self.tr(TextKey::Platform), &host.os);
                    detail_row(ui, self.tr(TextKey::Owner), &host.owner);
                    detail_row(ui, self.tr(TextKey::LastSeen), &host.last_seen);

                    ui.add_space(14.0);

                    // Actions
                    ui.label(
                        RichText::new("Действия")
                            .size(13.0)
                            .strong()
                            .color(Color32::from_rgb(60, 72, 90)),
                    );
                    ui.add_space(6.0);

                    let w = ui.available_width();

                    let btn = ui.add_sized(
                        [w, 36.0],
                        Button::new(
                            RichText::new(self.tr(TextKey::Connect))
                                .size(15.0)
                                .color(Color32::WHITE),
                        )
                        .fill(Color32::from_rgb(0, 120, 212)),
                    );
                    if btn.clicked() {
                        self.request_session(false);
                    }

                    let btn = ui.add_sized(
                        [w, 36.0],
                        Button::new(
                            RichText::new(self.tr(TextKey::View))
                                .size(15.0)
                                .color(Color32::from_rgb(60, 72, 90)),
                        )
                        .fill(Color32::from_rgb(240, 242, 248))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(200, 208, 218))),
                    );
                    if btn.clicked() {
                        self.request_session(true);
                    }

                    let btn = ui.add_sized(
                        [w, 36.0],
                        Button::new(
                            RichText::new("Завершить")
                                .size(15.0)
                                .color(Color32::from_rgb(200, 60, 50)),
                        )
                        .fill(Color32::from_rgb(255, 240, 238))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(230, 190, 185))),
                    );
                    if btn.clicked() {
                        self.close_session();
                    }

                    let btn = ui.add_sized(
                        [w, 36.0],
                        Button::new(
                            RichText::new(self.tr(TextKey::Files))
                                .size(15.0)
                                .color(Color32::from_rgb(60, 72, 90)),
                        )
                        .fill(Color32::from_rgb(245, 246, 250))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(215, 222, 232))),
                    );
                    if btn.clicked() {
                        self.status_line = format!(
                            "Открыта рабочая область передачи файлов для {}.",
                            host.name
                        );
                        self.add_activity(format!(
                            "Открыт раздел передачи файлов для {}",
                            host.name
                        ));
                    }

                    ui.add_space(14.0);

                    // Permissions
                    ui.label(
                        RichText::new(self.tr(TextKey::Permissions))
                            .size(13.0)
                            .strong()
                            .color(Color32::from_rgb(60, 72, 90)),
                    );
                    ui.add_space(6.0);
                    permission_row(ui, self.tr(TextKey::Screen), host.permissions.screen_capture);
                    permission_row(ui, self.tr(TextKey::Input), host.permissions.input_control);
                    permission_row(ui, self.tr(TextKey::Access), host.permissions.accessibility);
                    permission_row(ui, self.tr(TextKey::Files), host.permissions.file_transfer);
                }
            });
    }
}

impl App for ConsoleApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        let repaint_delay = if self.media_connected_session_id.is_some()
            && now_ms().saturating_sub(self.media_last_frame_at_ms) < 1_000
        {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(250)
        };
        ctx.request_repaint_after(repaint_delay);
        self.maybe_auto_sign_in();
        self.ensure_signal_listener();
        self.process_signal_events();
        self.ensure_media_listener();
        self.process_media_events(ctx);
        self.ensure_selection_visible();
        self.top_bar(ctx);
        self.center_panel(ctx);
        self.host_details(ctx);
        self.session_window(ctx);
        self.footer(ctx);
    }
}

impl From<DeviceSummary> for HostRecord {
    fn from(value: DeviceSummary) -> Self {
        let state = if value.online {
            HostState::Online
        } else {
            HostState::Offline
        };
        let endpoint = format!(
            "{} / {}",
            value.host_info.hostname, value.host_info.username
        );
        let os = format!(
            "{} {} ({})",
            value.host_info.os, value.host_info.os_version, value.host_info.arch
        );
        let group = value.group_name.unwrap_or_else(|| {
            if value.online {
                "В сети".to_owned()
            } else {
                "Не в сети".to_owned()
            }
        });
        let note = format!(
            "{}. {}{}{}Код подключения обновляется автоматически; текущий слот истекает {}.",
            if value.permissions.input_control {
                "Устройство готово к удалённому управлению"
            } else {
                "Устройство с ограниченным управлением"
            },
            value.department
                .as_deref()
                .filter(|value| !value.is_empty())
                .map(|value| format!("Отдел: {value}. "))
                .unwrap_or_default(),
            value.location
                .as_deref()
                .filter(|value| !value.is_empty())
                .map(|value| format!("Локация: {value}. "))
                .unwrap_or_default(),
            if value.host_info.ram_total_mb > 0 || !value.host_info.cpu.is_empty() {
                format!(
                    "CPU: {}. RAM: {} MB. ",
                    if value.host_info.cpu.is_empty() {
                        "unknown"
                    } else {
                        value.host_info.cpu.as_str()
                    },
                    value.host_info.ram_total_mb
                )
            } else {
                String::new()
            },
            format_ms(value.connect_code_expires_at_ms)
        );

        Self {
            id: value.device_id,
            name: value.device_name,
            group,
            endpoint,
            connect_code: value.connect_code,
            os,
            owner: value.host_info.username,
            note,
            last_seen: format_last_seen(value.last_seen_ms),
            state,
            permissions: value.permissions,
        }
    }
}

fn app_brand_name() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "BK-Console macOS"
    }

    #[cfg(not(target_os = "macos"))]
    {
        "BK-Console"
    }
}

fn apply_console_theme(ctx: &Context) {
    let mut style = (*ctx.style()).clone();
    let mut visuals = egui::Visuals::light();

    // Background
    visuals.window_fill = Color32::from_rgb(248, 250, 254);
    visuals.panel_fill = Color32::from_rgb(240, 243, 248);
    visuals.extreme_bg_color = Color32::from_rgb(252, 253, 255);
    visuals.faint_bg_color = Color32::from_rgb(235, 239, 245);

    // Widgets
    visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(240, 243, 248);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(200, 208, 218));
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(60, 70, 85));

    visuals.widgets.inactive.bg_fill = Color32::from_rgb(242, 245, 250);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(205, 212, 222));

    visuals.widgets.hovered.bg_fill = Color32::from_rgb(220, 232, 248);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(100, 130, 190));

    visuals.widgets.active.bg_fill = Color32::from_rgb(200, 218, 242);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, Color32::from_rgb(70, 100, 165));

    visuals.widgets.open.bg_fill = Color32::from_rgb(225, 235, 250);
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, Color32::from_rgb(90, 120, 180));

    // Selection
    visuals.selection.bg_fill = Color32::from_rgb(200, 218, 242);
    visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(80, 120, 190));

    // Window
    visuals.window_corner_radius = CornerRadius::same(8);
    visuals.window_stroke = Stroke::new(1.0, Color32::from_rgb(210, 218, 228));
    visuals.menu_corner_radius = CornerRadius::same(8);

    // Popup
    visuals.popup_shadow = egui::epaint::Shadow::NONE;

    style.visuals = visuals;
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.spacing.window_margin = egui::Margin::same(10);
    ctx.set_style(style);
}

fn menu_button(ui: &mut Ui, label: &str) -> egui::Response {
    ui.add(
        Button::new(RichText::new(label).size(14.0))
            .fill(Color32::from_rgb(235, 240, 248))
            .stroke(Stroke::new(1.0, Color32::from_rgb(205, 212, 222))),
    )
}

fn panel_title(ui: &mut Ui, title: &str) {
    ui.label(
        RichText::new(title)
            .strong()
            .size(13.0)
            .color(Color32::from_rgb(50, 62, 80)),
    );
    ui.add_space(6.0);
}

fn status_chip(ui: &mut Ui, text: &str, tint: Color32, fill: Color32) {
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, tint))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(text).strong().size(13.0).color(tint));
        });
}

fn status_chip_dark(ui: &mut Ui, text: &str, tint: Color32, fill: Color32) {
    status_chip(ui, text, tint, fill);
}

fn permission_chip(ui: &mut Ui, text: &str, enabled: bool) {
    let tint = if enabled {
        Color32::from_rgb(40, 120, 65)
    } else {
        Color32::from_rgb(150, 110, 50)
    };
    let fill = if enabled {
        Color32::from_rgb(220, 240, 225)
    } else {
        Color32::from_rgb(248, 240, 225)
    };
    let label = if enabled {
        text.to_owned()
    } else {
        format!("{text} выкл")
    };
    status_chip(ui, &label, tint, fill);
}

fn permission_chip_dark(ui: &mut Ui, text: &str, enabled: bool) {
    permission_chip(ui, text, enabled);
}

fn inspector_card(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(Color32::WHITE)
        .stroke(Stroke::new(1.0, Color32::from_rgb(210, 218, 228)))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::same(12))
        .show(ui, add_contents);
}

fn labeled_edit(ui: &mut Ui, label: &str, value: &mut String, password: bool) {
    ui.label(
        RichText::new(label)
            .strong()
            .size(14.0)
            .color(Color32::from_rgb(60, 72, 90)),
    );
    let mut edit = TextEdit::singleline(value).desired_width(f32::INFINITY);
    if password {
        edit = edit.password(true);
    }
    ui.add(edit);
}

fn table_header(ui: &mut Ui, language: AppLanguage) {
    ui.horizontal(|ui| {
        ui.add_space(22.0);
        column_text(ui, language.text(TextKey::Computer), 160.0, true, Color32::from_rgb(50, 62, 80));
        column_text(ui, "ID", 92.0, true, Color32::from_rgb(50, 62, 80));
        column_text(ui, language.text(TextKey::Endpoint), 180.0, true, Color32::from_rgb(50, 62, 80));
        column_text(ui, language.text(TextKey::ActivityShort), 110.0, true, Color32::from_rgb(50, 62, 80));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(language.text(TextKey::State))
                    .size(13.0)
                    .strong()
                    .color(Color32::from_rgb(50, 62, 80)),
            );
        });
    });
}

fn column_text(ui: &mut Ui, text: &str, width: f32, strong: bool, color: Color32) {
    let rich = if strong {
        RichText::new(text).strong().size(14.0).color(color)
    } else {
        RichText::new(text).size(14.0).color(color)
    };
    ui.add_sized([width, 20.0], egui::Label::new(rich));
}

fn state_dot(ui: &mut Ui, state: HostState) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 5.0, state.tint());
}

fn detail_row(ui: &mut Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .size(13.0)
                .color(Color32::from_rgb(130, 140, 155)),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(value)
                    .size(13.0)
                    .color(Color32::from_rgb(50, 60, 78)),
            );
        });
    });
    ui.add_space(4.0);
}

fn detail_row_dark(ui: &mut Ui, label: &str, value: &str) {
    detail_row(ui, label, value);
}

fn permission_row(ui: &mut Ui, label: &str, enabled: bool) {
    ui.horizontal(|ui| {
        let (icon, color) = if enabled {
            ("●", Color32::from_rgb(40, 150, 70))
        } else {
            ("○", Color32::from_rgb(180, 130, 60))
        };
        ui.label(RichText::new(icon).size(14.0).color(color));
        ui.label(
            RichText::new(label)
                .size(13.0)
                .color(Color32::from_rgb(60, 70, 88)),
        );
    });
    ui.add_space(3.0);
}

fn action_button(ui: &mut Ui, label: &str) -> egui::Response {
    ui.add(
        Button::new(RichText::new(label).size(14.0))
            .min_size(Vec2::new(100.0, 30.0))
            .fill(Color32::from_rgb(235, 240, 248))
            .stroke(Stroke::new(1.0, Color32::from_rgb(205, 212, 222))),
    )
}

fn action_button_widget(label: &str) -> impl egui::Widget {
    Button::new(RichText::new(label).size(14.0))
        .min_size(Vec2::new(104.0, 32.0))
        .fill(Color32::from_rgb(235, 240, 248))
}

fn subpanel(ui: &mut Ui, title: &str, add_contents: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(Color32::from_rgb(248, 250, 254))
        .stroke(Stroke::new(1.0, Color32::from_rgb(215, 222, 232)))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.label(
                RichText::new(title)
                    .strong()
                    .size(14.0)
                    .color(Color32::from_rgb(60, 72, 90)),
            );
            ui.add_space(8.0);
            add_contents(ui);
        });
}

fn empty_state(ui: &mut Ui, text: &str) {
    let width = ui.available_width().max(200.0);
    let _ = ui.allocate_ui_with_layout(
        Vec2::new(width, 120.0),
        Layout::centered_and_justified(egui::Direction::TopDown),
        |ui| {
            Frame::new()
                .fill(Color32::from_rgb(245, 248, 252))
                .stroke(Stroke::new(1.0, Color32::from_rgb(215, 222, 232)))
                .corner_radius(CornerRadius::same(6))
                .show(ui, |ui| {
                    ui.add_space(20.0);
                    ui.label(
                        RichText::new(text)
                            .size(15.0)
                            .color(Color32::from_rgb(120, 130, 148)),
                    );
                    ui.add_space(20.0);
                });
        },
    );
}

fn map_egui_key(key: egui::Key) -> Option<&'static str> {
    match key {
        egui::Key::Enter => Some("enter"),
        egui::Key::Tab => Some("tab"),
        egui::Key::Backspace => Some("backspace"),
        egui::Key::Escape => Some("escape"),
        egui::Key::Space => Some("space"),
        egui::Key::Insert => Some("insert"),
        egui::Key::ArrowUp => Some("arrow_up"),
        egui::Key::ArrowDown => Some("arrow_down"),
        egui::Key::ArrowLeft => Some("arrow_left"),
        egui::Key::ArrowRight => Some("arrow_right"),
        egui::Key::Delete => Some("delete"),
        egui::Key::Home => Some("home"),
        egui::Key::End => Some("end"),
        egui::Key::PageUp => Some("page_up"),
        egui::Key::PageDown => Some("page_down"),
        egui::Key::Colon => Some("colon"),
        egui::Key::Comma => Some("comma"),
        egui::Key::Backslash => Some("backslash"),
        egui::Key::Slash => Some("slash"),
        egui::Key::Pipe => Some("pipe"),
        egui::Key::Questionmark => Some("questionmark"),
        egui::Key::Exclamationmark => Some("exclamationmark"),
        egui::Key::OpenBracket => Some("open_bracket"),
        egui::Key::CloseBracket => Some("close_bracket"),
        egui::Key::OpenCurlyBracket => Some("open_curly_bracket"),
        egui::Key::CloseCurlyBracket => Some("close_curly_bracket"),
        egui::Key::Backtick => Some("backtick"),
        egui::Key::Minus => Some("minus"),
        egui::Key::Period => Some("period"),
        egui::Key::Plus => Some("plus"),
        egui::Key::Equals => Some("equals"),
        egui::Key::Semicolon => Some("semicolon"),
        egui::Key::Quote => Some("quote"),
        egui::Key::A => Some("a"),
        egui::Key::B => Some("b"),
        egui::Key::C => Some("c"),
        egui::Key::D => Some("d"),
        egui::Key::E => Some("e"),
        egui::Key::F => Some("f"),
        egui::Key::G => Some("g"),
        egui::Key::H => Some("h"),
        egui::Key::I => Some("i"),
        egui::Key::J => Some("j"),
        egui::Key::K => Some("k"),
        egui::Key::L => Some("l"),
        egui::Key::M => Some("m"),
        egui::Key::N => Some("n"),
        egui::Key::O => Some("o"),
        egui::Key::P => Some("p"),
        egui::Key::Q => Some("q"),
        egui::Key::R => Some("r"),
        egui::Key::S => Some("s"),
        egui::Key::T => Some("t"),
        egui::Key::U => Some("u"),
        egui::Key::V => Some("v"),
        egui::Key::W => Some("w"),
        egui::Key::X => Some("x"),
        egui::Key::Y => Some("y"),
        egui::Key::Z => Some("z"),
        egui::Key::Num0 => Some("0"),
        egui::Key::Num1 => Some("1"),
        egui::Key::Num2 => Some("2"),
        egui::Key::Num3 => Some("3"),
        egui::Key::Num4 => Some("4"),
        egui::Key::Num5 => Some("5"),
        egui::Key::Num6 => Some("6"),
        egui::Key::Num7 => Some("7"),
        egui::Key::Num8 => Some("8"),
        egui::Key::Num9 => Some("9"),
        egui::Key::F1 => Some("f1"),
        egui::Key::F2 => Some("f2"),
        egui::Key::F3 => Some("f3"),
        egui::Key::F4 => Some("f4"),
        egui::Key::F5 => Some("f5"),
        egui::Key::F6 => Some("f6"),
        egui::Key::F7 => Some("f7"),
        egui::Key::F8 => Some("f8"),
        egui::Key::F9 => Some("f9"),
        egui::Key::F10 => Some("f10"),
        egui::Key::F11 => Some("f11"),
        egui::Key::F12 => Some("f12"),
        egui::Key::F13 => Some("f13"),
        egui::Key::F14 => Some("f14"),
        egui::Key::F15 => Some("f15"),
        egui::Key::F16 => Some("f16"),
        egui::Key::F17 => Some("f17"),
        egui::Key::F18 => Some("f18"),
        egui::Key::F19 => Some("f19"),
        egui::Key::F20 => Some("f20"),
        egui::Key::F21 => Some("f21"),
        egui::Key::F22 => Some("f22"),
        egui::Key::F23 => Some("f23"),
        egui::Key::F24 => Some("f24"),
        egui::Key::BrowserBack => Some("browser_back"),
        _ => None,
    }
}

fn map_egui_modifiers(modifiers: egui::Modifiers) -> Vec<&'static str> {
    let mut mapped = Vec::new();

    if modifiers.command || modifiers.ctrl {
        mapped.push("ctrl");
    }
    if modifiers.alt {
        mapped.push("alt");
    }
    if modifiers.shift {
        mapped.push("shift");
    }

    mapped
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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn format_ms(value: u64) -> String {
    if value == 0 {
        return "никогда".to_owned();
    }

    let now = now_ms();
    if value <= now {
        return "истёк".to_owned();
    }

    let remaining = (value - now) / 1000;
    if remaining < 60 {
        format!("через {remaining}с")
    } else if remaining < 3600 {
        format!("через {}м", remaining / 60)
    } else {
        format!("через {}ч", remaining / 3600)
    }
}

fn format_last_seen(value: u64) -> String {
    if value == 0 {
        return "никогда".to_owned();
    }

    let now = now_ms();
    let elapsed = now.saturating_sub(value) / 1000;
    if elapsed < 15 {
        "сейчас".to_owned()
    } else if elapsed < 60 {
        format!("{elapsed}с назад")
    } else if elapsed < 3600 {
        format!("{} мин назад", elapsed / 60)
    } else if elapsed < 86_400 {
        format!("{} ч назад", elapsed / 3600)
    } else {
        format!("{} д назад", elapsed / 86_400)
    }
}

fn format_clock(value: u64) -> String {
    let seconds = (value / 1000) % 86_400;
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    format!("{hours:02}:{minutes:02}")
}

fn format_frame_age(value: u64) -> String {
    if value == 0 {
        return "нет".to_owned();
    }

    let elapsed_ms = now_ms().saturating_sub(value);
    if elapsed_ms < 1_000 {
        format!("{}мс назад", elapsed_ms)
    } else {
        format!("{:.1}с назад", elapsed_ms as f32 / 1000.0)
    }
}

fn shorten_commit(commit: &str) -> String {
    if commit.len() <= 8 {
        commit.to_owned()
    } else {
        commit[..8].to_owned()
    }
}

fn build_nav_groups(hosts: &[HostRecord], language: AppLanguage) -> Vec<NavGroup> {
    let mut by_group: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let online = hosts
        .iter()
        .filter(|host| host.state != HostState::Offline)
        .count();

    for host in hosts {
        let entry = by_group.entry(host.group.clone()).or_insert((0, 0));
        if host.state != HostState::Offline {
            entry.0 += 1;
        }
        entry.1 += 1;
    }

    let mut groups = vec![NavGroup {
        title: language.text(TextKey::AllComputers).to_owned(),
        online,
        total: hosts.len(),
    }];

    for (title, (group_online, total)) in by_group {
        groups.push(NavGroup {
            title,
            online: group_online,
            total,
        });
    }

    groups
}

fn demo_hosts() -> Vec<HostRecord> {
    vec![
        HostRecord {
            id: "100 245 771".to_owned(),
            name: "SUP-WS-01".to_owned(),
            group: "Поддержка".to_owned(),
            endpoint: "10.10.1.24".to_owned(),
            connect_code: "764 992".to_owned(),
            os: "Windows 11 Pro".to_owned(),
            owner: "Служба поддержки".to_owned(),
            note: "Основная рабочая станция поддержки с профилем на два монитора.".to_owned(),
            last_seen: "сейчас".to_owned(),
            state: HostState::Online,
            permissions: PermissionStatus {
                screen_capture: true,
                input_control: true,
                accessibility: true,
                file_transfer: true,
            },
        },
        HostRecord {
            id: "100 245 772".to_owned(),
            name: "SUP-LT-04".to_owned(),
            group: "Поддержка".to_owned(),
            endpoint: "10.10.1.38".to_owned(),
            connect_code: "108 553".to_owned(),
            os: "Windows 11 Pro".to_owned(),
            owner: "Выездная поддержка".to_owned(),
            note: "Ноутбук мобильного инженера. Часто используется для сеансов только просмотра."
                .to_owned(),
            last_seen: "1 мин назад".to_owned(),
            state: HostState::Busy,
            permissions: PermissionStatus {
                screen_capture: true,
                input_control: false,
                accessibility: true,
                file_transfer: false,
            },
        },
        HostRecord {
            id: "100 245 910".to_owned(),
            name: "ACC-WS-03".to_owned(),
            group: "Бухгалтерия".to_owned(),
            endpoint: "10.10.2.14".to_owned(),
            connect_code: "320 664".to_owned(),
            os: "Windows 10 Pro".to_owned(),
            owner: "Бухгалтерия".to_owned(),
            note: "Рабочая станция 1С с ограниченной политикой безнадзорного доступа.".to_owned(),
            last_seen: "3 мин назад".to_owned(),
            state: HostState::Online,
            permissions: PermissionStatus {
                screen_capture: true,
                input_control: true,
                accessibility: false,
                file_transfer: true,
            },
        },
        HostRecord {
            id: "100 246 012".to_owned(),
            name: "ACC-ARCHIVE".to_owned(),
            group: "Бухгалтерия".to_owned(),
            endpoint: "10.10.2.45".to_owned(),
            connect_code: "991 003".to_owned(),
            os: "Windows Server 2019".to_owned(),
            owner: "Финансовый ИТ-отдел".to_owned(),
            note: "Архивный узел. Обычно выключен вне отчётных окон.".to_owned(),
            last_seen: "2 ч назад".to_owned(),
            state: HostState::Offline,
            permissions: PermissionStatus {
                screen_capture: true,
                input_control: false,
                accessibility: false,
                file_transfer: false,
            },
        },
        HostRecord {
            id: "100 246 420".to_owned(),
            name: "WH-TERM-07".to_owned(),
            group: "Склад".to_owned(),
            endpoint: "10.10.3.77".to_owned(),
            connect_code: "447 286".to_owned(),
            os: "Windows 10 IoT".to_owned(),
            owner: "Склад".to_owned(),
            note: "Терминал сканера. Нужны быстрые действия оператора для подключения.".to_owned(),
            last_seen: "сейчас".to_owned(),
            state: HostState::Online,
            permissions: PermissionStatus {
                screen_capture: true,
                input_control: true,
                accessibility: true,
                file_transfer: false,
            },
        },
        HostRecord {
            id: "100 246 700".to_owned(),
            name: "CEO-LAPTOP".to_owned(),
            group: "Руководство".to_owned(),
            endpoint: "10.10.4.12".to_owned(),
            connect_code: "550 871".to_owned(),
            os: "Windows 11 Pro".to_owned(),
            owner: "Приёмная руководства".to_owned(),
            note: "Чувствительная машина. Перед управлением требуется явное подтверждение."
                .to_owned(),
            last_seen: "вчера".to_owned(),
            state: HostState::Offline,
            permissions: PermissionStatus {
                screen_capture: true,
                input_control: false,
                accessibility: true,
                file_transfer: false,
            },
        },
    ]
}
