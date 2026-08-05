use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, RwLock};
use std::thread;

use anyhow::{Context, Result};
use serde_json::{Value, json};

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
    snapshot.context_used = last.get("totalTokens").and_then(Value::as_u64);
    snapshot.input_tokens = last.get("inputTokens").and_then(Value::as_u64);
    snapshot.cached_input_tokens = last.get("cachedInputTokens").and_then(Value::as_u64);
    snapshot.output_tokens = last.get("outputTokens").and_then(Value::as_u64);
    snapshot.context_percent = match (snapshot.context_used, snapshot.context_window) {
        (Some(used), Some(window)) if window > 0 => {
            Some(((used.saturating_mul(100) / window).min(100)) as u8)
        }
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
        assert_eq!(snapshot.context_percent, Some(25));
        assert_eq!(snapshot.output_tokens, Some(10000));
    }
}
