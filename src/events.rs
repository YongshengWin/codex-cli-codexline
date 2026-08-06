use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Read};
use std::net::{SocketAddr, UdpSocket};
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Instant;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::state::{AgentActivity, StatusSnapshot, ToolCount};

const MAX_HOOK_INPUT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_EVENT_BYTES: usize = 60 * 1024;

pub struct EventServer {
    endpoint: SocketAddr,
    token: String,
}

impl EventServer {
    pub fn start(snapshot: Arc<RwLock<StatusSnapshot>>) -> Result<Self> {
        let socket = UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .context("could not bind the local hook event socket")?;
        let endpoint = socket.local_addr()?;
        let random_file = tempfile::Builder::new()
            .prefix("codexline-event-")
            .tempfile()
            .context("could not generate an event token")?;
        let token = random_file
            .path()
            .file_name()
            .context("event token has no filename")?
            .to_string_lossy()
            .into_owned();
        drop(random_file);
        let expected = token.clone();
        thread::spawn(move || listen(socket, expected, snapshot));
        Ok(Self { endpoint, token })
    }

    pub fn endpoint(&self) -> String {
        self.endpoint.to_string()
    }

    pub fn token(&self) -> &str {
        &self.token
    }
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    token: String,
    event: Value,
}

pub fn emit_hook() -> Result<i32> {
    // A status source must never make an official Codex hook fail. Oversized, partial, or
    // version-skewed events are deliberately dropped and the TUI continues normally.
    let _ = try_emit_hook();
    Ok(0)
}

fn try_emit_hook() -> Result<()> {
    let Some(endpoint) = std::env::var_os("CODEXLINE_EVENT_ENDPOINT") else {
        return Ok(());
    };
    let Some(token) = std::env::var_os("CODEXLINE_EVENT_TOKEN") else {
        return Ok(());
    };
    let endpoint = SocketAddr::from_str(&endpoint.to_string_lossy())
        .context("invalid CODEXLINE_EVENT_ENDPOINT")?;
    let mut bytes = Vec::new();
    io::stdin()
        .take(MAX_HOOK_INPUT_BYTES)
        .read_to_end(&mut bytes)
        .context("could not read hook input")?;
    let event: Value = serde_json::from_slice(&bytes).context("invalid hook JSON")?;
    let event = compact_hook_event(&event);
    let message = serde_json::to_vec(&Envelope {
        token: token.to_string_lossy().into_owned(),
        event,
    })?;
    let socket = UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    socket.send_to(&message, endpoint)?;
    Ok(())
}

fn compact_hook_event(event: &Value) -> Value {
    const STRING_FIELDS: &[&str] = &[
        "hook_event_name",
        "session_id",
        "model",
        "reasoning_effort",
        "model_reasoning_effort",
        "cwd",
        "permission_mode",
        "sandbox_mode",
        "approval_policy",
        "approvals_reviewer",
        "tool_name",
        "agent_id",
        "agent_type",
        "prompt",
        "last_assistant_message",
    ];
    let mut compact = Map::new();
    for key in STRING_FIELDS {
        if let Some(value) = event.get(*key).and_then(Value::as_str) {
            compact.insert(
                (*key).into(),
                Value::String(value.chars().take(512).collect()),
            );
        }
    }
    if let Some(input) = event.get("tool_input") {
        let mut tool_input = Map::new();
        if let Some(task_name) = input.get("task_name").and_then(Value::as_str) {
            tool_input.insert(
                "task_name".into(),
                Value::String(task_name.chars().take(80).collect()),
            );
        }
        if let Some(plan) = input.get("plan").and_then(Value::as_array) {
            tool_input.insert(
                "plan".into(),
                Value::Array(
                    plan.iter()
                        .take(256)
                        .filter_map(|step| step.get("status").and_then(Value::as_str))
                        .map(|status| {
                            Value::Object(Map::from_iter([(
                                "status".into(),
                                Value::String(status.chars().take(32).collect()),
                            )]))
                        })
                        .collect(),
                ),
            );
        }
        if !tool_input.is_empty() {
            compact.insert("tool_input".into(), Value::Object(tool_input));
        }
    }
    Value::Object(compact)
}

