use std::{
    collections::HashMap,
    io::ErrorKind,
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};

use crossbeam_channel::{Sender, unbounded};
use serde_json::{Value, json};
use tungstenite::{Message, connect, stream::MaybeTlsStream};
use url::Url;

#[derive(Clone)]
pub enum SignalEvent {
    Connected,
    Disconnected {
        reason: String,
    },
    SessionRequested {
        session_id: String,
        from_user_id: String,
    },
    SessionClosed {
        session_id: String,
    },
    MouseInput {
        session_id: String,
        action: String,
        button: String,
        x_norm: f32,
        y_norm: f32,
        scroll_x: f32,
        scroll_y: f32,
    },
    KeyInput {
        session_id: String,
        kind: String,
        key: String,
        text: String,
        modifiers: Vec<String>,
    },
    MediaFeedback {
        session_id: String,
        profile: String,
        codec: Option<String>,
    },
}

const SIGNAL_READ_TIMEOUT: Duration = Duration::from_secs(10);
const SIGNAL_PING_INTERVAL: Duration = Duration::from_secs(10);
const SIGNAL_RECONNECT_DELAYS_MS: [u64; 4] = [1_000, 2_000, 5_000, 10_000];

static SIGNAL_OUTBOUND: OnceLock<Mutex<HashMap<String, Sender<Value>>>> = OnceLock::new();

fn outbound_registry() -> &'static Mutex<HashMap<String, Sender<Value>>> {
    SIGNAL_OUTBOUND.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn spawn_listener(server_url: String, token: String, event_tx: Sender<SignalEvent>) {
    let connection_key = signal_connection_key(&server_url, &token);
    let (outbound_tx, outbound_rx) = unbounded::<Value>();
    if let Ok(mut registry) = outbound_registry().lock() {
        registry.insert(connection_key.clone(), outbound_tx);
    }

    thread::spawn(move || {
        let mut was_connected = false;
        let mut reconnect_attempt = 0usize;
        let Ok(url) = signal_url(&server_url, &token) else {
            let _ = event_tx.send(SignalEvent::Disconnected {
                reason: "invalid websocket url".to_owned(),
            });
            return;
        };

        loop {
            match connect(url.as_str()) {
                Ok((mut socket, _)) => {
                    // Увеличиваем таймаут чтения до 10 секунд для стабильности соединения
                    if let MaybeTlsStream::Plain(stream) = socket.get_mut() {
                        let _ = stream.set_read_timeout(Some(SIGNAL_READ_TIMEOUT));
                    }
                    was_connected = true;
                    reconnect_attempt = 0;
                    let _ = event_tx.send(SignalEvent::Connected);
                    let mut last_ping_at = Instant::now();
                    let mut disconnect_reason = "websocket closed".to_owned();

                    loop {
                        loop {
                            match outbound_rx.try_recv() {
                                Ok(payload) => {
                                    if socket
                                        .send(Message::Text(payload.to_string().into()))
                                        .is_err()
                                    {
                                        disconnect_reason =
                                            "failed to send queued signal message".to_owned();
                                        break;
                                    }
                                }
                                Err(crossbeam_channel::TryRecvError::Empty) => break,
                                Err(crossbeam_channel::TryRecvError::Disconnected) => return,
                            }
                        }

                        if last_ping_at.elapsed() >= SIGNAL_PING_INTERVAL {
                            if socket.send(Message::Ping(Vec::new().into())).is_err() {
                                disconnect_reason = "failed to send ping".to_owned();
                                break;
                            }
                            last_ping_at = Instant::now();
                        }

                        match socket.read() {
                            Ok(Message::Text(text)) => {
                                if let Some(event) = parse_signal_event(text.as_str()) {
                                    let _ = event_tx.send(event);
                                }
                            }
                            Ok(Message::Ping(payload)) => {
                                let _ = socket.send(Message::Pong(payload));
                            }
                            Ok(Message::Pong(_)) => {
                                last_ping_at = Instant::now();
                            }
                            Ok(Message::Close(_)) => break,
                            Ok(_) => {}
                            Err(tungstenite::Error::Io(error))
                                if matches!(
                                    error.kind(),
                                    ErrorKind::WouldBlock | ErrorKind::TimedOut
                                ) =>
                            {
                                disconnect_reason = format!("read timeout: {}", error);
                                break;
                            }
                            Err(error) => {
                                disconnect_reason = format!("socket error: {error}");
                                break;
                            }
                        }
                    }

                    if was_connected {
                        let _ = event_tx.send(SignalEvent::Disconnected {
                            reason: disconnect_reason,
                        });
                        was_connected = false;
                    }
                }
                Err(error) => {
                    if !was_connected {
                        let _ = event_tx.send(SignalEvent::Disconnected {
                            reason: format!("connect failed: {error}"),
                        });
                    }
                }
            }

            let delay_ms = SIGNAL_RECONNECT_DELAYS_MS
                .get(reconnect_attempt)
                .copied()
                .unwrap_or(*SIGNAL_RECONNECT_DELAYS_MS.last().unwrap_or(&10_000));
            reconnect_attempt = reconnect_attempt.saturating_add(1);
            thread::sleep(Duration::from_millis(delay_ms));
        }
    });
}

