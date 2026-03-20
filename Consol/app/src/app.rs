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
        self, Align, Button, CentralPanel, Color32, Context, CornerRadius, Frame, Grid, Layout,
        RichText, ScrollArea, Sense, SidePanel, Stroke, TextEdit, TextureHandle, TopBottomPanel,
        Ui, Vec2, ViewportBuilder,
    },
};
use reqwest::blocking::Client;

use crate::{
    api::{self, DeviceSummary, PermissionStatus},
    media::{self, MediaEvent},
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
            Self::Online => Color32::from_rgb(64, 153, 86),
            Self::Busy => Color32::from_rgb(47, 92, 141),
            Self::Offline => Color32::from_rgb(150, 157, 166),
        }
    }

    fn fill(self) -> Color32 {
        match self {
            Self::Online => Color32::from_rgb(224, 241, 228),
            Self::Busy => Color32::from_rgb(223, 232, 243),
            Self::Offline => Color32::from_rgb(232, 236, 240),
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
    session_token: String,
    expires_at_ms: u64,
    state: String,
    target_host_id: String,
    target_host_name: String,
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
    media_rx: Receiver<MediaEvent>,
    media_tx: Sender<MediaEvent>,
    session_texture: Option<TextureHandle>,
    last_auto_sign_in_attempt_at_ms: u64,
    signal_listener_key: Option<String>,
    signal_tx: Sender<SignalEvent>,
    signal_rx: Receiver<SignalEvent>,
}

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
            server_url: "http://172.16.100.164:8080".to_owned(),
            login: DEFAULT_OPERATOR_LOGIN.to_owned(),
            password: DEFAULT_OPERATOR_PASSWORD.to_owned(),
            search_query: String::new(),
            status_line: "Консоль готова к работе. Выполняется автоматическое подключение к серверу BK-Wiver.".to_owned(),
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
            media_rx,
            media_tx,
            session_texture: None,
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
                self.status_line =
                    "Вход выполнен успешно. Загружаю список устройств с сервера.".to_owned();
                self.add_activity("Оператор вошёл в систему");
                self.ensure_signal_listener();
                self.refresh_devices();
            }
            Err(error) => {
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
                    if !self.using_demo_data {
                        self.connection_note = format!(
                            "Подключено к {}. Signal channel активен.",
                            normalize_server_url(&self.server_url)
                        );
                    }
                }
                SignalEvent::Disconnected => {
                    if !self.using_demo_data {
                        self.connection_note = format!(
                            "Подключено к {}. Signal channel переподключается.",
                            normalize_server_url(&self.server_url)
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
                        self.show_session_window = true;
                        self.status_line = format!("Сеанс {session_id} подтверждён хостом.");
                        self.add_activity(format!("Хост подтвердил сеанс {session_id}"));
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
                        self.stop_media_listener();
                        self.media_connected_session_id = None;
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
                        self.stop_media_listener();
                        self.media_connected_session_id = None;
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
        self.session_texture = None;
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
        self.session_texture = None;

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
                session_token: "demo".to_owned(),
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
                self.last_session = Some(SessionPreview {
                    session_id: body.session_id.clone(),
                    session_token: body.session_token,
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
                    if self.media_connected_session_id.as_deref() == Some(session_id.as_str()) {
                        self.media_connected_session_id = None;
                    }
                }
                MediaEvent::Frame { session_id, bytes } => {
                    if self
                        .last_session
                        .as_ref()
                        .map(|session| session.session_id.as_str())
                        != Some(session_id.as_str())
                    {
                        continue;
                    }

                    let Ok(image) = image::load_from_memory(&bytes) else {
                        continue;
                    };
                    let image = image.to_rgba8();
                    let size = [image.width() as usize, image.height() as usize];
                    let pixels = image.into_raw();
                    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);

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
                Color32::from_rgb(47, 120, 62),
                Color32::from_rgb(224, 241, 228),
            ),
            "pending" => (
                Color32::from_rgb(47, 92, 141),
                Color32::from_rgb(223, 232, 243),
            ),
            "demo" => (
                Color32::from_rgb(145, 102, 60),
                Color32::from_rgb(245, 233, 219),
            ),
            "rejected" | "closed" => (
                Color32::from_rgb(126, 82, 58),
                Color32::from_rgb(241, 229, 219),
            ),
            _ => (
                Color32::from_rgb(108, 115, 124),
                Color32::from_rgb(232, 236, 240),
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

    fn remote_view_placeholder(&self, ui: &mut Ui, session: &SessionPreview) {
        let available = ui.available_size_before_wrap();
        let desired_height = available.y.max(260.0);
        let desired_size = Vec2::new(available.x.max(320.0), desired_height);
        let (rect, _) = ui.allocate_exact_size(desired_size, Sense::hover());
        let has_live_frame = self.session_texture.is_some();

        let fill = match session.state.as_str() {
            "connected" => Color32::from_rgb(23, 31, 43),
            "pending" => Color32::from_rgb(38, 49, 63),
            "demo" => Color32::from_rgb(53, 45, 36),
            "closed" | "rejected" => Color32::from_rgb(50, 50, 52),
            _ => Color32::from_rgb(36, 40, 45),
        };

        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(12), fill);
        if let Some(texture) = &self.session_texture {
            painter.image(
                texture.id(),
                rect.shrink2(Vec2::new(8.0, 40.0)),
                egui::Rect::from_min_max(
                    egui::Pos2::new(0.0, 0.0),
                    egui::Pos2::new(1.0, 1.0),
                ),
                Color32::WHITE,
            );
        }
        painter.rect_stroke(
            rect,
            CornerRadius::same(12),
            Stroke::new(1.0, Color32::from_rgb(74, 86, 100)),
            egui::StrokeKind::Inside,
        );

        let header_rect = egui::Rect::from_min_size(rect.min, Vec2::new(rect.width(), 34.0));
        painter.rect_filled(
            header_rect,
            CornerRadius {
                nw: 12,
                ne: 12,
                sw: 0,
                se: 0,
            },
            Color32::from_rgba_unmultiplied(255, 255, 255, 18),
        );
        painter.text(
            header_rect.center(),
            egui::Align2::CENTER_CENTER,
            "BK Remote Session",
            egui::FontId::proportional(16.0),
            Color32::from_rgb(229, 235, 241),
        );

        let (accent, accent_fill) = Self::session_status_colors(&session.state);
        let status_rect = egui::Rect::from_center_size(
            if has_live_frame {
                rect.left_top() + Vec2::new(120.0, 54.0)
            } else {
                rect.center_top() + Vec2::new(0.0, 88.0)
            },
            Vec2::new(220.0, 34.0),
        );
        painter.rect_filled(status_rect, CornerRadius::same(17), accent_fill);
        painter.text(
            status_rect.center(),
            egui::Align2::CENTER_CENTER,
            Self::session_status_label(&session.state),
            egui::FontId::proportional(15.0),
            accent,
        );

        if has_live_frame {
            painter.text(
                rect.left_top() + Vec2::new(26.0, 88.0),
                egui::Align2::LEFT_TOP,
                format!("Хост: {}", session.target_host_name),
                egui::FontId::proportional(18.0),
                Color32::from_rgb(245, 248, 252),
            );
        } else {
            painter.text(
                rect.center_top() + Vec2::new(0.0, 138.0),
                egui::Align2::CENTER_CENTER,
                &session.target_host_name,
                egui::FontId::proportional(26.0),
                Color32::from_rgb(245, 248, 252),
            );

            painter.text(
                rect.center_top() + Vec2::new(0.0, 172.0),
                egui::Align2::CENTER_CENTER,
                "Полотно удалённого экрана готово для медиапотока",
                egui::FontId::proportional(15.0),
                Color32::from_rgb(192, 202, 213),
            );

            painter.text(
                rect.center_top() + Vec2::new(0.0, 204.0),
                egui::Align2::CENTER_CENTER,
                match session.state.as_str() {
                    "connected" => "Ожидаем первые кадры со стороны Host.",
                    "pending" => "Ожидаем подтверждение со стороны Host.",
                    "demo" => "Работаем в локальном демонстрационном режиме.",
                    "closed" => "Сеанс закрыт.",
                    "rejected" => "Подключение отклонено.",
                    _ => "Ожидаем новое состояние сеанса.",
                },
                egui::FontId::proportional(14.0),
                Color32::from_rgb(170, 181, 193),
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
        egui::Window::new("Окно сеанса")
            .id(egui::Id::new("session_window"))
            .open(&mut open)
            .default_size(Vec2::new(980.0, 640.0))
            .min_size(Vec2::new(760.0, 520.0))
            .resizable(true)
            .show(ctx, |ui| {
                ui.columns(2, |columns| {
                    columns[0].vertical(|ui| {
                        self.remote_view_placeholder(ui, &session);
                    });

                    columns[1].vertical(|ui| {
                        ui.heading(
                            RichText::new(&session.target_host_name)
                                .strong()
                                .color(Color32::from_rgb(37, 54, 74)),
                        );
                        ui.label(
                            RichText::new(format!("ID хоста: {}", session.target_host_id))
                                .color(Color32::from_rgb(92, 103, 114)),
                        );
                        ui.add_space(8.0);

                        let (status_tint, status_fill) =
                            Self::session_status_colors(&session.state);
                        status_chip(
                            ui,
                            Self::session_status_label(&session.state),
                            status_tint,
                            status_fill,
                        );

                        ui.add_space(10.0);
                        Frame::new()
                            .fill(Color32::from_rgb(243, 246, 249))
                            .stroke(Stroke::new(1.0, Color32::from_rgb(199, 206, 213)))
                            .corner_radius(CornerRadius::same(6))
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                Grid::new("session_window_grid")
                                    .num_columns(2)
                                    .spacing([12.0, 8.0])
                                    .show(ui, |ui| {
                                        detail_row(ui, "Сеанс", &session.session_id);
                                        detail_row(
                                            ui,
                                            "Истекает",
                                            &format_ms(session.expires_at_ms),
                                        );
                                        detail_row(
                                            ui,
                                            "Канал",
                                            if self.signal_listener_key.is_some() {
                                                "signal подключён"
                                            } else {
                                                "signal переподключается"
                                            },
                                        );
                                        detail_row(
                                            ui,
                                            "Режим",
                                            if session.state == "demo" {
                                                "демонстрационный"
                                            } else {
                                                "удалённый сеанс"
                                            },
                                        );
                                        detail_row(
                                            ui,
                                            "Медиа",
                                            if self.media_connected_session_id.as_deref()
                                                == Some(session.session_id.as_str())
                                            {
                                                "media подключён"
                                            } else {
                                                "ожидаем кадры"
                                            },
                                        );
                                    });
                            });

                        ui.add_space(12.0);
                        subpanel(ui, "Этап сеанса", |ui| {
                            ui.label(
                                RichText::new(Self::session_stage_note(&session.state))
                                    .color(Color32::from_rgb(79, 90, 101)),
                            );
                        });

                        ui.add_space(10.0);
                        subpanel(ui, "Следующие шаги", |ui| {
                            ui.label("1. Подключить тестовый media-канал и вывести первые кадры.");
                            ui.label("2. Передать в это окно координаты мыши и клавиатурные события.");
                            ui.label("3. После этого включить реальный screen capture на Host.");
                        });

                        ui.add_space(14.0);
                        ui.horizontal_wrapped(|ui| {
                            let close_enabled =
                                !matches!(session.state.as_str(), "closed" | "rejected");
                            if ui
                                .add_enabled(close_enabled, action_button_widget("Завершить"))
                                .clicked()
                            {
                                self.close_session();
                            }
                            if ui.button("Скрыть окно").clicked() {
                                self.show_session_window = false;
                            }
                        });
                    });
                });
            });

        self.show_session_window = open;
    }

    fn top_bar(&mut self, ctx: &Context) {
        TopBottomPanel::top("top_bar")
            .frame(
                Frame::new()
                    .fill(Color32::from_rgb(231, 236, 241))
                    .inner_margin(egui::Margin::same(10))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(188, 197, 206))),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(app_brand_name())
                            .size(22.0)
                            .strong()
                            .color(Color32::from_rgb(37, 54, 74)),
                    );
                    ui.add_space(12.0);
                    if menu_button(ui, self.tr(TextKey::Connect)).clicked() {
                        self.request_session(false);
                    }
                    if menu_button(ui, self.tr(TextKey::Refresh)).clicked() {
                        self.refresh_devices();
                    }
                    let _ = menu_button(ui, self.tr(TextKey::Properties));
                    let _ = menu_button(ui, "Wake-on-LAN");
                    let _ = menu_button(ui, self.tr(TextKey::Statistics));
                    let _ = menu_button(ui, self.tr(TextKey::Tools));

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let search_hint = self.tr(TextKey::SearchHosts);
                        let response = ui.add(
                            TextEdit::singleline(&mut self.search_query)
                                .hint_text(search_hint)
                                .desired_width(240.0),
                        );
                        if response.changed() {
                            self.ensure_selection_visible();
                        }

                        let label = if self.using_demo_data {
                            self.tr(TextKey::DemoProfile)
                        } else if self.signed_in() {
                            self.tr(TextKey::SignedIn)
                        } else {
                            self.tr(TextKey::Offline)
                        };

                        let (tint, fill) = if self.using_demo_data {
                            (
                                Color32::from_rgb(145, 102, 60),
                                Color32::from_rgb(245, 233, 219),
                            )
                        } else {
                            (
                                Color32::from_rgb(78, 136, 82),
                                Color32::from_rgb(222, 240, 225),
                            )
                        };
                        status_chip(ui, label, tint, fill);
                        if ui
                            .selectable_label(
                                self.language == AppLanguage::Ru,
                                AppLanguage::Ru.code(),
                            )
                            .clicked()
                        {
                            self.set_language(AppLanguage::Ru);
                        }
                        if ui
                            .selectable_label(
                                self.language == AppLanguage::En,
                                AppLanguage::En.code(),
                            )
                            .clicked()
                        {
                            self.set_language(AppLanguage::En);
                        }
                    });
                });
            });
    }

    fn footer(&self, ctx: &Context) {
        TopBottomPanel::bottom("status_bar")
            .frame(
                Frame::new()
                    .fill(Color32::from_rgb(238, 242, 246))
                    .inner_margin(egui::Margin::same(8))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(190, 197, 205))),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.tr(TextKey::Ready)).strong());
                    ui.separator();
                    ui.label(&self.status_line);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(format!(
                            "{}: {}",
                            self.tr(TextKey::HostsCount),
                            self.filtered_host_indices().len()
                        ));
                    });
                });
            });
    }

    fn left_panel(&mut self, ctx: &Context) {
        SidePanel::left("nav_panel")
            .resizable(true)
            .default_width(250.0)
            .width_range(220.0..=320.0)
            .frame(
                Frame::new()
                    .fill(Color32::from_rgb(244, 247, 250))
                    .inner_margin(egui::Margin::same(10)),
            )
            .show(ctx, |ui| {
                panel_title(ui, self.tr(TextKey::Connection));
                inspector_card(ui, |ui| {
                    labeled_edit(ui, self.tr(TextKey::Server), &mut self.server_url, false);
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_sized(
                                [92.0, 28.0],
                                Button::new(if self.signed_in() {
                                    self.tr(TextKey::Reconnect)
                                } else {
                                    self.tr(TextKey::SignIn)
                                }),
                            )
                            .clicked()
                        {
                            self.sign_in();
                        }
                        if ui
                            .add_sized([80.0, 28.0], Button::new(self.tr(TextKey::Refresh)))
                            .clicked()
                        {
                            self.refresh_devices();
                        }
                        if ui
                            .add_sized([88.0, 28.0], Button::new(self.tr(TextKey::Demo)))
                            .clicked()
                        {
                            self.switch_to_demo_mode();
                        }
                    });
                });

                ui.add_space(10.0);
                panel_title(ui, self.tr(TextKey::AddressBook));
                inspector_card(ui, |ui| {
                    let show_only_online_label = self.tr(TextKey::ShowOnlyOnline);
                    ui.checkbox(&mut self.show_only_online, show_only_online_label);
                    ui.add_space(8.0);
                    ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .max_height(330.0)
                        .show(ui, |ui| {
                            let mut clicked_group = None;
                            for (index, group) in self.nav_groups.iter().enumerate() {
                                let selected = self.active_group == index;
                                let label =
                                    format!("{}   {} / {}", group.title, group.online, group.total);
                                let response = ui.selectable_label(selected, label);
                                if response.clicked() {
                                    clicked_group = Some(index);
                                }
                            }

                            if let Some(index) = clicked_group {
                                self.active_group = index;
                                self.ensure_selection_visible();
                            }
                        });
                });

                ui.add_space(10.0);
                panel_title(ui, self.tr(TextKey::Activity));
                inspector_card(ui, |ui| {
                    ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .max_height(180.0)
                        .show(ui, |ui| {
                            for item in &self.activity_log {
                                ui.label(item);
                            }
                        });
                });
            });
    }

    fn center_panel(&mut self, ctx: &Context) {
        CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(Color32::from_rgb(248, 250, 252))
                    .inner_margin(egui::Margin::same(10)),
            )
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    self.summary_strip(ui);
                    ui.add_space(10.0);

                    ui.columns(2, |columns| {
                        self.host_table(&mut columns[0]);
                        self.host_details(&mut columns[1]);
                    });
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
        panel_title(ui, self.tr(TextKey::Hosts));
        Frame::new()
            .fill(Color32::WHITE)
            .stroke(Stroke::new(1.0, Color32::from_rgb(194, 201, 208)))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                let filtered = self.filtered_host_indices();
                table_header(ui, self.language);
                ui.separator();

                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(560.0)
                    .show(ui, |ui| {
                        if filtered.is_empty() {
                            empty_state(ui, self.tr(TextKey::NoHosts));
                            return;
                        }

                        for host_index in filtered {
                            let host = &self.hosts[host_index];
                            let selected = self.selected_host == host_index;
                            let row_fill = if selected {
                                Color32::from_rgb(217, 229, 243)
                            } else {
                                Color32::WHITE
                            };

                            let response = Frame::new()
                                .fill(row_fill)
                                .stroke(Stroke::new(1.0, Color32::from_rgb(227, 232, 236)))
                                .corner_radius(CornerRadius::same(4))
                                .inner_margin(egui::Margin::symmetric(8, 6))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        state_dot(ui, host.state);
                                        column_text(ui, &host.name, 160.0, true);
                                        column_text(ui, &host.id, 92.0, false);
                                        column_text(ui, &host.endpoint, 180.0, false);
                                        column_text(ui, &host.last_seen, 110.0, false);
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

                            if response.clicked() {
                                self.selected_host = host_index;
                                self.status_line = format!(
                                    "{} {} ({})",
                                    self.tr(TextKey::SelectedHost),
                                    host.name,
                                    host.id
                                );
                            }
                            ui.add_space(4.0);
                        }
                    });
            });
    }

    fn host_details(&mut self, ui: &mut Ui) {
        panel_title(ui, self.tr(TextKey::SelectedHost));
        Frame::new()
            .fill(Color32::WHITE)
            .stroke(Stroke::new(1.0, Color32::from_rgb(194, 201, 208)))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                if let Some(host) = self.selected_host().cloned() {
                    ui.horizontal(|ui| {
                        ui.heading(&host.name);
                        ui.add_space(6.0);
                        status_chip(ui, host.state.label(), host.state.tint(), host.state.fill());
                    });
                    ui.label(RichText::new(&host.note).color(Color32::from_rgb(79, 90, 101)));
                    ui.add_space(10.0);

                    Grid::new("host_details_grid")
                        .num_columns(2)
                        .spacing([12.0, 10.0])
                        .show(ui, |ui| {
                            detail_row(ui, self.tr(TextKey::HostId), &host.id);
                            detail_row(ui, self.tr(TextKey::ConnectCode), &host.connect_code);
                            detail_row(ui, self.tr(TextKey::Group), &host.group);
                            detail_row(ui, self.tr(TextKey::Endpoint), &host.endpoint);
                            detail_row(ui, self.tr(TextKey::Platform), &host.os);
                            detail_row(ui, self.tr(TextKey::Owner), &host.owner);
                            detail_row(ui, self.tr(TextKey::LastSeen), &host.last_seen);
                            detail_row(
                                ui,
                                self.tr(TextKey::SessionPolicy),
                                if host.permissions.input_control {
                                    self.tr(TextKey::InteractiveControlAllowed)
                                } else {
                                    self.tr(TextKey::ViewOnlyRestricted)
                                },
                            );
                        });

                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(12.0);

                    ui.horizontal_wrapped(|ui| {
                        if action_button(ui, self.tr(TextKey::Connect)).clicked() {
                            self.request_session(false);
                        }
                        if action_button(ui, self.tr(TextKey::View)).clicked() {
                            self.request_session(true);
                        }
                        if action_button(ui, "Завершить").clicked() {
                            self.close_session();
                        }
                        if action_button(ui, self.tr(TextKey::Files)).clicked() {
                            self.status_line = format!(
                                "Открыта рабочая область передачи файлов для {}.",
                                host.name
                            );
                            self.add_activity(format!(
                                "Открыт раздел передачи файлов для {}",
                                host.name
                            ));
                        }
                        if action_button(ui, self.tr(TextKey::CopyId)).clicked() {
                            ui.ctx().copy_text(host.id.clone());
                            self.status_line =
                                format!("ID хоста {} скопирован в буфер обмена.", host.id);
                        }
                    });

                    ui.add_space(16.0);
                    subpanel(ui, self.tr(TextKey::Permissions), |ui| {
                        ui.horizontal_wrapped(|ui| {
                            permission_chip(
                                ui,
                                self.tr(TextKey::Screen),
                                host.permissions.screen_capture,
                            );
                            permission_chip(
                                ui,
                                self.tr(TextKey::Input),
                                host.permissions.input_control,
                            );
                            permission_chip(
                                ui,
                                self.tr(TextKey::Access),
                                host.permissions.accessibility,
                            );
                            permission_chip(
                                ui,
                                self.tr(TextKey::Files),
                                host.permissions.file_transfer,
                            );
                        });
                    });

                    ui.add_space(10.0);
                    subpanel(ui, self.tr(TextKey::OperatorNotes), |ui| {
                        ui.label(&host.note);
                        if self.using_demo_data {
                            ui.label(self.tr(TextKey::DemoRecordNote));
                        } else {
                            ui.label(self.tr(TextKey::LiveRecordNote));
                        }
                    });
                } else {
                    empty_state(ui, self.tr(TextKey::SelectHostPrompt));
                }
            });
    }
}

