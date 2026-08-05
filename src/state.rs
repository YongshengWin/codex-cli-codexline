use std::time::Instant;

#[derive(Debug, Clone)]
pub struct AgentActivity {
    pub kind: String,
    pub started: Instant,
}

#[derive(Debug, Clone)]
pub struct ToolCount {
    pub name: String,
    pub count: u16,
}

#[derive(Debug, Clone)]
pub struct RateLimitWindow {
    pub used_percent: u8,
    pub window_minutes: Option<u64>,
    pub resets_at: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct StatusSnapshot {
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub work: Option<String>,
    pub context_percent: Option<u8>,
    pub context_used: Option<u64>,
    pub context_window: Option<u64>,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub rate_limits: Vec<RateLimitWindow>,
    pub reset_credits: Option<u16>,
    pub cwd: Option<String>,
    pub project_root: Option<String>,
    pub git_branch: Option<String>,
    pub git_dirty: Option<bool>,
    pub git_staged: Option<u16>,
    pub git_modified: Option<u16>,
    pub git_ahead: Option<u16>,
    pub git_behind: Option<u16>,
    pub worktree: Option<String>,
    pub linked_worktree: Option<bool>,
    pub agents_active: Option<u16>,
    pub agents_total: Option<u16>,
    pub agents: Vec<AgentActivity>,
    pub tools: Vec<ToolCount>,
    pub plan_completed: Option<u16>,
    pub plan_total: Option<u16>,
    pub compactions: Option<u16>,
    pub safety: Option<String>,
    pub session_id: Option<String>,
    pub events_active: bool,
    pub app_server_active: bool,
}

impl StatusSnapshot {
    pub fn showcase() -> Self {
        Self {
            model: Some("gpt-5.6-sol".into()),
            reasoning: Some("high".into()),
            work: Some("exec 8s".into()),
            context_percent: Some(42),
            context_used: Some(84_000),
            context_window: Some(200_000),
            input_tokens: Some(72_400),
            cached_input_tokens: Some(51_200),
            output_tokens: Some(8_600),
            rate_limits: vec![
                RateLimitWindow {
                    used_percent: 34,
                    window_minutes: Some(300),
                    resets_at: None,
                },
                RateLimitWindow {
                    used_percent: 65,
                    window_minutes: Some(10_080),
                    resets_at: None,
                },
            ],
            reset_credits: Some(1),
            cwd: Some("~/pro/codex-cli-statusline".into()),
            project_root: Some("~/pro/codex-cli-statusline".into()),
            git_branch: Some("feat/statusline".into()),
            git_dirty: Some(true),
            git_staged: Some(1),
            git_modified: Some(3),
            git_ahead: Some(2),
            git_behind: Some(0),
            worktree: Some("codexline-agent-2".into()),
            linked_worktree: Some(true),
            agents_active: Some(2),
            agents_total: Some(3),
            agents: vec![
                AgentActivity {
                    kind: "explore".into(),
                    started: Instant::now(),
                },
                AgentActivity {
                    kind: "worker".into(),
                    started: Instant::now(),
                },
            ],
            tools: vec![
                ToolCount {
                    name: "exec".into(),
                    count: 3,
                },
                ToolCount {
                    name: "patch".into(),
                    count: 2,
                },
            ],
            plan_completed: Some(2),
            plan_total: Some(4),
            compactions: Some(1),
            safety: Some("workspace · ask".into()),
            session_id: Some("thr_showcase".into()),
            events_active: true,
            app_server_active: true,
        }
    }
}
