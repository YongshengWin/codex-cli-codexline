#[derive(Debug, Clone, Default)]
pub struct StatusSnapshot {
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub work: Option<String>,
    pub context_percent: Option<u8>,
    pub git_branch: Option<String>,
    pub git_dirty: Option<bool>,
    pub agents_active: Option<u16>,
    pub agents_total: Option<u16>,
    pub plan_completed: Option<u16>,
    pub plan_total: Option<u16>,
    pub safety: Option<String>,
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
            agents_active: Some(2),
            agents_total: Some(3),
            plan_completed: Some(2),
            plan_total: Some(4),
            safety: Some("workspace · ask".into()),
        }
    }
}