impl App for ConsoleApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(250));
        self.maybe_auto_sign_in();
        self.ensure_signal_listener();
        self.process_signal_events();
        self.ensure_media_listener();
        self.process_media_events(ctx);
        self.ensure_selection_visible();
        self.top_bar(ctx);
        self.left_panel(ctx);
        self.center_panel(ctx);
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
        let group = if value.online {
            "В сети".to_owned()
        } else {
            "Не в сети".to_owned()
        };
        let note = format!(
            "{}. Код подключения обновляется автоматически; текущий слот истекает {}.",
            if value.permissions.input_control {
                "Устройство готово к удалённому управлению"
            } else {
                "Устройство с ограниченным управлением"
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
    style.visuals = egui::Visuals::light();
    style.visuals.window_corner_radius = CornerRadius::same(6);
    style.visuals.menu_corner_radius = CornerRadius::same(6);
    style.visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(245, 247, 249);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(242, 245, 248);
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(190, 198, 206));
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(226, 234, 243);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(86, 119, 164));
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(210, 223, 237);
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, Color32::from_rgb(63, 98, 145));
    style.visuals.selection.bg_fill = Color32::from_rgb(188, 210, 233);
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(10.0, 5.0);
    style.spacing.window_margin = egui::Margin::same(10);
    ctx.set_style(style);
}

