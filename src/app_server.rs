use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tungstenite::{Message, WebSocket, accept, client};

use crate::state::{RateLimitWindow, StatusSnapshot};

pub struct AppServerSource {
    child: Child,
    // Keeping stdin open owns the stdio transport lifetime. Dropping it immediately can make the
    // server observe EOF before the asynchronous rate-limit response is delivered.
    _stdin: ChildStdin,
}

impl AppServerSource {
    pub fn start(executable: &Path, snapshot: Arc<RwLock<StatusSnapshot>>) -> Result<Self> {
        let mut child = Command::new(executable)
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("could not start Codex app-server")?;
        let mut stdin = child.stdin.take().context("app-server stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("app-server stdout unavailable")?;

        for message in [
            json!({"id": 1, "method": "initialize", "params": {
                "clientInfo": {"name": "codexline", "title": "Codexline", "version": env!("CARGO_PKG_VERSION")},
                "capabilities": {"experimentalApi": true}
            }}),
            json!({"method": "initialized"}),
            json!({"id": 2, "method": "account/rateLimits/read"}),
        ] {
            serde_json::to_writer(&mut stdin, &message)?;
            stdin.write_all(b"\n")?;
        }
        stdin.flush()?;

        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if let Ok(mut state) = snapshot.write() {
                    apply_message(&message, &mut state);
                }
            }
        });
        Ok(Self {
            child,
            _stdin: stdin,
        })
    }
}

impl Drop for AppServerSource {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct ProtocolProxy {
    child: Child,
    endpoint: String,
}

impl ProtocolProxy {
    pub fn start(executable: &Path, snapshot: Arc<RwLock<StatusSnapshot>>) -> Result<Self> {
        let listener =
            TcpListener::bind("127.0.0.1:0").context("could not bind Codexline protocol proxy")?;
        let proxy_address = listener.local_addr()?;
        let reservation =
            TcpListener::bind("127.0.0.1:0").context("could not reserve an app-server port")?;
        let server_address = reservation.local_addr()?;
        drop(reservation);

        let upstream_url = format!("ws://{server_address}");
        let mut child = Command::new(executable)
            .args(["app-server", "--listen", &upstream_url])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("could not start websocket app-server")?;

        let deadline = Instant::now() + Duration::from_millis(300);
        let upstream = loop {
            match connect_websocket(server_address, &upstream_url) {
                Ok(socket) => break socket,
                Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error.context("app-server websocket was not ready within 300 ms"));
                }
            }
        };

        thread::spawn(move || {
            let Ok((downstream_stream, _)) = listener.accept() else {
                return;
            };
            let Ok(downstream) = accept(downstream_stream) else {
                return;
            };
            relay_protocol(downstream, upstream, snapshot);
        });

