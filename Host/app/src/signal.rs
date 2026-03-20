use std::{thread, time::Duration};

use crossbeam_channel::Sender;
use serde_json::{Value, json};
use tungstenite::{Message, connect};
use url::Url;

#[derive(Clone)]
pub enum SignalEvent {
    Connected,
    Disconnected,
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
    },
    KeyInput {
        session_id: String,
        kind: String,
        key: String,
        text: String,
    },
}

pub fn spawn_listener(server_url: String, token: String, event_tx: Sender<SignalEvent>) {
    thread::spawn(move || {
        let mut was_connected = false;
        let Ok(url) = signal_url(&server_url, &token) else {
            let _ = event_tx.send(SignalEvent::Disconnected);
            return;
        };

        loop {
            match connect(url.as_str()) {
                Ok((mut socket, _)) => {
                    was_connected = true;
                    let _ = event_tx.send(SignalEvent::Connected);

                    loop {
                        match socket.read() {
                            Ok(Message::Text(text)) => {
                                if let Some(event) = parse_signal_event(text.as_str()) {
                                    let _ = event_tx.send(event);
                                }
                            }
                            Ok(Message::Ping(payload)) => {
                                let _ = socket.send(Message::Pong(payload));
                            }
                            Ok(Message::Close(_)) => break,
                            Ok(_) => {}
                            Err(_) => break,
                        }
                    }
                }
                Err(_) => {
                    if !was_connected {
                        let _ = event_tx.send(SignalEvent::Disconnected);
                    }
                }
            }

            if was_connected {
                let _ = event_tx.send(SignalEvent::Disconnected);
                was_connected = false;
            }
            thread::sleep(Duration::from_secs(2));
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
