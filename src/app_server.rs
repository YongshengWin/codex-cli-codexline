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

use crate::state::{AgentActivity, RateLimitWindow, StatusSnapshot};

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
        Some("thread/started") => {
            if let Some(thread) = message.pointer("/params/thread") {
                if thread
                    .get("parentThreadId")
                    .is_some_and(|value| !value.is_null())
                {
                    if let Some(id) = thread.get("id").and_then(Value::as_str) {
                        let kind = thread
                            .get("agentRole")
                            .or_else(|| thread.get("agentNickname"))
                            .and_then(Value::as_str)
                            .unwrap_or("agent");
                        upsert_agent(snapshot, id, kind, None);
                    }
                }
            }
        }
        Some("thread/status/changed") => {
            if let (Some(id), Some(status)) = (
                message.pointer("/params/threadId").and_then(Value::as_str),
                message
                    .pointer("/params/status/type")
                    .and_then(Value::as_str),
            ) {
                set_agent_status(snapshot, id, status, None);
            }
        }
        Some("item/started") | Some("item/completed") => {
            if let Some(item) = message.pointer("/params/item") {
                apply_agent_item(
                    item,
                    message.pointer("/params/threadId").and_then(Value::as_str),
                    snapshot,
                );
            }
        }
        _ => {}
    }
}

fn apply_agent_item(item: &Value, thread_id: Option<&str>, snapshot: &mut StatusSnapshot) {
    match item.get("type").and_then(Value::as_str) {
        Some("collabAgentToolCall") => {
            let prompt = item.get("prompt").and_then(Value::as_str);
            if let Some(receivers) = item.get("receiverThreadIds").and_then(Value::as_array) {
                for id in receivers.iter().filter_map(Value::as_str) {
                    upsert_agent(snapshot, id, "agent", prompt);
                }
            }
            if let Some(states) = item.get("agentsStates").and_then(Value::as_object) {
                for (id, state) in states {
                    let status = state
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("running");
                    let message = state.get("message").and_then(Value::as_str);
                    set_agent_status(snapshot, id, status, message);
                }
            }
        }
        Some("agentMessage") => {
            if let (Some(id), Some(text)) = (thread_id, item.get("text").and_then(Value::as_str)) {
                if let Some(agent) = snapshot.agents.iter_mut().find(|agent| agent.id == id) {
                    agent.message = Some(sanitize_agent_text(text));
                }
            }
        }
        _ => {}
    }
    sync_agent_counts(snapshot);
}

fn upsert_agent(snapshot: &mut StatusSnapshot, id: &str, kind: &str, prompt: Option<&str>) {
    if let Some(agent) = snapshot.agents.iter_mut().find(|agent| agent.id == id) {
        if agent.kind == "agent" && kind != "agent" {
            agent.kind = sanitize_agent_text(kind);
        }
        if let Some(prompt) = prompt {
            agent.prompt = Some(sanitize_agent_text(prompt));
        }
        agent.active = true;
    } else {
        snapshot.agents.push(AgentActivity {
            id: id.into(),
            kind: sanitize_agent_text(kind),
            started: Instant::now(),
            prompt: prompt.map(sanitize_agent_text),
            message: None,
            active: true,
        });
    }
    sync_agent_counts(snapshot);
}

fn set_agent_status(snapshot: &mut StatusSnapshot, id: &str, status: &str, message: Option<&str>) {
    if !snapshot.agents.iter().any(|agent| agent.id == id) {
        upsert_agent(snapshot, id, "agent", None);
    }
    if let Some(agent) = snapshot.agents.iter_mut().find(|agent| agent.id == id) {
        agent.active = matches!(status, "pendingInit" | "running" | "active");
        if let Some(message) = message {
            agent.message = Some(sanitize_agent_text(message));
        }
    }
    sync_agent_counts(snapshot);
}

fn sync_agent_counts(snapshot: &mut StatusSnapshot) {
    snapshot.agents_active = Some(
        snapshot
            .agents
            .iter()
            .filter(|agent| agent.active)
            .count()
            .min(usize::from(u16::MAX)) as u16,
    );
    snapshot.agents_total = Some(snapshot.agents.len().min(usize::from(u16::MAX)) as u16);
}

fn sanitize_agent_text(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(160)
        .collect()
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

    #[test]
    fn tracks_spawned_agent_status_and_latest_message() {
        let mut snapshot = StatusSnapshot::default();
        apply_message(
            &json!({"method": "item/completed", "params": {
                "threadId": "root", "turnId": "turn-1", "item": {
                    "type": "collabAgentToolCall", "id": "item-1",
                    "tool": "spawnAgent", "status": "completed",
                    "senderThreadId": "root", "receiverThreadIds": ["child-1"],
                    "prompt": "Inspect the renderer", "model": null,
                    "reasoningEffort": null,
                    "agentsStates": {"child-1": {"status": "running", "message": "Reading files"}}
                }
            }}),
            &mut snapshot,
        );
        assert_eq!(snapshot.agents_active, Some(1));
        assert_eq!(
            snapshot.agents[0].prompt.as_deref(),
            Some("Inspect the renderer")
        );
        assert_eq!(snapshot.agents[0].message.as_deref(), Some("Reading files"));

        apply_message(
            &json!({"method": "item/completed", "params": {
                "threadId": "child-1", "turnId": "turn-2",
                "item": {"type": "agentMessage", "id": "item-2", "text": "Found the layout", "phase": "final_answer", "memoryCitation": null}
            }}),
            &mut snapshot,
        );
        assert_eq!(
            snapshot.agents[0].message.as_deref(),
            Some("Found the layout")
        );
    }
}