        Ok(Self {
            child,
            endpoint: format!("ws://{proxy_address}"),
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

impl Drop for ProtocolProxy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn connect_websocket(address: std::net::SocketAddr, url: &str) -> Result<WebSocket<TcpStream>> {
    let stream = TcpStream::connect_timeout(&address, Duration::from_millis(80))?;
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    stream.set_write_timeout(Some(Duration::from_millis(100)))?;
    let (socket, _) = client(url, stream).context("app-server websocket handshake failed")?;
    Ok(socket)
}

fn relay_protocol(
    mut downstream: WebSocket<TcpStream>,
    mut upstream: WebSocket<TcpStream>,
    snapshot: Arc<RwLock<StatusSnapshot>>,
) {
    let _ = downstream.get_mut().set_nonblocking(true);
    let _ = upstream.get_mut().set_nonblocking(true);
    if let Ok(mut state) = snapshot.write() {
        state.app_server_active = true;
    }
    let mut requests = HashMap::<String, String>::new();
    let mut to_upstream = VecDeque::<Message>::new();
    let mut to_downstream = VecDeque::<Message>::new();
    loop {
        let mut progressed = false;
        if to_upstream.len() < 256 {
            match downstream.read() {
                Ok(message) => {
                    progressed = true;
                    observe_client_message(&message, &mut requests);
                    to_upstream.push_back(message);
                }
                Err(error) if is_would_block(&error) => {}
                Err(_) => break,
            }
        }
        if to_downstream.len() < 256 {
            match upstream.read() {
                Ok(message) => {
                    progressed = true;
                    observe_server_message(&message, &mut requests, &snapshot);
                    to_downstream.push_back(message);
                }
                Err(error) if is_would_block(&error) => {}
                Err(_) => break,
            }
        }
        if try_forward(&mut upstream, &mut to_upstream).unwrap_or(false) {
            progressed = true;
        }
        if try_forward(&mut downstream, &mut to_downstream).unwrap_or(false) {
            progressed = true;
        }
        if flush_nonblocking(&mut upstream).is_err() || flush_nonblocking(&mut downstream).is_err()
        {
            break;
        }
        if !progressed {
            thread::sleep(Duration::from_millis(1));
        }
    }
}

fn try_forward(
    socket: &mut WebSocket<TcpStream>,
    queue: &mut VecDeque<Message>,
) -> Result<bool, tungstenite::Error> {
    let Some(message) = queue.front().cloned() else {
        return Ok(false);
    };
    match socket.write(message) {
        Ok(()) => {
            queue.pop_front();
            Ok(true)
        }
        Err(error)
            if is_would_block(&error)
                || matches!(error, tungstenite::Error::WriteBufferFull(_)) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

fn flush_nonblocking(socket: &mut WebSocket<TcpStream>) -> Result<(), tungstenite::Error> {
    match socket.flush() {
        Ok(()) => Ok(()),
        Err(error) if is_would_block(&error) => Ok(()),
        Err(error) => Err(error),
    }
}

fn is_would_block(error: &tungstenite::Error) -> bool {
    matches!(error, tungstenite::Error::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock)
}

fn observe_client_message(message: &Message, requests: &mut HashMap<String, String>) {
    let Some(text) = message.to_text().ok() else {
        return;
    };
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let (Some(id), Some(method)) = (value.get("id"), value.get("method").and_then(Value::as_str))
    else {
        return;
    };
    requests.insert(id.to_string(), method.to_owned());
}

fn observe_server_message(
    message: &Message,
    requests: &mut HashMap<String, String>,
    snapshot: &Arc<RwLock<StatusSnapshot>>,
) {
    let Some(text) = message.to_text().ok() else {
        return;
    };
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let response_method = value
        .get("id")
        .and_then(|id| requests.remove(&id.to_string()));
    let Ok(mut state) = snapshot.write() else {
        return;
    };
    if response_method.as_deref() == Some("account/rateLimits/read") {
        if let Some(result) = value.get("result") {
            apply_rate_limits(result, &mut state);
        }
    } else {
        apply_message(&value, &mut state);
    }
}

fn apply_message(message: &Value, snapshot: &mut StatusSnapshot) {
    if message.get("id").and_then(Value::as_u64) == Some(1) && message.get("result").is_some() {
        snapshot.app_server_active = true;
    }
    if message.get("id").and_then(Value::as_u64) == Some(2) {
        if let Some(result) = message.get("result") {
            apply_rate_limits(result, snapshot);
        }
        return;
    }
    match message.get("method").and_then(Value::as_str) {
        Some("account/rateLimits/updated") => {
            if let Some(params) = message.get("params") {
                apply_rate_limits(params, snapshot);
            }
        }
        Some("thread/tokenUsage/updated") => {
            if let Some(usage) = message.pointer("/params/tokenUsage") {
                apply_token_usage(usage, snapshot);
            }
        }
        _ => {}
    }
}

fn apply_rate_limits(value: &Value, snapshot: &mut StatusSnapshot) {
    let rate = value.get("rateLimits").unwrap_or(value);
    let windows = ["primary", "secondary"]
        .into_iter()
        .filter_map(|name| parse_window(rate.get(name)?))
        .collect::<Vec<_>>();
    if !windows.is_empty() {
        snapshot.rate_limits = windows;
    }
    if let Some(count) = value
        .pointer("/rateLimitResetCredits/availableCount")
        .and_then(Value::as_u64)
    {
        snapshot.reset_credits = Some(count.min(u64::from(u16::MAX)) as u16);
    }
}

fn parse_window(value: &Value) -> Option<RateLimitWindow> {
    Some(RateLimitWindow {
        used_percent: value.get("usedPercent")?.as_u64()?.min(100) as u8,
        window_minutes: value.get("windowDurationMins").and_then(Value::as_u64),
        resets_at: value.get("resetsAt").and_then(Value::as_u64),
    })
}

fn apply_token_usage(value: &Value, snapshot: &mut StatusSnapshot) {
    snapshot.context_window = value.get("modelContextWindow").and_then(Value::as_u64);
    let last = value.get("last").unwrap_or(value);
    snapshot.input_tokens = last.get("inputTokens").and_then(Value::as_u64);
    // Codex's native "context left" is based on the current input context. totalTokens also
    // includes generated output and can overstate pressure relative to the model window.
    snapshot.context_used = snapshot.input_tokens;
    snapshot.cached_input_tokens = last.get("cachedInputTokens").and_then(Value::as_u64);
    snapshot.output_tokens = last.get("outputTokens").and_then(Value::as_u64);
    snapshot.context_percent = match (snapshot.context_used, snapshot.context_window) {
        (Some(used), Some(window)) if window > 0 => Some(
            (used
                .saturating_mul(100)
                .saturating_add(window.saturating_sub(1))
                / window)
                .min(100) as u8,
        ),
        _ => None,
    };
}

#[cfg(test)]
mod tests {
    use super::apply_message;
    use crate::state::StatusSnapshot;
    use serde_json::json;

    #[test]
    fn parses_rate_limits_and_token_usage_without_requiring_all_fields() {
        let mut snapshot = StatusSnapshot::default();
        apply_message(
            &json!({"id": 2, "result": {"rateLimits": {
            "primary": {"usedPercent": 35, "windowDurationMins": 300},
            "secondary": {"usedPercent": 65, "windowDurationMins": 10080}
        }, "rateLimitResetCredits": {"availableCount": 1}}}),
            &mut snapshot,
        );
        assert_eq!(snapshot.rate_limits.len(), 2);
        assert_eq!(snapshot.reset_credits, Some(1));

        apply_message(
            &json!({"method": "thread/tokenUsage/updated", "params": {"tokenUsage": {
                "modelContextWindow": 200000,
                "last": {"totalTokens": 50000, "inputTokens": 40000, "cachedInputTokens": 30000, "outputTokens": 10000}
            }}}),
            &mut snapshot,
        );
        assert_eq!(snapshot.context_percent, Some(20));
        assert_eq!(snapshot.output_tokens, Some(10000));
    }
}
