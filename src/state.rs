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

#[derive(Debug, Clone, Default)]
pub struct StatusSnapshot {
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub work: Option<String>,
    pub context_percent: Option<u8>,
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
}

impl StatusSnapshot {
    pub fn showcase() -> Self {
        Self {
            model: Some("gpt-5.6-sol".into()),
            reasoning: Some("high".into()),
            work: Some("exec 8s".into()),
            context_percent: Some(42),
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
        }
    }
}
