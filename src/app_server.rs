use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tungstenite::{Message, WebSocket, accept, client};

use crate::state::{AgentActivity, LiveProxyStatus, RateLimitWindow, StatusSnapshot, ToolCount};

const RELAY_MAX_MESSAGES: usize = 256;
const RELAY_MAX_BYTES: usize = 64 * 1024 * 1024;

pub struct AppServerSource {
    child: Child,
    // Keeping stdin open owns the stdio transport lifetime. Dropping it immediately can make the
    // server observe EOF before the asynchronous rate-limit response is delivered.
    _stdin: Arc<Mutex<ChildStdin>>,
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

        let stdin = Arc::new(Mutex::new(stdin));
        let reader_snapshot = Arc::clone(&snapshot);
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if let Ok(mut state) = reader_snapshot.write() {
                    apply_message(&message, &mut state);
                }
            }
        });
        let poll_stdin = Arc::clone(&stdin);
        thread::spawn(move || poll_stored_thread(poll_stdin, snapshot));
        Ok(Self {
            child,
            _stdin: stdin,
        })
    }
}

fn poll_stored_thread(stdin: Arc<Mutex<ChildStdin>>, snapshot: Arc<RwLock<StatusSnapshot>>) {
    loop {
        thread::sleep(Duration::from_secs(1));
        let (session_id, agent_ids) = snapshot.read().map_or_else(
            |_| (None, Vec::new()),
            |state| {
                (
                    state.session_id.clone(),
                    state
                        .agents
                        .iter()
                        .take(8)
                        .map(|agent| agent.id.clone())
                        .collect::<Vec<_>>(),
                )
            },
        );
        let Some(session_id) = session_id else {
            continue;
        };
        let mut requests = Vec::with_capacity(agent_ids.len() + 1);
        requests.push(json!({"id": format!("codexline:root:{session_id}"), "method": "thread/turns/list", "params": {
            "threadId": session_id, "limit": 1, "sortDirection": "desc", "itemsView": "full"
        }}));
        requests.extend(agent_ids.into_iter().enumerate().map(|(index, thread_id)| {
            json!({"id": format!("codexline:agent:{thread_id}:{index}"), "method": "thread/turns/list", "params": {
                "threadId": thread_id, "limit": 1, "sortDirection": "desc", "itemsView": "full"
            }})
        }));
        let Ok(mut stdin) = stdin.lock() else {
            break;
        };
        let write_result = requests.into_iter().try_for_each(|request| {
            serde_json::to_writer(&mut *stdin, &request)?;
            stdin.write_all(b"\n")?;
            Ok::<_, anyhow::Error>(())
        });
        if write_result.is_err() || stdin.flush().is_err() {
            break;
        }
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
        if let Ok(mut state) = snapshot.write() {
            state.live_proxy_status = LiveProxyStatus::Starting;
            state.live_proxy_error = None;
        }
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

        // App-server startup varies considerably on cold Windows/macOS launches. This wait is
        // still bounded and occurs before Codex is told to use the relay, so failure can safely
        // fall back to a normal native launch.
        let startup_timeout = if cfg!(windows) {
            Duration::from_millis(1_500)
        } else {
            Duration::from_millis(800)
        };
        let deadline = Instant::now() + startup_timeout;
        let upstream = loop {
            match connect_websocket(server_address, &upstream_url) {
                Ok(socket) => break socket,
                Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    if let Ok(mut state) = snapshot.write() {
                        state.live_proxy_status = LiveProxyStatus::Closed;
                        state.live_proxy_error = Some(format!(
                            "app-server was not ready within {} ms",
                            startup_timeout.as_millis()
                        ));
                    }
                    return Err(error.context(format!(
                        "app-server websocket was not ready within {} ms",
                        startup_timeout.as_millis()
                    )));
                }
            }
        };

        if let Ok(mut state) = snapshot.write() {
            state.live_proxy_status = LiveProxyStatus::WaitingForCodex;
        }
        thread::spawn(move || {
            let result = listener
                .accept()
                .context("Codex did not connect to the live relay")
                .and_then(|(downstream_stream, _)| {
                    accept(downstream_stream).context("Codex live relay handshake failed")
                });
            match result {
                Ok(downstream) => relay_protocol(downstream, upstream, snapshot),
                Err(error) => mark_proxy_closed(&snapshot, error.to_string()),
            }
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
        state.live_session_active = true;
        state.live_proxy_status = LiveProxyStatus::Connected;
        state.live_proxy_error = None;
    }
    let mut requests = HashMap::<String, String>::new();
    let mut to_upstream = RelayQueue::default();
    let mut to_downstream = RelayQueue::default();
    let close_reason = loop {
        let mut progressed = false;
        if to_upstream.can_read() {
            match downstream.read() {
                Ok(message) => {
                    progressed = true;
                    match message {
                        Message::Text(_) | Message::Binary(_) => {
                            observe_client_message(&message, &mut requests, &snapshot);
                            to_upstream.push(message);
                        }
                        Message::Close(frame) => {
                            to_upstream.push(Message::Close(frame));
                            break "Codex closed the live connection".to_owned();
                        }
                        // Tungstenite answers Ping on this hop automatically. Forwarding control
                        // frames would create a second, unrelated heartbeat across the other hop.
                        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
                    }
                }
                Err(error) if is_would_block(&error) => {}
                Err(tungstenite::Error::ConnectionClosed) => {
                    break "Codex closed the live connection".to_owned();
                }
                Err(error) => break format!("Codex relay read failed: {error}"),
            }
        }
        if to_downstream.can_read() {
            match upstream.read() {
                Ok(message) => {
                    progressed = true;
                    match message {
                        Message::Text(_) | Message::Binary(_) => {
                            observe_server_message(&message, &mut requests, &snapshot);
                            to_downstream.push(message);
                        }
                        Message::Close(frame) => {
                            to_downstream.push(Message::Close(frame));
                            break "Codex app-server closed the live connection".to_owned();
                        }
                        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
                    }
                }
                Err(error) if is_would_block(&error) => {}
                Err(tungstenite::Error::ConnectionClosed) => {
                    break "Codex app-server closed the live connection".to_owned();
                }
                Err(error) => break format!("app-server relay read failed: {error}"),
            }
        }
        match try_forward(&mut upstream, &mut to_upstream) {
            Ok(wrote) => progressed |= wrote,
            Err(error) => break format!("app-server relay write failed: {error}"),
        }
        match try_forward(&mut downstream, &mut to_downstream) {
            Ok(wrote) => progressed |= wrote,
            Err(error) => break format!("Codex relay write failed: {error}"),
        }
        if let Err(error) = flush_nonblocking(&mut upstream) {
            break format!("app-server relay flush failed: {error}");
        }
        if let Err(error) = flush_nonblocking(&mut downstream) {
            break format!("Codex relay flush failed: {error}");
        }
        if !progressed {
            thread::sleep(Duration::from_millis(1));
        }
    };

    // Best-effort close propagation. Never block the PTY owner while a broken peer drains.
    let _ = try_forward(&mut upstream, &mut to_upstream);
    let _ = try_forward(&mut downstream, &mut to_downstream);
    let _ = flush_nonblocking(&mut upstream);
    let _ = flush_nonblocking(&mut downstream);
    mark_proxy_closed(&snapshot, close_reason);
}

#[derive(Default)]
struct RelayQueue {
    messages: VecDeque<Message>,
    bytes: usize,
}

impl RelayQueue {
    fn can_read(&self) -> bool {
        self.messages.len() < RELAY_MAX_MESSAGES && self.bytes < RELAY_MAX_BYTES
    }

    fn push(&mut self, message: Message) {
        self.bytes = self.bytes.saturating_add(message.len());
        self.messages.push_back(message);
    }

    fn pop(&mut self) -> Option<Message> {
        let message = self.messages.pop_front()?;
        self.bytes = self.bytes.saturating_sub(message.len());
        Some(message)
    }

    fn push_front(&mut self, message: Message) {
        self.bytes = self.bytes.saturating_add(message.len());
        self.messages.push_front(message);
    }
}

fn try_forward(
    socket: &mut WebSocket<TcpStream>,
    queue: &mut RelayQueue,
) -> Result<bool, tungstenite::Error> {
    let Some(message) = queue.pop() else {
        return Ok(false);
    };
    match socket.write(message) {
        Ok(()) => Ok(true),
        // A WouldBlock from write means tungstenite retained the frame in its own buffer. It
        // must not be re-enqueued here or the JSON-RPC request is sent twice.
        Err(error) if is_would_block(&error) => Ok(true),
        Err(tungstenite::Error::WriteBufferFull(message)) => {
            queue.push_front(*message);
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

fn mark_proxy_closed(snapshot: &Arc<RwLock<StatusSnapshot>>, reason: String) {
    if let Ok(mut state) = snapshot.write() {
        state.app_server_active = false;
        state.live_session_active = false;
        state.live_proxy_status = LiveProxyStatus::Closed;
        state.live_proxy_error = Some(reason);
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

fn observe_client_message(
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
    let (Some(id), Some(method)) = (value.get("id"), value.get("method").and_then(Value::as_str))
    else {
        return;
    };
    if matches!(method, "thread/start" | "thread/resume" | "turn/start") {
        if let (Some(params), Ok(mut state)) = (value.get("params"), snapshot.write()) {
            apply_runtime_params(params, &mut state);
        }
    }
    requests.insert(id.to_string(), method.to_owned());
}

fn apply_runtime_params(params: &Value, snapshot: &mut StatusSnapshot) {
    if let Some(thread_id) = params.get("threadId").and_then(Value::as_str) {
        snapshot.session_id = Some(thread_id.into());
    }
    if let Some(model) = params.get("model").and_then(Value::as_str) {
        snapshot.model = Some(model.into());
        snapshot.model_live = true;
    }
    if let Some(reasoning) = params
        .get("effort")
        .or_else(|| params.get("reasoningEffort"))
        .and_then(Value::as_str)
    {
        snapshot.reasoning = Some(reasoning.into());
        snapshot.model_live = true;
    }
    if let Some(cwd) = params.get("cwd").and_then(Value::as_str) {
        snapshot.cwd = Some(cwd.into());
    }

    let mut settings_updated = false;
    if let Some(policy) = params.get("approvalPolicy").and_then(Value::as_str) {
        snapshot.approval_policy = Some(policy.into());
        settings_updated = true;
    }
    if let Some(reviewer) = params.get("approvalsReviewer").and_then(Value::as_str) {
        snapshot.approvals_reviewer = Some(reviewer.into());
        settings_updated = true;
    }
    if let Some(permissions) = params.get("permissions").and_then(Value::as_str) {
        snapshot.permission_mode = Some(permissions.into());
        settings_updated = true;
    }
    if let Some(sandbox) = params.get("sandbox").and_then(Value::as_str) {
        snapshot.sandbox = Some(sandbox.into());
        settings_updated = true;
    }
    if let Some(sandbox) = params
        .pointer("/sandboxPolicy/type")
        .and_then(Value::as_str)
    {
        snapshot.sandbox = Some(sandbox.into());
        settings_updated = true;
    }
    if settings_updated {
        snapshot.settings_live = true;
    }
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
    if let Some(thread) = message.pointer("/result/thread") {
        apply_stored_thread(thread, snapshot);
        return;
    }
    if let Some(request_id) = message.get("id").and_then(Value::as_str) {
        if let Some(session_id) = request_id.strip_prefix("codexline:root:") {
            if snapshot.session_id.as_deref() == Some(session_id) {
                if let Some(turns) = message.pointer("/result/data").and_then(Value::as_array) {
                    apply_parent_turns(turns, snapshot);
                }
            }
            return;
        }
        if let Some(agent) = request_id.strip_prefix("codexline:agent:") {
            let thread_id = agent.rsplit_once(':').map_or(agent, |(id, _)| id);
            if let Some(turns) = message.pointer("/result/data").and_then(Value::as_array) {
                apply_agent_turns(thread_id, turns, snapshot);
            }
            return;
        }
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
                } else if let Some(id) = thread.get("id").and_then(Value::as_str) {
                    snapshot.session_id = Some(id.into());
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
                if snapshot.session_id.as_deref() == Some(id) {
                    snapshot.work = Some(
                        match status {
                            "active" => "working",
                            "idle" => "ready",
                            "systemError" => "error",
                            "notLoaded" => "ended",
                            other => other,
                        }
                        .into(),
                    );
                } else {
                    set_agent_status(snapshot, id, status, None);
                }
            }
        }
        Some("turn/started") => snapshot.work = Some("working".into()),
        Some("turn/completed") => {
            let status = message
                .pointer("/params/turn/status")
                .and_then(Value::as_str)
                .unwrap_or("completed");
            snapshot.work = Some(
                match status {
                    "completed" => "ready",
                    "interrupted" => "interrupted",
                    "failed" => "failed",
                    other => other,
                }
                .into(),
            );
        }
        Some("turn/plan/updated") => apply_live_plan(message, snapshot),
        Some(method @ ("item/started" | "item/completed")) => {
            if let Some(item) = message.pointer("/params/item") {
                apply_agent_item(
                    item,
                    message.pointer("/params/threadId").and_then(Value::as_str),
                    snapshot,
                );
                apply_live_item(item, method == "item/completed", snapshot);
            }
        }
        _ => {}
    }
}

fn apply_stored_thread(thread: &Value, snapshot: &mut StatusSnapshot) {
    let Some(thread_id) = thread.get("id").and_then(Value::as_str) else {
        return;
    };
    let is_subagent = thread
        .get("parentThreadId")
        .is_some_and(|parent| !parent.is_null());
    if is_subagent {
        let kind = thread
            .pointer("/source/subAgent/thread_spawn/agent_path")
            .and_then(Value::as_str)
            .and_then(|path| path.rsplit('/').find(|part| !part.is_empty()))
            .or_else(|| thread.get("agentRole").and_then(Value::as_str))
            .or_else(|| thread.get("agentNickname").and_then(Value::as_str))
            .unwrap_or("agent");
        upsert_agent(snapshot, thread_id, kind, None);
        let turns = thread
            .get("turns")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if let Some(latest) = turns.last() {
            apply_agent_turns(thread_id, std::slice::from_ref(latest), snapshot);
        }
        return;
    }

    if snapshot.session_id.is_none() {
        snapshot.session_id = Some(thread_id.into());
    } else if snapshot.session_id.as_deref() != Some(thread_id) {
        return;
    }
    let Some(turns) = thread.get("turns").and_then(Value::as_array) else {
        return;
    };
    apply_parent_turns(turns, snapshot);
}

fn apply_parent_turns(turns: &[Value], snapshot: &mut StatusSnapshot) {
    for item in turns
        .iter()
        .filter_map(|turn| turn.get("items").and_then(Value::as_array))
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("subAgentActivity"))
    {
        let (Some(id), Some(path)) = (
            item.get("agentThreadId").and_then(Value::as_str),
            item.get("agentPath").and_then(Value::as_str),
        ) else {
            continue;
        };
        let kind = path
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or("agent");
        upsert_agent(snapshot, id, kind, None);
        if item.get("kind").and_then(Value::as_str) != Some("started") {
            set_agent_status(snapshot, id, "completed", None);
        }
    }
}

fn apply_agent_turns(thread_id: &str, turns: &[Value], snapshot: &mut StatusSnapshot) {
    let status = turns
        .first()
        .and_then(|turn| turn.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("running");
    let message = turns.iter().find_map(|turn| {
        turn.get("items")
            .and_then(Value::as_array)?
            .iter()
            .rev()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("agentMessage"))?
            .get("text")?
            .as_str()
    });
    set_agent_status(snapshot, thread_id, status, message);
}

fn apply_agent_item(item: &Value, thread_id: Option<&str>, snapshot: &mut StatusSnapshot) {
    match item.get("type").and_then(Value::as_str) {
        Some("collabToolCall") => {
            let prompt = item.get("prompt").and_then(Value::as_str);
            let id = item
                .get("newThreadId")
                .or_else(|| item.get("receiverThreadId"))
                .and_then(Value::as_str);
            if let Some(id) = id {
                let kind = item
                    .pointer("/agentStatus/name")
                    .or_else(|| item.pointer("/agentStatus/role"))
                    .and_then(Value::as_str)
                    .unwrap_or("agent");
                upsert_agent(snapshot, id, kind, prompt);
                if let Some(status) = item.pointer("/agentStatus/status").and_then(Value::as_str) {
                    set_agent_status(snapshot, id, status, None);
                }
            }
        }
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

fn apply_live_plan(message: &Value, snapshot: &mut StatusSnapshot) {
    let Some(plan) = message.pointer("/params/plan").and_then(Value::as_array) else {
        return;
    };
    snapshot.plan_completed = Some(
        plan.iter()
            .filter(|step| step.get("status").and_then(Value::as_str) == Some("completed"))
            .count()
            .min(usize::from(u16::MAX)) as u16,
    );
    snapshot.plan_total = Some(plan.len().min(usize::from(u16::MAX)) as u16);
}

fn apply_live_item(item: &Value, completed: bool, snapshot: &mut StatusSnapshot) {
    let Some(item_type) = item.get("type").and_then(Value::as_str) else {
        return;
    };
    if item_type == "contextCompaction" {
        if completed {
            snapshot.compactions = Some(snapshot.compactions.unwrap_or(0).saturating_add(1));
            snapshot.work = Some("working".into());
        } else {
            snapshot.work = Some("compacting".into());
        }
        return;
    }
    let tool = match item_type {
        "commandExecution" => Some("exec"),
        "fileChange" => Some("patch"),
        "webSearch" => Some("web"),
        "collabToolCall" | "collabAgentToolCall" => Some("agent"),
        "mcpToolCall" => item
            .get("tool")
            .and_then(Value::as_str)
            .or_else(|| item.get("server").and_then(Value::as_str)),
        "dynamicToolCall" => item.get("tool").and_then(Value::as_str),
        _ => None,
    };
    let Some(tool) = tool.map(compact_protocol_label) else {
        return;
    };
    if completed {
        record_live_tool(snapshot, &tool);
        snapshot.work = Some("working".into());
    } else {
        snapshot.work = Some(format!("running · {tool}"));
    }
}

fn compact_protocol_label(value: &str) -> String {
    value
        .rsplit(['/', '_'])
        .find(|part| !part.is_empty())
        .unwrap_or(value)
        .chars()
        .filter(|character| !character.is_control())
        .take(20)
        .collect()
}

fn record_live_tool(snapshot: &mut StatusSnapshot, tool: &str) {
    let count = snapshot
        .tools
        .iter()
        .find(|entry| entry.name == tool)
        .map_or(1, |entry| entry.count.saturating_add(1));
    snapshot.tools.retain(|entry| entry.name != tool);
    snapshot.tools.insert(
        0,
        ToolCount {
            name: tool.into(),
            count,
        },
    );
    snapshot.tools.truncate(4);
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
        agent.active = matches!(status, "pendingInit" | "running" | "active" | "inProgress");
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
    let total = value.get("total").unwrap_or(last);
    // Codex's native "context left" is based on the current input context. totalTokens also
    // includes generated output and can overstate pressure relative to the model window.
    snapshot.context_used = last.get("inputTokens").and_then(Value::as_u64);
    // Token counters are cumulative for the session; the context bar intentionally remains the
    // most recent model request. Those are separate concepts in the official protocol.
    snapshot.input_tokens = total.get("inputTokens").and_then(Value::as_u64);
    snapshot.cached_input_tokens = total.get("cachedInputTokens").and_then(Value::as_u64);
    snapshot.output_tokens = total.get("outputTokens").and_then(Value::as_u64);
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
    snapshot.context_live = true;
}

#[cfg(test)]
mod tests {
    use super::{
        RELAY_MAX_BYTES, RELAY_MAX_MESSAGES, RelayQueue, apply_message, apply_runtime_params,
        relay_protocol,
    };
    use crate::state::StatusSnapshot;
    use serde_json::{Value, json};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, RwLock, mpsc};
    use std::thread;
    use std::time::Duration;
    use tungstenite::{Message, WebSocket, accept, client};

    fn websocket_pair() -> (WebSocket<TcpStream>, WebSocket<TcpStream>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            sender.send(accept(stream).unwrap()).unwrap();
        });
        let stream = TcpStream::connect(address).unwrap();
        let (client_socket, _) = client(format!("ws://{address}"), stream).unwrap();
        let server_socket = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        (client_socket, server_socket)
    }

    #[test]
    fn live_relay_forwards_each_rpc_once_and_observes_token_events() {
        let (mut tui, downstream) = websocket_pair();
        let (upstream, mut app_server) = websocket_pair();
        tui.get_mut()
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        app_server
            .get_mut()
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();

        let snapshot = Arc::new(RwLock::new(StatusSnapshot::default()));
        let relay_snapshot = Arc::clone(&snapshot);
        let relay = thread::spawn(move || relay_protocol(downstream, upstream, relay_snapshot));

        let request = json!({"id": 30, "method": "turn/start", "params": {
            "threadId": "thr_1", "model": "gpt-live", "effort": "high",
            "cwd": "/tmp/project", "approvalPolicy": "never"
        }});
        tui.send(Message::text(request.to_string())).unwrap();
        assert_eq!(
            app_server.read().unwrap().to_text().unwrap(),
            request.to_string()
        );

        let notification = json!({"method": "thread/tokenUsage/updated", "params": {
            "tokenUsage": {"modelContextWindow": 200000, "last": {
                "inputTokens": 42000, "cachedInputTokens": 12000, "outputTokens": 3000
            }}
        }});
        app_server
            .send(Message::text(notification.to_string()))
            .unwrap();
        assert_eq!(
            tui.read().unwrap().to_text().unwrap(),
            notification.to_string()
        );

        let state = snapshot.read().unwrap();
        assert_eq!(state.model.as_deref(), Some("gpt-live"));
        assert_eq!(state.context_percent, Some(21));
        assert_eq!(state.input_tokens, Some(42000));
        assert_eq!(state.output_tokens, Some(3000));
        drop(state);

        tui.close(None).unwrap();
        relay.join().unwrap();
    }

    #[test]
    fn live_relay_preserves_order_during_a_bidirectional_burst() {
        const FRAMES: usize = 512;
        let (mut tui, downstream) = websocket_pair();
        let (upstream, mut app_server) = websocket_pair();
        tui.get_mut()
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        app_server
            .get_mut()
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let snapshot = Arc::new(RwLock::new(StatusSnapshot::default()));
        let relay = thread::spawn(move || relay_protocol(downstream, upstream, snapshot));

        for id in 0..FRAMES {
            tui.write(Message::text(format!(
                r#"{{"id":{id},"method":"test/echo","params":{{"value":{id}}}}}"#
            )))
            .unwrap();
        }
        tui.flush().unwrap();
        for id in 0..FRAMES {
            let message = app_server.read().unwrap();
            let value: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
            assert_eq!(value["id"], id);
            app_server
                .write(Message::text(format!(r#"{{"id":{id},"result":{id}}}"#)))
                .unwrap();
        }
        app_server.flush().unwrap();
        for id in 0..FRAMES {
            let message = tui.read().unwrap();
            let value: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
            assert_eq!(value["id"], id);
            assert_eq!(value["result"], id);
        }

        tui.close(None).unwrap();
        relay.join().unwrap();
    }

    #[test]
    fn relay_queue_tracks_bytes_and_preserves_order() {
        let mut queue = RelayQueue::default();
        queue.push(Message::text("first"));
        queue.push(Message::binary(vec![1, 2, 3]));
        assert_eq!(queue.bytes, 8);
        assert_eq!(queue.pop(), Some(Message::text("first")));
        assert_eq!(queue.bytes, 3);
        assert_eq!(queue.pop(), Some(Message::binary(vec![1, 2, 3])));
        assert_eq!(queue.bytes, 0);
    }

    #[test]
    fn relay_queue_applies_message_and_byte_backpressure() {
        let mut by_count = RelayQueue::default();
        for _ in 0..RELAY_MAX_MESSAGES {
            by_count.push(Message::text("x"));
        }
        assert!(!by_count.can_read());

        let mut by_bytes = RelayQueue::default();
        by_bytes.push(Message::binary(vec![0; RELAY_MAX_BYTES]));
        assert!(!by_bytes.can_read());
    }

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
                "total": {"totalTokens": 90000, "inputTokens": 70000, "cachedInputTokens": 50000, "outputTokens": 20000},
                "last": {"totalTokens": 50000, "inputTokens": 40000, "cachedInputTokens": 30000, "outputTokens": 10000}
            }}}),
            &mut snapshot,
        );
        assert_eq!(snapshot.context_percent, Some(20));
        assert_eq!(snapshot.context_used, Some(40000));
        assert_eq!(snapshot.input_tokens, Some(70000));
        assert_eq!(snapshot.output_tokens, Some(20000));
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

    #[test]
    fn live_events_drive_work_tools_plans_compactions_and_modern_agents() {
        let mut snapshot = StatusSnapshot::default();
        apply_message(
            &json!({"method": "turn/plan/updated", "params": {"plan": [
                {"step": "inspect", "status": "completed"},
                {"step": "fix", "status": "inProgress"}
            ]}}),
            &mut snapshot,
        );
        apply_message(
            &json!({"method": "item/started", "params": {"threadId": "root", "item": {
                "type": "commandExecution", "id": "cmd", "command": "cargo test"
            }}}),
            &mut snapshot,
        );
        assert_eq!(snapshot.work.as_deref(), Some("running · exec"));
        apply_message(
            &json!({"method": "item/completed", "params": {"threadId": "root", "item": {
                "type": "commandExecution", "id": "cmd", "status": "completed"
            }}}),
            &mut snapshot,
        );
        apply_message(
            &json!({"method": "item/completed", "params": {"threadId": "root", "item": {
                "type": "contextCompaction", "id": "compact"
            }}}),
            &mut snapshot,
        );
        apply_message(
            &json!({"method": "item/completed", "params": {"threadId": "root", "item": {
                "type": "collabToolCall", "id": "agent-call", "tool": "spawnAgent",
                "newThreadId": "child-1", "prompt": "Review the relay",
                "agentStatus": {"status": "running", "name": "reviewer"}
            }}}),
            &mut snapshot,
        );

        assert_eq!(snapshot.plan_completed, Some(1));
        assert_eq!(snapshot.plan_total, Some(2));
        assert_eq!(snapshot.tools[0].name, "agent");
        assert!(snapshot.tools.iter().any(|tool| tool.name == "exec"));
        assert_eq!(snapshot.compactions, Some(1));
        assert_eq!(snapshot.agents[0].kind, "reviewer");
        assert_eq!(
            snapshot.agents[0].prompt.as_deref(),
            Some("Review the relay")
        );
    }

    #[test]
    fn stored_thread_reads_enrich_agent_names_and_progress() {
        let mut snapshot = StatusSnapshot {
            session_id: Some("root".into()),
            ..StatusSnapshot::default()
        };
        apply_message(
            &json!({"id": 100, "result": {"thread": {
                "id": "root", "parentThreadId": null, "turns": [{"items": [{
                    "type": "subAgentActivity", "kind": "started",
                    "agentThreadId": "child-1", "agentPath": "/root/architecture"
                }]}]
            }}}),
            &mut snapshot,
        );
        assert_eq!(snapshot.agents[0].kind, "architecture");

        apply_message(
            &json!({"id": 101, "result": {"thread": {
                "id": "child-1", "parentThreadId": "root",
                "source": {"subAgent": {"thread_spawn": {"agent_path": "/root/architecture"}}},
                "turns": [{"status": "inProgress", "items": [
                    {"type": "agentMessage", "text": "Comparing DESIGN.md with the renderer"}
                ]}]
            }}}),
            &mut snapshot,
        );
        assert!(snapshot.agents[0].active);
        assert_eq!(
            snapshot.agents[0].message.as_deref(),
            Some("Comparing DESIGN.md with the renderer")
        );
    }

    #[test]
    fn paged_turn_reads_update_agents_without_loading_full_history() {
        let mut snapshot = StatusSnapshot {
            session_id: Some("root".into()),
            ..StatusSnapshot::default()
        };
        apply_message(
            &json!({"id": "codexline:root:root", "result": {"data": [{
                "status": "inProgress", "items": [{
                    "type": "subAgentActivity", "kind": "started",
                    "agentThreadId": "child-1", "agentPath": "/root/quality"
                }]
            }]}}),
            &mut snapshot,
        );
        apply_message(
            &json!({"id": "codexline:agent:child-1:0", "result": {"data": [{
                "status": "inProgress", "items": [{
                    "type": "agentMessage", "text": "Running the test matrix"
                }]
            }]}}),
            &mut snapshot,
        );
        assert_eq!(snapshot.agents[0].kind, "quality");
        assert_eq!(
            snapshot.agents[0].message.as_deref(),
            Some("Running the test matrix")
        );
    }

    #[test]
    fn live_turn_params_refresh_model_and_permissions() {
        let mut snapshot = StatusSnapshot::default();
        apply_runtime_params(
            &json!({
                "model": "gpt-live",
                "effort": "xhigh",
                "cwd": "/workspace/live",
                "approvalPolicy": "onRequest",
                "approvalsReviewer": "auto_review",
                "sandboxPolicy": {"type": "workspaceWrite"}
            }),
            &mut snapshot,
        );
        assert_eq!(snapshot.model.as_deref(), Some("gpt-live"));
        assert!(snapshot.model_live);
        assert_eq!(snapshot.sandbox.as_deref(), Some("workspaceWrite"));
        assert_eq!(snapshot.approvals_reviewer.as_deref(), Some("auto_review"));
        assert!(snapshot.settings_live);
    }
}