#[derive(Default)]
struct RuntimeState {
    agents: HashMap<String, AgentActivity>,
    seen_agents: HashSet<String>,
    tools: Vec<ToolCount>,
    compactions: u16,
    pending_agent_names: VecDeque<String>,
}

fn listen(socket: UdpSocket, expected_token: String, snapshot: Arc<RwLock<StatusSnapshot>>) {
    let mut buffer = vec![0_u8; MAX_EVENT_BYTES + 4096];
    let mut runtime = RuntimeState::default();
    while let Ok((count, source)) = socket.recv_from(&mut buffer) {
        if !source.ip().is_loopback() {
            continue;
        }
        let Ok(envelope) = serde_json::from_slice::<Envelope>(&buffer[..count]) else {
            continue;
        };
        if envelope.token != expected_token {
            continue;
        }
        let Ok(mut current) = snapshot.write() else {
            break;
        };
        apply_event(&envelope.event, &mut runtime, &mut current);
    }
}

fn apply_event(event: &Value, runtime: &mut RuntimeState, snapshot: &mut StatusSnapshot) {
    snapshot.events_active = true;
    if let Some(value) = string(event, "session_id") {
        snapshot.session_id = Some(value.into());
    }
    if let Some(value) = string(event, "model") {
        snapshot.model = Some(value.into());
        snapshot.model_live = true;
    }
    if let Some(value) =
        string(event, "reasoning_effort").or_else(|| string(event, "model_reasoning_effort"))
    {
        snapshot.reasoning = Some(value.into());
        snapshot.model_live = true;
    }
    if let Some(value) = string(event, "cwd") {
        snapshot.cwd = Some(value.into());
    }
    let mut settings_updated = false;
    if let Some(value) = string(event, "permission_mode") {
        snapshot.permission_mode = Some(compact_permission(value).into());
        settings_updated = true;
    }
    if let Some(value) = string(event, "sandbox_mode") {
        snapshot.sandbox = Some(value.into());
        settings_updated = true;
    }
    if let Some(value) = string(event, "approval_policy") {
        snapshot.approval_policy = Some(value.into());
        settings_updated = true;
    }
    if let Some(value) = string(event, "approvals_reviewer") {
        snapshot.approvals_reviewer = Some(value.into());
        settings_updated = true;
    }
    if settings_updated {
        snapshot.settings_live = true;
    }

    match string(event, "hook_event_name").unwrap_or_default() {
        "SessionStart" => snapshot.work = Some("ready".into()),
        "UserPromptSubmit" => {
            snapshot.work = Some("thinking".into());
            if !snapshot.live_session_active {
                clear_unobserved_context(snapshot);
            }
        }
        "PreToolUse" => {
            let tool = string(event, "tool_name").unwrap_or("tool");
            snapshot.work = Some(format!("running · {}", compact_tool(tool)));
            if tool == "update_plan" {
                update_plan(event, snapshot);
            }
            if is_spawn_agent_tool(tool)
                && let Some(task_name) = event
                    .pointer("/tool_input/task_name")
                    .and_then(Value::as_str)
            {
                runtime
                    .pending_agent_names
                    .push_back(compact_agent(task_name));
            }
        }
        "PermissionRequest" => {
            let tool = string(event, "tool_name").unwrap_or("tool");
            snapshot.work = Some(format!("approval · {}", compact_tool(tool)));
        }
        "PostToolUse" => {
            let tool = compact_tool(string(event, "tool_name").unwrap_or("tool"));
            record_tool(&tool, runtime);
            snapshot.tools = runtime.tools.clone();
            snapshot.work = Some("working".into());
        }
        "PreCompact" => snapshot.work = Some("compacting".into()),
        "PostCompact" => {
            runtime.compactions = runtime.compactions.saturating_add(1);
            snapshot.compactions = Some(runtime.compactions);
            snapshot.work = Some("working".into());
        }
        "SubagentStart" => {
            if let Some(id) = string(event, "agent_id") {
                let reported = string(event, "agent_type").unwrap_or("agent");
                let pending = runtime.pending_agent_names.pop_front();
                let kind = pending.unwrap_or_else(|| {
                    if matches!(reported, "default" | "agent") {
                        compact_agent(id.rsplit(['/', ':']).next().unwrap_or(id))
                    } else {
                        compact_agent(reported)
                    }
                });
                runtime.seen_agents.insert(id.into());
                runtime.agents.insert(
                    id.into(),
                    AgentActivity {
                        id: id.into(),
                        kind,
                        started: Instant::now(),
                        prompt: string(event, "prompt").map(sanitize_detail),
                        message: None,
                        active: true,
                    },
                );
                sync_agents(runtime, snapshot);
            }
        }
        "SubagentStop" => {
            if let Some(id) = string(event, "agent_id") {
                if let Some(agent) = runtime.agents.get_mut(id) {
                    agent.active = false;
                    agent.message = string(event, "last_assistant_message").map(sanitize_detail);
                }
                sync_agents(runtime, snapshot);
            }
        }
        "Stop" => snapshot.work = Some("ready".into()),
        "SessionEnd" => snapshot.work = Some("ended".into()),
        _ => {}
    }
}

