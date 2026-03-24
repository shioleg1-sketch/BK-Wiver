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
    SessionAccepted { session_id: String },
    SessionRejected { session_id: String },
    SessionClosed { session_id: String },
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

pub fn send_session_closed(server_url: &str, token: &str, session_id: &str) -> Result<(), String> {
    send_message(
        server_url,
        token,
        json!({
            "type": "session.closed",
            "sessionId": session_id,
        }),
    )
}

pub fn send_mouse_event(
    server_url: &str,
    token: &str,
    session_id: &str,
    action: &str,
    button: &str,
    x_norm: f32,
    y_norm: f32,
    scroll_x: f32,
    scroll_y: f32,
) -> Result<(), String> {
    send_message(
        server_url,
        token,
        json!({
            "type": "session.input_mouse",
            "sessionId": session_id,
            "action": action,
            "button": button,
            "xNorm": x_norm,
            "yNorm": y_norm,
            "scrollX": scroll_x,
            "scrollY": scroll_y,
        }),
    )
}

pub fn send_key_text(
    server_url: &str,
    token: &str,
    session_id: &str,
    text: &str,
) -> Result<(), String> {
    send_message(
        server_url,
        token,
        json!({
            "type": "session.input_key",
            "sessionId": session_id,
            "kind": "text",
            "text": text,
        }),
    )
}

pub fn send_key_named(
    server_url: &str,
    token: &str,
    session_id: &str,
    key: &str,
    modifiers: &[&str],
) -> Result<(), String> {
    send_message(
        server_url,
        token,
        json!({
            "type": "session.input_key",
            "sessionId": session_id,
            "kind": "named",
            "key": key,
            "modifiers": modifiers,
        }),
    )
}

pub fn send_media_feedback(
    server_url: &str,
    token: &str,
    session_id: &str,
    profile: &str,
    codec: &str,
) -> Result<(), String> {
    send_message(
        server_url,
        token,
        json!({
            "type": "session.media_feedback",
            "sessionId": session_id,
            "profile": profile,
            "codec": codec,
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
        "session.accepted" => Some(SignalEvent::SessionAccepted {
            session_id: payload.get("sessionId")?.as_str()?.to_owned(),
        }),
        "session.rejected" => Some(SignalEvent::SessionRejected {
            session_id: payload.get("sessionId")?.as_str()?.to_owned(),
        }),
        "session.closed" => Some(SignalEvent::SessionClosed {
            session_id: payload.get("sessionId")?.as_str()?.to_owned(),
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