fn menu_button(ui: &mut Ui, label: &str) -> egui::Response {
    ui.add(Button::new(label).fill(Color32::from_rgb(242, 245, 248)))
}

fn panel_title(ui: &mut Ui, title: &str) {
    ui.label(
        RichText::new(title)
            .strong()
            .size(14.5)
            .color(Color32::from_rgb(48, 61, 76)),
    );
    ui.add_space(4.0);
}

fn status_chip(ui: &mut Ui, text: &str, tint: Color32, fill: Color32) {
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, tint))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(text).strong().color(tint));
        });
}

fn permission_chip(ui: &mut Ui, text: &str, enabled: bool) {
    let tint = if enabled {
        Color32::from_rgb(47, 120, 62)
    } else {
        Color32::from_rgb(135, 97, 58)
    };
    let fill = if enabled {
        Color32::from_rgb(224, 241, 228)
    } else {
        Color32::from_rgb(245, 233, 219)
    };
    let label = if enabled {
        text.to_owned()
    } else {
        format!("{text} выкл")
    };
    status_chip(ui, &label, tint, fill);
}

fn inspector_card(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(Color32::WHITE)
        .stroke(Stroke::new(1.0, Color32::from_rgb(196, 202, 209)))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::same(10))
        .show(ui, add_contents);
}