fn clear_unobserved_context(snapshot: &mut StatusSnapshot) {
    snapshot.context_percent = None;
    snapshot.context_used = None;
    snapshot.context_window = None;
    snapshot.input_tokens = None;
    snapshot.cached_input_tokens = None;
    snapshot.output_tokens = None;
    snapshot.context_live = false;
}

fn string<'a>(event: &'a Value, key: &str) -> Option<&'a str> {
    event.get(key)?.as_str()
}

fn update_plan(event: &Value, snapshot: &mut StatusSnapshot) {
    let Some(plan) = event
        .get("tool_input")
        .and_then(|input| input.get("plan"))
        .and_then(Value::as_array)
    else {
        return;
    };
    let completed = plan
        .iter()
        .filter(|step| step.get("status").and_then(Value::as_str) == Some("completed"))
        .count();
    snapshot.plan_completed = Some(completed.min(usize::from(u16::MAX)) as u16);
    snapshot.plan_total = Some(plan.len().min(usize::from(u16::MAX)) as u16);
}

fn record_tool(tool: &str, runtime: &mut RuntimeState) {
    let count = runtime
        .tools
        .iter()
        .find(|entry| entry.name == tool)
        .map_or(1, |entry| entry.count.saturating_add(1));
    runtime.tools.retain(|entry| entry.name != tool);
    runtime.tools.insert(
        0,
        ToolCount {
            name: tool.into(),
            count,
        },
    );
    runtime.tools.truncate(4);
}

fn sync_agents(runtime: &RuntimeState, snapshot: &mut StatusSnapshot) {
    snapshot.agents = runtime.agents.values().cloned().collect();
    snapshot
        .agents
        .sort_by(|left, right| left.kind.cmp(&right.kind));
    snapshot.agents_active = Some(
        snapshot
            .agents
            .iter()
            .filter(|agent| agent.active)
            .count()
            .min(usize::from(u16::MAX)) as u16,
    );
    snapshot.agents_total = Some(runtime.seen_agents.len().min(usize::from(u16::MAX)) as u16);
}

fn compact_tool(value: &str) -> String {
    if is_spawn_agent_tool(value) {
        return "spawn agent".into();
    }
    let value = match value {
        "Bash" | "exec_command" => "exec",
        "apply_patch" => "patch",
        "Agent" => "agent",
        other => other.rsplit("__").next().unwrap_or(other),
    };
    sanitize_label(value, 20)
}