pub fn send_session_accepted(
    server_url: &str,
    token: &str,
    session_id: &str,
) -> Result<(), String> {
    send_message(
        server_url,
        token,
        json!({
            "type": "session.accepted",
            "sessionId": session_id,
        }),
    )
}

pub fn send_message(server_url: &str, token: &str, payload: Value) -> Result<(), String> {
    if let Ok(registry) = outbound_registry().lock()
        && let Some(tx) = registry.get(&signal_connection_key(server_url, token))
    {
        return tx.send(payload).map_err(|error| error.to_string());
    }

    let url = signal_url(server_url, token)?;
    let (mut socket, _) = connect(url.as_str()).map_err(|error| error.to_string())?;
    socket
        .send(Message::Text(payload.to_string().into()))
        .map_err(|error| error.to_string())?;
    let _ = socket.close(None);
    Ok(())
}

fn parse_signal_event(text: &str) -> Option<SignalEvent> {
    let payload: Value = serde_json::from_str(text).ok()?;
    match payload.get("type")?.as_str()? {
        "session.request" => Some(SignalEvent::SessionRequested {
            session_id: payload.get("sessionId")?.as_str()?.to_owned(),
            from_user_id: payload
                .get("fromUserId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        }),
        "session.closed" => Some(SignalEvent::SessionClosed {
            session_id: payload.get("sessionId")?.as_str()?.to_owned(),
        }),
        "session.input_mouse" => Some(SignalEvent::MouseInput {
            session_id: payload.get("sessionId")?.as_str()?.to_owned(),
            action: payload
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("click")
                .to_owned(),
            button: payload
                .get("button")
                .and_then(Value::as_str)
                .unwrap_or("left")
                .to_owned(),
            x_norm: payload.get("xNorm").and_then(Value::as_f64).unwrap_or(0.5) as f32,
            y_norm: payload.get("yNorm").and_then(Value::as_f64).unwrap_or(0.5) as f32,
            scroll_x: payload
                .get("scrollX")
                .and_then(Value::as_f64)
                .unwrap_or_default() as f32,
            scroll_y: payload
                .get("scrollY")
                .and_then(Value::as_f64)
                .unwrap_or_default() as f32,
        }),
        "session.input_key" => Some(SignalEvent::KeyInput {
            session_id: payload.get("sessionId")?.as_str()?.to_owned(),
            kind: payload
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("named")
                .to_owned(),
            key: payload
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            text: payload
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            modifiers: payload
                .get("modifiers")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
        }),
        "session.media_feedback" => Some(SignalEvent::MediaFeedback {
            session_id: payload.get("sessionId")?.as_str()?.to_owned(),
            profile: payload
                .get("profile")
                .and_then(Value::as_str)
                .unwrap_or("balanced")
                .to_owned(),
            codec: payload.get("codec").and_then(Value::as_str).map(str::to_owned),
        }),
        _ => None,
    }
}

fn signal_url(server_url: &str, token: &str) -> Result<Url, String> {
    let normalized = server_url.trim().trim_end_matches('/');
    let ws_base = if let Some(rest) = normalized.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = normalized.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if normalized.starts_with("ws://") || normalized.starts_with("wss://") {
        normalized.to_owned()
    } else {
        format!("ws://{normalized}")
    };

    Url::parse(&format!("{ws_base}/ws/v1/signal?token={token}")).map_err(|error| error.to_string())
}

fn signal_connection_key(server_url: &str, token: &str) -> String {
    format!("{}|{}", server_url.trim().trim_end_matches('/'), token.trim())
}