fn labeled_edit(ui: &mut Ui, label: &str, value: &mut String, password: bool) {
    ui.label(RichText::new(label).strong());
    let mut edit = TextEdit::singleline(value).desired_width(f32::INFINITY);
    if password {
        edit = edit.password(true);
    }
    ui.add(edit);
}

fn table_header(ui: &mut Ui, language: AppLanguage) {
    ui.horizontal(|ui| {
        ui.add_space(22.0);
        column_text(ui, language.text(TextKey::Computer), 160.0, true);
        column_text(ui, "ID", 92.0, true);
        column_text(ui, language.text(TextKey::Endpoint), 180.0, true);
        column_text(ui, language.text(TextKey::ActivityShort), 110.0, true);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(language.text(TextKey::State)).strong());
        });
    });
}

fn column_text(ui: &mut Ui, text: &str, width: f32, strong: bool) {
    let rich = if strong {
        RichText::new(text).strong()
    } else {
        RichText::new(text)
    };
    ui.add_sized([width, 18.0], egui::Label::new(rich));
}

fn state_dot(ui: &mut Ui, state: HostState) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, state.tint());
}

fn detail_row(ui: &mut Ui, label: &str, value: &str) {
    ui.label(
        RichText::new(label)
            .strong()
            .color(Color32::from_rgb(75, 87, 100)),
    );
    ui.label(value);
    ui.end_row();
}

fn action_button(ui: &mut Ui, label: &str) -> egui::Response {
    ui.add(action_button_widget(label))
}

fn action_button_widget(label: &str) -> impl egui::Widget {
    Button::new(label)
        .min_size(Vec2::new(104.0, 30.0))
        .fill(Color32::from_rgb(235, 240, 246))
}

fn subpanel(ui: &mut Ui, title: &str, add_contents: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(Color32::from_rgb(247, 249, 251))
        .stroke(Stroke::new(1.0, Color32::from_rgb(210, 216, 222)))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.label(RichText::new(title).strong());
            ui.add_space(6.0);
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
                .fill(Color32::from_rgb(247, 249, 251))
                .stroke(Stroke::new(1.0, Color32::from_rgb(211, 217, 223)))
                .corner_radius(CornerRadius::same(6))
                .show(ui, |ui| {
                    ui.add_space(20.0);
                    ui.label(RichText::new(text).color(Color32::from_rgb(90, 101, 113)));
                    ui.add_space(20.0);
                });
        },
    );
}

fn normalize_server_url(value: &str) -> String {
    value.trim().trim_end_matches('/').to_owned()
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