fn is_spawn_agent_tool(value: &str) -> bool {
    value == "Agent" || value == "spawn_agent" || value.ends_with("spawn_agent")
}

fn compact_agent(value: &str) -> String {
    sanitize_label(value, 16)
}

fn compact_permission(value: &str) -> &str {
    match value {
        "acceptEdits" => "accept edits",
        "dontAsk" => "don't ask",
        "bypassPermissions" => "bypass permissions",
        "autoReview" | "auto_review" | "guardian_subagent" => "approve for me",
        other => other,
    }
}

fn sanitize_label(value: &str, max: usize) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(max)
        .collect()
}

fn sanitize_detail(value: &str) -> String {
    sanitize_label(value, 160)
}

#[cfg(test)]
mod tests {
    use super::{RuntimeState, apply_event};
    use crate::state::StatusSnapshot;
    use serde_json::json;

    #[test]
    fn hook_events_drive_tools_agents_and_plan() {
        let mut runtime = RuntimeState::default();
        let mut snapshot = StatusSnapshot::default();
        apply_event(
            &json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "update_plan",
                "tool_input": {"plan": [
                    {"status": "completed"}, {"status": "in_progress"}
                ]}
            }),
            &mut runtime,
            &mut snapshot,
        );
        assert_eq!(snapshot.plan_completed, Some(1));
        assert_eq!(snapshot.plan_total, Some(2));

        apply_event(
            &json!({"hook_event_name": "PostToolUse", "tool_name": "Bash"}),
            &mut runtime,
            &mut snapshot,
        );
        assert_eq!(snapshot.tools[0].name, "exec");

        apply_event(
            &json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Agent",
                "tool_input": {"task_name": "runtime_flow"}
            }),
            &mut runtime,
            &mut snapshot,
        );

        apply_event(
            &json!({
                "hook_event_name": "SubagentStart",
                "agent_id": "agent-1",
                "agent_type": "default"
            }),
            &mut runtime,
            &mut snapshot,
        );
        assert_eq!(snapshot.agents_active, Some(1));
        assert_eq!(snapshot.agents_total, Some(1));
        assert_eq!(snapshot.agents[0].kind, "runtime_flow");
    }

    #[test]
    fn compact_hook_event_drops_large_tool_output_but_keeps_agent_name() {
        let event = json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "collaboration.spawn_agent",
            "tool_input": {"task_name": "quality_delivery", "message": "secret prompt"},
            "tool_response": "x".repeat(100_000)
        });
        let compact = super::compact_hook_event(&event);
        assert_eq!(
            compact
                .pointer("/tool_input/task_name")
                .and_then(|value| value.as_str()),
            Some("quality_delivery")
        );
        assert!(compact.get("tool_response").is_none());
        assert!(serde_json::to_vec(&compact).unwrap().len() < 1024);
    }

    #[test]
    fn hook_refreshes_runtime_settings_and_invalidates_unobserved_context() {
        let mut runtime = RuntimeState::default();
        let mut snapshot = StatusSnapshot {
            model: Some("old-model".into()),
            context_percent: Some(0),
            context_used: Some(0),
            ..StatusSnapshot::default()
        };
        apply_event(
            &json!({
                "hook_event_name": "UserPromptSubmit",
                "model": "new-model",
                "reasoning_effort": "high",
                "cwd": "/tmp/project",
                "permission_mode": "autoReview"
            }),
            &mut runtime,
            &mut snapshot,
        );
        assert_eq!(snapshot.model.as_deref(), Some("new-model"));
        assert!(snapshot.model_live);
        assert_eq!(snapshot.reasoning.as_deref(), Some("high"));
        assert_eq!(snapshot.cwd.as_deref(), Some("/tmp/project"));
        assert_eq!(snapshot.permission_mode.as_deref(), Some("approve for me"));
        assert!(snapshot.settings_live);
        assert_eq!(snapshot.context_percent, None);
    }
}
