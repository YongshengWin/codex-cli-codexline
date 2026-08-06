use std::io::{self, Write};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use anyhow::Result;
use crossterm::terminal;
use unicode_width::UnicodeWidthStr;

use crate::config::{DisplayConfig, Glyphs, Segment, Theme};
use crate::state::{LiveProxyStatus, StatusSnapshot};

pub struct TerminalGuard {
    child_rows: std::cell::Cell<u16>,
    reserved_rows: std::cell::Cell<u16>,
}

impl TerminalGuard {
    pub fn enter(child_rows: u16, reserved_rows: u16) -> Result<Self> {
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout().lock();
        write!(stdout, "\x1b[1;{child_rows}r\x1b[?25h")?;
        stdout.flush()?;
        Ok(Self {
            child_rows: std::cell::Cell::new(child_rows),
            reserved_rows: std::cell::Cell::new(reserved_rows),
        })
    }

    pub fn update_reserved_rows(&self, child_rows: u16, reserved_rows: u16) -> Result<()> {
        let old_first = self.child_rows.get().saturating_add(1);
        let old_last = old_first
            .saturating_add(self.reserved_rows.get())
            .saturating_sub(1);
        self.child_rows.set(child_rows);
        self.reserved_rows.set(reserved_rows);
        let mut stdout = io::stdout().lock();
        write!(stdout, "\x1b7\x1b[r")?;
        for row in old_first..=old_last {
            write!(stdout, "\x1b[{row};1H\x1b[2K")?;
        }
        write!(stdout, "\x1b[1;{child_rows}r\x1b8")?;
        stdout.flush()?;
        Ok(())
    }

    /// Reassert the child scroll region after the child TUI has emitted a full-screen reset.
    /// The cursor is preserved and no status cells are touched here.
    pub fn prepare_status_draw(&self, output: &mut impl Write) -> Result<()> {
        let child_rows = self.child_rows.get();
        write!(output, "\x1b7\x1b[1;{child_rows}r\x1b8")?;
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout().lock();
        let first_status_row = self.child_rows.get().saturating_add(1);
        let last_status_row = first_status_row
            .saturating_add(self.reserved_rows.get())
            .saturating_sub(1);
        let _ = write!(stdout, "\x1b7");
        for row in first_status_row..=last_status_row {
            let _ = write!(stdout, "\x1b[{row};1H\x1b[2K");
        }
        let _ = write!(stdout, "\x1b8\x1b[r\x1b[0m\x1b[?25h");
        let _ = stdout.flush();
        let _ = terminal::disable_raw_mode();
    }
}

pub struct StatusRenderer {
    display: DisplayConfig,
    snapshot: Arc<RwLock<StatusSnapshot>>,
    started: Instant,
    agent_panel: Arc<Mutex<AgentPanelState>>,
    previous_rows: Vec<Vec<u8>>,
    previous_first_row: Option<u16>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum AgentPanelMode {
    #[default]
    Passive,
    List,
    Detail,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentPanelState {
    mode: AgentPanelMode,
    selected: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentInputAction {
    Forward,
    Consumed,
    OpenOfficialPicker,
}

impl AgentPanelState {
    /// A short ambiguity window lets terminal escape sequences survive split stdin reads.
    pub fn should_buffer_input(&self, input: &[u8]) -> bool {
        if input.is_empty() || input.len() > 64 || input[0] != 0x1b {
            return false;
        }
        if input.len() == 1 || input == b"\x1bO" {
            return true;
        }
        input.starts_with(b"\x1b[") && !input[2..].iter().any(|byte| (0x40..=0x7e).contains(byte))
    }

    /// Routes input between Codex, the local inspector, and Codex's official `/agent` picker.
    pub fn handle_input(&mut self, input: &[u8], agent_count: usize) -> AgentInputAction {
        if self.mode == AgentPanelMode::Passive {
            if contains_inspector_key(input, InspectorKey::Toggle) && agent_count > 0 {
                self.mode = AgentPanelMode::List;
                self.selected = self.selected.min(agent_count.saturating_sub(1));
                return AgentInputAction::Consumed;
            }
            return AgentInputAction::Forward;
        }
        let mut offset = 0;
        while offset < input.len() {
            let (key, consumed) = decode_inspector_key(&input[offset..]);
            match key {
                InspectorKey::Up => self.selected = self.selected.saturating_sub(1),
                InspectorKey::Down => {
                    self.selected = (self.selected + 1).min(agent_count.saturating_sub(1));
                }
                InspectorKey::Right if agent_count > 0 => {
                    self.mode = AgentPanelMode::Detail;
                }
                InspectorKey::Enter if agent_count > 0 => {
                    self.mode = AgentPanelMode::Passive;
                    return AgentInputAction::OpenOfficialPicker;
                }
                InspectorKey::Left | InspectorKey::Escape => self.back(),
                InspectorKey::Toggle => self.mode = AgentPanelMode::Passive,
                InspectorKey::Enter | InspectorKey::Right | InspectorKey::Other => {}
            }
            // Focused inspector owns all input so partial/unknown sequences cannot leak into the
            // Codex prompt. The decoder always consumes at least one byte.
            offset += consumed;
        }
        AgentInputAction::Consumed
    }

    fn back(&mut self) {
        self.mode = match self.mode {
            AgentPanelMode::Detail => AgentPanelMode::List,
            AgentPanelMode::List | AgentPanelMode::Passive => AgentPanelMode::Passive,
        };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InspectorKey {
    Toggle,
    Up,
    Down,
    Right,
    Left,
    Enter,
    Escape,
    Other,
}

fn contains_inspector_key(mut input: &[u8], wanted: InspectorKey) -> bool {
    while !input.is_empty() {
        let (key, consumed) = decode_inspector_key(input);
        if key == wanted {
            return true;
        }
        input = &input[consumed..];
    }
    false
}

fn decode_inspector_key(input: &[u8]) -> (InspectorKey, usize) {
    if input.is_empty() {
        return (InspectorKey::Other, 1);
    }
    match input[0] {
        b'\r' | b'\n' => return (InspectorKey::Enter, 1),
        b'j' => return (InspectorKey::Down, 1),
        b'k' => return (InspectorKey::Up, 1),
        0x1b if input.len() == 1 => return (InspectorKey::Escape, 1),
        0x1b => {}
        _ => return (InspectorKey::Other, 1),
    }

    // F2 encodings used by macOS Terminal/xterm (SS3), vt100, and Kitty/CSI keyboard modes.
    for sequence in [b"\x1bOQ".as_slice(), b"\x1b[12~", b"\x1b[1;1Q"] {
        if input.starts_with(sequence) {
            return (InspectorKey::Toggle, sequence.len());
        }
    }
    for (sequence, key) in [
        (b"\x1bOA".as_slice(), InspectorKey::Up),
        (b"\x1bOB".as_slice(), InspectorKey::Down),
        (b"\x1bOC".as_slice(), InspectorKey::Right),
        (b"\x1bOD".as_slice(), InspectorKey::Left),
    ] {
        if input.starts_with(sequence) {
            return (key, sequence.len());
        }
    }
    if input.starts_with(b"\x1b[") {
        if let Some(final_offset) = input[2..]
            .iter()
            .position(|byte| (0x40..=0x7e).contains(byte))
            .map(|offset| offset + 2)
        {
            let final_byte = input[final_offset];
            let codepoint = std::str::from_utf8(&input[2..final_offset])
                .ok()
                .and_then(|parameters| parameters.split([';', ':']).next())
                .and_then(|value| value.parse::<u32>().ok());
            let key = match (final_byte, codepoint) {
                // Kitty's progressive keyboard protocol maps functional keys into its private
                // Unicode range. Some terminals use these forms after enhanced reporting is enabled.
                (b'u', Some(57_377)) => InspectorKey::Toggle, // F2
                (b'u', Some(57_352)) => InspectorKey::Up,
                (b'u', Some(57_353)) => InspectorKey::Down,
                (b'u', Some(57_351)) => InspectorKey::Right,
                (b'u', Some(57_350)) => InspectorKey::Left,
                (b'u', Some(13 | 57_345)) => InspectorKey::Enter,
                (b'u', Some(27 | 57_344)) => InspectorKey::Escape,
                (b'Q', _) => InspectorKey::Toggle,
                (b'A', _) => InspectorKey::Up,
                (b'B', _) => InspectorKey::Down,
                (b'C', _) => InspectorKey::Right,
                (b'D', _) => InspectorKey::Left,
                _ => InspectorKey::Other,
            };
            return (key, final_offset + 1);
        }
    }
    (InspectorKey::Escape, 1)
}

impl StatusRenderer {
    pub fn new(display: DisplayConfig, snapshot: Arc<RwLock<StatusSnapshot>>) -> Self {
        Self {
            display,
            snapshot,
            started: Instant::now(),
            agent_panel: Arc::new(Mutex::new(AgentPanelState::default())),
            previous_rows: Vec::new(),
            previous_first_row: None,
        }
    }

    pub fn agent_panel(&self) -> Arc<Mutex<AgentPanelState>> {
        Arc::clone(&self.agent_panel)
    }

    pub fn required_rows(&self, width: u16) -> u16 {
        self.layouts(width).len().min(usize::from(u16::MAX)) as u16
    }

    /// Child full-screen redraws can erase unchanged HUD rows behind our diff cache.
    pub fn invalidate(&mut self) {
        self.previous_rows.clear();
        self.previous_first_row = None;
    }

    fn layouts(&self, width: u16) -> Vec<Vec<(Segment, String)>> {
        let snapshot = self.snapshot.read().map_or_else(
            |poisoned| poisoned.into_inner().clone(),
            |value| value.clone(),
        );
        let mut layouts = status_layouts(
            width,
            &self.display,
            &snapshot,
            self.started.elapsed().as_secs(),
        );
        let panel = self
            .agent_panel
            .lock()
            .map_or_else(|poisoned| *poisoned.into_inner(), |value| *value);
        layouts.extend(agent_panel_layouts(width, &snapshot, panel));
        layouts
    }

    pub fn draw(&mut self, output: &mut impl Write, width: u16, rows: u16) -> Result<()> {
        let snapshot = self.snapshot.read().map_or_else(
            |poisoned| poisoned.into_inner().clone(),
            |value| value.clone(),
        );
        let mut layouts = self.layouts(width);
        layouts.truncate(rows.saturating_sub(4).max(1) as usize);
        let first_row = rows.saturating_sub(layouts.len() as u16).saturating_add(1);
        let rendered = layouts
            .iter()
            .map(|layout| {
                let mut row = Vec::new();
                write_styled_row(&mut row, layout, width as usize, &self.display, &snapshot)?;
                Ok(row)
            })
            .collect::<Result<Vec<_>>>()?;
        let geometry_changed = self.previous_first_row != Some(first_row)
            || self.previous_rows.len() != rendered.len();
        let mut saved_cursor = false;
        for (offset, row_bytes) in rendered.iter().enumerate() {
            if !geometry_changed && self.previous_rows.get(offset) == Some(row_bytes) {
                continue;
            }
            if !saved_cursor {
                write!(output, "\x1b7")?;
                saved_cursor = true;
            }
            let row = first_row.saturating_add(offset as u16);
            write!(output, "\x1b[{row};1H")?;
            output.write_all(row_bytes)?;
        }
        if saved_cursor {
            write!(output, "\x1b[0m\x1b8")?;
        }
        self.previous_rows = rendered;
        self.previous_first_row = Some(first_row);
        Ok(())
    }
}

fn agent_panel_layouts(
    width: u16,
    snapshot: &StatusSnapshot,
    mut panel: AgentPanelState,
) -> Vec<Vec<(Segment, String)>> {
    if snapshot.agents.is_empty() {
        return Vec::new();
    }
    let mut agents = snapshot.agents.clone();
    agents.sort_by(|left, right| {
        right
            .active
            .cmp(&left.active)
            .then_with(|| left.kind.cmp(&right.kind))
    });
    panel.selected = panel.selected.min(agents.len().saturating_sub(1));
    let active = agents.iter().filter(|agent| agent.active).count();
    let total = usize::from(snapshot.agents_total.unwrap_or(0)).max(agents.len());
    let header = match panel.mode {
        AgentPanelMode::Passive => format!("AGENTS {active}/{total} · F2 inspect agents"),
        AgentPanelMode::List => format!(
            "AGENT INSPECTOR {active}/{} · ↑↓/jk choose · → details · Enter /agent · ←/Esc close",
            total
        ),
        AgentPanelMode::Detail => {
            let agent = &agents[panel.selected];
            format!(
                "{} {} · {} · Esc back",
                if agent.active { "●" } else { "✓" },
                agent.kind,
                compact_duration(agent.started.elapsed().as_secs())
            )
        }
    };
    let mut rows = vec![panel_layout(Segment::Agents, header, width)];
    match panel.mode {
        AgentPanelMode::Passive | AgentPanelMode::List => {
            let start = if panel.mode == AgentPanelMode::List {
                panel.selected.saturating_sub(2)
            } else {
                0
            };
            for (index, agent) in agents.iter().enumerate().skip(start).take(3) {
                let marker = if panel.mode == AgentPanelMode::List && index == panel.selected {
                    "▶"
                } else {
                    " "
                };
                let state = if agent.active { "●" } else { "✓" };
                let detail = agent
                    .message
                    .as_deref()
                    .or(agent.prompt.as_deref())
                    .unwrap_or("working");
                rows.push(panel_layout(
                    Segment::Agents,
                    format!(
                        "{marker} {state} {} {} · {}",
                        agent.kind,
                        compact_duration(agent.started.elapsed().as_secs()),
                        sanitize_dynamic(detail)
                    ),
                    width,
                ));
            }
        }
        AgentPanelMode::Detail => {
            let agent = &agents[panel.selected];
            rows.push(panel_layout(
                Segment::Agents,
                format!(
                    "Goal   {}",
                    agent.prompt.as_deref().unwrap_or("Waiting for agent goal")
                ),
                width,
            ));
            rows.push(panel_layout(
                Segment::Tokens,
                format!(
                    "Latest {}",
                    agent
                        .message
                        .as_deref()
                        .unwrap_or("No activity message yet")
                ),
                width,
            ));
        }
    }
    rows
}

fn panel_layout(segment: Segment, text: String, width: u16) -> Vec<(Segment, String)> {
    let limit = usize::from(width).saturating_sub(2);
    let mut text = sanitize_dynamic(&text);
    while UnicodeWidthStr::width(text.as_str()) > limit {
        text.pop();
    }
    vec![(segment, text)]
}

#[cfg(test)]
pub fn preview_line(width: u16, display: &DisplayConfig) -> String {
    let snapshot = StatusSnapshot::showcase();
    let mut layouts = status_layouts(width, display, &snapshot, 8);
    layouts.extend(agent_panel_layouts(
        width,
        &snapshot,
        AgentPanelState::default(),
    ));
    layouts
        .iter()
        .map(|layout| plain_row(layout, width as usize, display))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn preview_ansi(width: u16, display: &DisplayConfig) -> Result<String> {
    let snapshot = StatusSnapshot::showcase();
    let mut layouts = status_layouts(width, display, &snapshot, 8);
    layouts.extend(agent_panel_layouts(
        width,
        &snapshot,
        AgentPanelState::default(),
    ));
    let mut output = Vec::new();
    for (index, layout) in layouts.iter().enumerate() {
        if index > 0 {
            output.push(b'\n');
        }
        write_styled_row(&mut output, layout, width as usize, display, &snapshot)?;
    }
    Ok(String::from_utf8(output)?)
}

fn status_layouts(
    width: u16,
    display: &DisplayConfig,
    snapshot: &StatusSnapshot,
    elapsed: u64,
) -> Vec<Vec<(Segment, String)>> {
    let spinner = match display.glyphs {
        Glyphs::Ascii => ">",
        Glyphs::Unicode => "●",
    };
    let cwd = workspace_path(snapshot);
    let cwd = sanitize_dynamic(&cwd);
    let segments = display
        .segments
        .iter()
        .filter(|segment| !(**segment == Segment::Agents && !snapshot.agents.is_empty()))
        .filter_map(|segment| {
            segment_text(*segment, spinner, &cwd, elapsed, snapshot).map(|text| (*segment, text))
        })
        .collect::<Vec<(Segment, String)>>();
    if display.rows >= 2 {
        let session = segments
            .iter()
            .filter(|(segment, _)| {
                matches!(
                    segment,
                    Segment::App
                        | Segment::Model
                        | Segment::Reasoning
                        | Segment::Work
                        | Segment::Elapsed
                        | Segment::SessionId
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        let workspace = segments
            .iter()
            .filter(|(segment, _)| {
                matches!(
                    segment,
                    Segment::Git
                        | Segment::GitDirty
                        | Segment::GitStaged
                        | Segment::GitModified
                        | Segment::GitSync
                        | Segment::Worktree
                        | Segment::Cwd
                        | Segment::ProjectRoot
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        let activity = segments
            .iter()
            .filter(|(segment, _)| {
                matches!(
                    segment,
                    Segment::Context
                        | Segment::ContextRemaining
                        | Segment::ContextUsed
                        | Segment::ContextWindow
                        | Segment::Tokens
                        | Segment::InputTokens
                        | Segment::CachedTokens
                        | Segment::OutputTokens
                        | Segment::RateLimits
                        | Segment::FiveHourLimit
                        | Segment::WeeklyLimit
                        | Segment::ResetCredits
                        | Segment::Agents
                        | Segment::AgentCount
                        | Segment::Tools
                        | Segment::Plan
                        | Segment::Compactions
                        | Segment::Safety
                        | Segment::Status
                        | Segment::HooksHealth
                        | Segment::AppServerHealth
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut rows = Vec::new();
        if !session.is_empty() {
            rows.push(fit_layout(session, width as usize, &display.separator));
        }
        if display.rows == 2 {
            let mut secondary = workspace;
            secondary.extend(activity);
            if !secondary.is_empty() {
                rows.push(fit_layout(secondary, width as usize, &display.separator));
            }
        } else {
            if !workspace.is_empty() {
                rows.push(fit_layout(workspace, width as usize, &display.separator));
            }
            if !activity.is_empty() {
                rows.push(fit_layout(activity, width as usize, &display.separator));
            }
        }
        return rows;
    }
    vec![fit_layout(segments, width as usize, &display.separator)]
}

fn fit_layout(
    mut segments: Vec<(Segment, String)>,
    width: usize,
    separator: &str,
) -> Vec<(Segment, String)> {
    while rendered_width(&segments, separator) > width && segments.len() > 1 {
        let remove = segments
            .iter()
            .enumerate()
            .min_by_key(|(_, (segment, _))| segment_priority(*segment))
            .map(|(index, _)| index)
            .unwrap_or(segments.len() - 1);
        segments.remove(remove);
    }
    segments
}

#[cfg(test)]
fn plain_row(layout: &[(Segment, String)], width: usize, display: &DisplayConfig) -> String {
    let text = layout
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join(&display.separator);
    fit(format!(" {text} "), width)
}

fn write_styled_row(
    output: &mut impl Write,
    layout: &[(Segment, String)],
    width: usize,
    display: &DisplayConfig,
    snapshot: &StatusSnapshot,
) -> Result<()> {
    let base = theme_base(display.theme);
    let separator_style = theme_separator(display.theme);
    write!(output, "{base} ")?;
    for (index, (segment, text)) in layout.iter().enumerate() {
        if index > 0 {
            write!(output, "{separator_style}{}", display.separator)?;
        }
        write!(
            output,
            "{}{}{base}",
            theme_segment(display.theme, *segment, snapshot),
            text
        )?;
    }
    let used = rendered_width(layout, &display.separator).min(width);
    write!(output, "{}\x1b[0m", " ".repeat(width.saturating_sub(used)))?;
    Ok(())
}

fn segment_text(
    segment: Segment,
    spinner: &str,
    cwd: &str,
    elapsed: u64,
    snapshot: &StatusSnapshot,
) -> Option<String> {
    match segment {
        Segment::App => Some(format!("{spinner} Codex")),
        Segment::Model => snapshot.model.as_ref().map(|model| {
            let value = match &snapshot.reasoning {
                Some(reasoning) => format!("{model} {reasoning}"),
                None => model.clone(),
            };
            if snapshot.model_live {
                value
            } else {
                format!("{value} · default")
            }
        }),
        Segment::Reasoning => snapshot
            .reasoning
            .as_ref()
            .map(|reasoning| format!("reasoning {}", sanitize_dynamic(reasoning))),
        Segment::Work => snapshot.work.clone(),
        Segment::Context => context_text(snapshot),
        Segment::ContextRemaining => context_percent(snapshot)
            .map(|percent| format!("ctx {}% left", 100_u8.saturating_sub(percent))),
        Segment::ContextUsed => {
            context_percent(snapshot).map(|percent| format!("ctx {percent}% used"))
        }
        Segment::ContextWindow => snapshot
            .context_window
            .map(|window| format!("window {}", compact_number(window))),
        Segment::Tokens => token_text(snapshot),
        Segment::InputTokens => snapshot
            .input_tokens
            .map(|tokens| format!("in {}", compact_number(tokens))),
        Segment::CachedTokens => snapshot
            .cached_input_tokens
            .map(|tokens| format!("cache {}", compact_number(tokens))),
        Segment::OutputTokens => snapshot
            .output_tokens
            .map(|tokens| format!("out {}", compact_number(tokens))),
        Segment::RateLimits => rate_limits_text(snapshot),
        Segment::FiveHourLimit => limit_text(snapshot, false),
        Segment::WeeklyLimit => limit_text(snapshot, true),
        Segment::ResetCredits => snapshot
            .reset_credits
            .map(|credits| format!("reset ×{credits}")),
        Segment::Git => snapshot
            .git_branch
            .as_ref()
            .map(|branch| git_text(branch, snapshot)),
        Segment::GitDirty => snapshot.git_dirty.map(|dirty| {
            if dirty {
                "git dirty".into()
            } else {
                "git clean".into()
            }
        }),
        Segment::GitStaged => snapshot.git_staged.map(|count| format!("staged {count}")),
        Segment::GitModified => snapshot
            .git_modified
            .map(|count| format!("modified {count}")),
        Segment::GitSync => match (snapshot.git_ahead, snapshot.git_behind) {
            (None, None) => None,
            (ahead, behind) => Some(format!(
                "sync ↑{} ↓{}",
                ahead.unwrap_or(0),
                behind.unwrap_or(0)
            )),
        },
        Segment::Worktree => snapshot.worktree.as_ref().map(|worktree| {
            format!(
                "WT {}{}",
                sanitize_dynamic(worktree),
                if snapshot.linked_worktree == Some(true) {
                    " ↗"
                } else {
                    ""
                }
            )
        }),
        Segment::Tools => tools_text(snapshot),
        Segment::Agents => agents_text(snapshot),
        Segment::AgentCount => agent_count_text(snapshot),
        Segment::Plan => match (snapshot.plan_completed, snapshot.plan_total) {
            (Some(completed), Some(total)) => Some(format!("{completed}/{total} plan")),
            _ => None,
        },
        Segment::Compactions => snapshot
            .compactions
            .filter(|count| *count > 0)
            .map(|count| format!("compact ×{count}")),
        Segment::Safety => safety_text(snapshot),
        Segment::Elapsed => Some(format!("{elapsed}s")),
        Segment::Cwd => Some(format!("DIR {cwd}")),
        Segment::ProjectRoot => snapshot
            .project_root
            .as_deref()
            .map(|root| format!("ROOT {}", compact_path(root))),
        Segment::SessionId => snapshot
            .session_id
            .as_deref()
            .map(|id| format!("thread {}", sanitize_dynamic(id))),
        Segment::Status => Some(if snapshot.live_proxy_status == LiveProxyStatus::Closed {
            if snapshot.app_server_active {
                "ACCOUNT · LIVE !".into()
            } else {
                "LIVE !".into()
            }
        } else if snapshot.live_proxy_status == LiveProxyStatus::WaitingForCodex {
            "LIVE …".into()
        } else if snapshot.live_session_active && snapshot.events_active {
            "LIVE+HOOK ✓".into()
        } else if snapshot.live_session_active {
            "LIVE ✓".into()
        } else if snapshot.events_active && snapshot.app_server_active {
            "ACCOUNT+HOOK ✓".into()
        } else if snapshot.app_server_active {
            "ACCOUNT ✓".into()
        } else if snapshot.events_active {
            "HOOK ✓".into()
        } else {
            "LOCAL".into()
        }),
        Segment::HooksHealth => Some(if snapshot.events_active {
            "HOOK ✓".into()
        } else {
            "HOOK —".into()
        }),
        Segment::AppServerHealth => {
            Some(if snapshot.live_proxy_status == LiveProxyStatus::Closed {
                "LIVE !".into()
            } else if snapshot.live_proxy_status == LiveProxyStatus::WaitingForCodex {
                "LIVE …".into()
            } else if snapshot.live_session_active {
                "LIVE ✓".into()
            } else if snapshot.app_server_active {
                "ACCOUNT ✓".into()
            } else {
                "ACCOUNT —".into()
            })
        }
    }
}

fn compact_path(value: &str) -> String {
    let value = sanitize_dynamic(value);
    let Some(home) = directories::BaseDirs::new() else {
        return value;
    };
    let home = home.home_dir().to_string_lossy();
    if value == home {
        "~".into()
    } else if let Some(rest) = value
        .strip_prefix(home.as_ref())
        .and_then(|v| v.strip_prefix('/'))
    {
        format!("~/{rest}")
    } else {
        value
    }
}

fn workspace_path(snapshot: &StatusSnapshot) -> String {
    let Some(cwd) = snapshot.cwd.as_deref() else {
        return "workspace".into();
    };
    let Some(root) = snapshot.project_root.as_deref() else {
        return compact_path(cwd);
    };
    if cwd == root {
        return compact_path(root);
    }
    if let Some(relative) = cwd
        .strip_prefix(root)
        .and_then(|value| value.strip_prefix('/'))
    {
        return format!("{} › {}", compact_path(root), sanitize_dynamic(relative));
    }
    compact_path(cwd)
}

fn context_text(snapshot: &StatusSnapshot) -> Option<String> {
    let percent = context_percent(snapshot)?;
    let mut text = context_bar(percent);
    if let (Some(used), Some(window)) = (snapshot.context_used, snapshot.context_window) {
        text.push_str(&format!(
            " {}/{}",
            compact_number(used),
            compact_number(window)
        ));
    }
    if !snapshot.context_live {
        text.push_str(" · start");
    }
    Some(text)
}

fn safety_text(snapshot: &StatusSnapshot) -> Option<String> {
    let full_access = matches!(
        snapshot.sandbox.as_deref(),
        Some("danger-full-access" | "dangerFullAccess")
    );
    let sandbox = snapshot.sandbox.as_deref().map(|value| match value {
        "workspace-write" | "workspaceWrite" => "workspace",
        "read-only" | "readOnly" => "read-only",
        "danger-full-access" | "dangerFullAccess" => "full access",
        other => other,
    });
    let mode = (!full_access)
        .then(|| {
            snapshot
                .permission_mode
                .as_deref()
                .filter(|value| *value != "default")
                .or_else(|| match snapshot.approvals_reviewer.as_deref() {
                    Some("auto_review" | "autoReview" | "guardian_subagent") => {
                        Some("approve for me")
                    }
                    Some("user") | None => {
                        snapshot
                            .approval_policy
                            .as_deref()
                            .map(|value| match value {
                                "on-request" | "onRequest" => "ask",
                                "never" => "never ask",
                                "untrusted" | "unlessTrusted" => "ask untrusted",
                                other => other,
                            })
                    }
                    Some(other) => Some(other),
                })
        })
        .flatten();
    let mut parts = [sandbox, mode]
        .into_iter()
        .flatten()
        .map(sanitize_dynamic)
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return None;
    }
    if !snapshot.settings_live {
        parts.push("default".into());
    }
    Some(parts.join(" · "))
}

fn context_percent(snapshot: &StatusSnapshot) -> Option<u8> {
    snapshot.context_percent.or_else(|| {
        let used = snapshot.context_used?;
        let window = snapshot.context_window?.max(1);
        Some(
            (used
                .saturating_mul(100)
                .saturating_add(window.saturating_sub(1))
                / window)
                .min(100) as u8,
        )
    })
}

fn token_text(snapshot: &StatusSnapshot) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(value) = snapshot.input_tokens {
        parts.push(format!("in {}", compact_number(value)));
    }
    if let Some(value) = snapshot.cached_input_tokens.filter(|value| *value > 0) {
        parts.push(format!("cache {}", compact_number(value)));
    }
    if let Some(value) = snapshot.output_tokens {
        parts.push(format!("out {}", compact_number(value)));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn rate_limits_text(snapshot: &StatusSnapshot) -> Option<String> {
    let mut windows = snapshot.rate_limits.clone();
    windows.sort_by_key(|window| window.window_minutes.unwrap_or(u64::MAX));
    let mut parts = windows
        .iter()
        .take(2)
        .map(|window| {
            let label = match window.window_minutes {
                Some(minutes) if minutes <= 360 => "5H".to_owned(),
                Some(minutes) if minutes >= 7 * 24 * 60 => "WEEK".to_owned(),
                Some(minutes) => format!("{}H", minutes.div_ceil(60)),
                None => "LIMIT".to_owned(),
            };
            let left = 100_u8.saturating_sub(window.used_percent);
            let reset = window
                .resets_at
                .and_then(reset_in)
                .map(|value| format!(" ↻{value}"));
            format!("{label} {left}%{}", reset.unwrap_or_default())
        })
        .collect::<Vec<_>>();
    if let Some(credits) = snapshot.reset_credits.filter(|value| *value > 0) {
        parts.push(format!("+{credits}"));
    }
    (!parts.is_empty()).then(|| parts.join("  "))
}

fn limit_text(snapshot: &StatusSnapshot, weekly: bool) -> Option<String> {
    let mut windows = snapshot.rate_limits.iter().collect::<Vec<_>>();
    windows.sort_by_key(|window| window.window_minutes.unwrap_or(u64::MAX));
    let window = if weekly {
        windows.iter().rev().find(|window| {
            window
                .window_minutes
                .is_some_and(|minutes| minutes >= 7 * 24 * 60)
        })
    } else {
        windows
            .iter()
            .find(|window| window.window_minutes.is_some_and(|minutes| minutes <= 360))
    }
    .copied()?;
    let label = if weekly { "WEEK" } else { "5H" };
    let left = 100_u8.saturating_sub(window.used_percent);
    let reset = window
        .resets_at
        .and_then(reset_in)
        .map(|value| format!(" ↻{value}"))
        .unwrap_or_default();
    Some(format!("{label} {left}%{reset}"))
}

fn reset_in(timestamp: u64) -> Option<String> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let seconds = timestamp.saturating_sub(now);
    if seconds >= 86_400 {
        Some(format!(
            "{}d {}h",
            seconds / 86_400,
            seconds % 86_400 / 3_600
        ))
    } else if seconds >= 3_600 {
        Some(format!("{}h {}m", seconds / 3_600, seconds % 3_600 / 60))
    } else {
        Some(format!("{}m", seconds.div_ceil(60)))
    }
}

fn compact_number(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}m", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn tools_text(snapshot: &StatusSnapshot) -> Option<String> {
    if snapshot.tools.is_empty() {
        return None;
    }
    Some(format!(
        "tools: {}",
        snapshot
            .tools
            .iter()
            .take(3)
            .map(|tool| format!("{} ×{}", sanitize_dynamic(&tool.name), tool.count))
            .collect::<Vec<_>>()
            .join(" · ")
    ))
}

fn agents_text(snapshot: &StatusSnapshot) -> Option<String> {
    if !snapshot.agents.is_empty() {
        return Some(format!(
            "agents: {}",
            snapshot
                .agents
                .iter()
                .take(2)
                .map(|agent| format!(
                    "{} {}",
                    sanitize_dynamic(&agent.kind),
                    compact_duration(agent.started.elapsed().as_secs())
                ))
                .collect::<Vec<_>>()
                .join(" · ")
        ));
    }
    match (snapshot.agents_active, snapshot.agents_total) {
        (Some(active), Some(total)) if total > 0 => Some(format!("agents {active}/{total}")),
        _ => None,
    }
}

fn agent_count_text(snapshot: &StatusSnapshot) -> Option<String> {
    if let (Some(active), Some(total)) = (snapshot.agents_active, snapshot.agents_total) {
        return Some(format!("agents {active}/{total}"));
    }
    if !snapshot.agents.is_empty() {
        let active = snapshot.agents.iter().filter(|agent| agent.active).count();
        return Some(format!("agents {active}/{}", snapshot.agents.len()));
    }
    None
}

fn compact_duration(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    }
}

fn git_text(branch: &str, snapshot: &StatusSnapshot) -> String {
    let mut parts = vec![format!(
        "git:({}{})",
        sanitize_dynamic(branch),
        if snapshot.git_dirty == Some(true) {
            "*"
        } else {
            ""
        }
    )];
    if let Some(staged) = snapshot.git_staged.filter(|count| *count > 0) {
        parts.push(format!("S{staged}"));
    }
    if let Some(modified) = snapshot.git_modified.filter(|count| *count > 0) {
        parts.push(format!("M{modified}"));
    }
    if let Some(ahead) = snapshot.git_ahead.filter(|count| *count > 0) {
        parts.push(format!("↑{ahead}"));
    }
    if let Some(behind) = snapshot.git_behind.filter(|count| *count > 0) {
        parts.push(format!("↓{behind}"));
    }
    parts.join(" ")
}

fn context_bar(percent: u8) -> String {
    let percent = percent.min(100);
    let filled = usize::from((percent.saturating_add(10) / 20).min(5));
    format!(
        "CTX {}{} {percent}% used",
        "█".repeat(filled),
        "░".repeat(5 - filled)
    )
}

fn rendered_width(segments: &[(Segment, String)], separator: &str) -> usize {
    let content = segments
        .iter()
        .map(|(_, text)| UnicodeWidthStr::width(text.as_str()))
        .sum::<usize>();
    content + UnicodeWidthStr::width(separator) * segments.len().saturating_sub(1) + 2
}

fn segment_priority(segment: Segment) -> u8 {
    match segment {
        Segment::Work => 100,
        Segment::Context => 95,
        Segment::ContextRemaining | Segment::ContextUsed => 94,
        Segment::ContextWindow => 68,
        Segment::RateLimits => 94,
        Segment::FiveHourLimit | Segment::WeeklyLimit => 93,
        Segment::ResetCredits => 66,
        Segment::Tokens => 72,
        Segment::InputTokens | Segment::CachedTokens | Segment::OutputTokens => 64,
        Segment::Model => 90,
        Segment::Reasoning => 74,
        Segment::Git => 93,
        Segment::GitDirty => 85,
        Segment::GitStaged | Segment::GitModified | Segment::GitSync => 76,
        Segment::Worktree => 86,
        Segment::Tools => 78,
        Segment::App => 80,
        Segment::Agents => 70,
        Segment::AgentCount => 71,
        Segment::Plan => 65,
        Segment::Compactions => 62,
        Segment::Safety => 84,
        Segment::Elapsed => 50,
        Segment::Cwd => 90,
        Segment::ProjectRoot => 73,
        Segment::SessionId => 45,
        Segment::Status => 89,
        Segment::HooksHealth | Segment::AppServerHealth => 58,
    }
}

fn sanitize_dynamic(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .collect()
}

#[cfg(test)]
fn fit(mut text: String, width: usize) -> String {
    while UnicodeWidthStr::width(text.as_str()) > width {
        text.pop();
    }
    let padding = width.saturating_sub(UnicodeWidthStr::width(text.as_str()));
    text.push_str(&" ".repeat(padding));
    text
}

fn theme_base(theme: Theme) -> &'static str {
    match theme {
        Theme::Inherit
        | Theme::Ox96f
        | Theme::TokyoNight
        | Theme::CatppuccinMocha
        | Theme::Dracula
        | Theme::Nord
        | Theme::Gruvbox
        | Theme::RosePine
        | Theme::PastelSyntax => "\x1b[0m",
        Theme::CodexDark => "\x1b[0m\x1b[48;2;17;20;22m\x1b[38;2;214;222;217m",
        Theme::CodexLight => "\x1b[0m\x1b[48;2;238;242;239m\x1b[38;2;35;42;38m",
        Theme::Minimal | Theme::Mono => "\x1b[0m",
    }
}

fn theme_separator(theme: Theme) -> &'static str {
    if let Some(palette) = color_palette(theme) {
        return palette.muted;
    }
    match theme {
        Theme::Inherit => "\x1b[2m",
        Theme::Ox96f => "\x1b[38;2;84;84;82m",
        Theme::PastelSyntax => "\x1b[38;2;112;190;181m",
        Theme::CodexDark => "\x1b[38;2;74;85;79m",
        Theme::CodexLight => "\x1b[38;2;153;164;157m",
        Theme::Minimal => "\x1b[2m",
        Theme::Mono => "",
        Theme::TokyoNight
        | Theme::CatppuccinMocha
        | Theme::Dracula
        | Theme::Nord
        | Theme::Gruvbox
        | Theme::RosePine => unreachable!(),
    }
}

#[derive(Clone, Copy)]
struct ColorPalette {
    app: &'static str,
    info: &'static str,
    work: &'static str,
    ok: &'static str,
    warn: &'static str,
    danger: &'static str,
    git: &'static str,
    muted: &'static str,
}

fn color_palette(theme: Theme) -> Option<ColorPalette> {
    Some(match theme {
        Theme::TokyoNight => ColorPalette {
            app: "\x1b[1m\x1b[38;2;158;206;106m",
            info: "\x1b[38;2;125;207;255m",
            work: "\x1b[1m\x1b[38;2;224;175;104m",
            ok: "\x1b[38;2;158;206;106m",
            warn: "\x1b[1m\x1b[38;2;224;175;104m",
            danger: "\x1b[1m\x1b[38;2;247;118;142m",
            git: "\x1b[38;2;187;154;247m",
            muted: "\x1b[38;2;86;95;137m",
        },
        Theme::CatppuccinMocha => ColorPalette {
            app: "\x1b[1m\x1b[38;2;166;227;161m",
            info: "\x1b[38;2;116;199;236m",
            work: "\x1b[1m\x1b[38;2;249;226;175m",
            ok: "\x1b[38;2;166;227;161m",
            warn: "\x1b[1m\x1b[38;2;249;226;175m",
            danger: "\x1b[1m\x1b[38;2;243;139;168m",
            git: "\x1b[38;2;203;166;247m",
            muted: "\x1b[38;2;108;112;134m",
        },
        Theme::Dracula => ColorPalette {
            app: "\x1b[1m\x1b[38;2;80;250;123m",
            info: "\x1b[38;2;139;233;253m",
            work: "\x1b[1m\x1b[38;2;241;250;140m",
            ok: "\x1b[38;2;80;250;123m",
            warn: "\x1b[1m\x1b[38;2;241;250;140m",
            danger: "\x1b[1m\x1b[38;2;255;85;85m",
            git: "\x1b[38;2;189;147;249m",
            muted: "\x1b[38;2;98;114;164m",
        },
        Theme::Nord => ColorPalette {
            app: "\x1b[1m\x1b[38;2;163;190;140m",
            info: "\x1b[38;2;136;192;208m",
            work: "\x1b[1m\x1b[38;2;235;203;139m",
            ok: "\x1b[38;2;163;190;140m",
            warn: "\x1b[1m\x1b[38;2;235;203;139m",
            danger: "\x1b[1m\x1b[38;2;191;97;106m",
            git: "\x1b[38;2;180;142;173m",
            muted: "\x1b[38;2;76;86;106m",
        },
        Theme::Gruvbox => ColorPalette {
            app: "\x1b[1m\x1b[38;2;184;187;38m",
            info: "\x1b[38;2;142;192;124m",
            work: "\x1b[1m\x1b[38;2;250;189;47m",
            ok: "\x1b[38;2;184;187;38m",
            warn: "\x1b[1m\x1b[38;2;250;189;47m",
            danger: "\x1b[1m\x1b[38;2;251;73;52m",
            git: "\x1b[38;2;211;134;155m",
            muted: "\x1b[38;2;146;131;116m",
        },
        Theme::RosePine => ColorPalette {
            app: "\x1b[1m\x1b[38;2;156;207;216m",
            info: "\x1b[38;2;235;188;186m",
            work: "\x1b[1m\x1b[38;2;246;193;119m",
            ok: "\x1b[38;2;156;207;216m",
            warn: "\x1b[1m\x1b[38;2;246;193;119m",
            danger: "\x1b[1m\x1b[38;2;235;111;146m",
            git: "\x1b[38;2;196;167;231m",
            muted: "\x1b[38;2;110;106;134m",
        },
        _ => return None,
    })
}

fn palette_segment(
    palette: ColorPalette,
    segment: Segment,
    snapshot: &StatusSnapshot,
) -> &'static str {
    match segment {
        Segment::App => palette.app,
        Segment::Model
        | Segment::Reasoning
        | Segment::Agents
        | Segment::AgentCount
        | Segment::Tools
        | Segment::Tokens
        | Segment::InputTokens
        | Segment::CachedTokens
        | Segment::OutputTokens => palette.info,
        Segment::Work => palette.work,
        Segment::Context | Segment::ContextRemaining | Segment::ContextUsed => {
            match snapshot.context_percent.unwrap_or(0) {
                80..=u8::MAX => palette.danger,
                60..=79 => palette.warn,
                _ => palette.ok,
            }
        }
        Segment::RateLimits
        | Segment::FiveHourLimit
        | Segment::WeeklyLimit
        | Segment::ResetCredits => palette.warn,
        Segment::Git
        | Segment::GitDirty
        | Segment::GitStaged
        | Segment::GitModified
        | Segment::GitSync
        | Segment::Worktree
        | Segment::ProjectRoot
        | Segment::Plan
        | Segment::Compactions => palette.git,
        Segment::Safety => palette.danger,
        Segment::Elapsed
        | Segment::Cwd
        | Segment::ContextWindow
        | Segment::SessionId
        | Segment::Status
        | Segment::HooksHealth
        | Segment::AppServerHealth => palette.muted,
    }
}

fn theme_segment(theme: Theme, segment: Segment, snapshot: &StatusSnapshot) -> &'static str {
    if matches!(theme, Theme::Mono) {
        return "";
    }
    if matches!(theme, Theme::Minimal) {
        return match segment {
            Segment::Work
            | Segment::Context
            | Segment::ContextRemaining
            | Segment::ContextUsed
            | Segment::RateLimits
            | Segment::FiveHourLimit
            | Segment::WeeklyLimit
            | Segment::Model => "\x1b[1m",
            _ => "\x1b[2m",
        };
    }
    if matches!(theme, Theme::Inherit) {
        return match segment {
            Segment::App => "\x1b[1;32m",
            Segment::Model
            | Segment::Reasoning
            | Segment::Agents
            | Segment::AgentCount
            | Segment::Tools
            | Segment::Tokens
            | Segment::InputTokens
            | Segment::CachedTokens
            | Segment::OutputTokens => "\x1b[36m",
            Segment::Work => "\x1b[1;33m",
            Segment::Context | Segment::ContextRemaining | Segment::ContextUsed => {
                match snapshot.context_percent.unwrap_or(0) {
                    80..=u8::MAX => "\x1b[1;31m",
                    60..=79 => "\x1b[1;33m",
                    _ => "\x1b[32m",
                }
            }
            Segment::RateLimits
            | Segment::FiveHourLimit
            | Segment::WeeklyLimit
            | Segment::ResetCredits => "\x1b[1;33m",
            Segment::Git
            | Segment::GitDirty
            | Segment::GitStaged
            | Segment::GitModified
            | Segment::GitSync
            | Segment::Worktree
            | Segment::ProjectRoot
            | Segment::Plan
            | Segment::Compactions => "\x1b[35m",
            Segment::Safety => "\x1b[31m",
            Segment::Elapsed
            | Segment::Cwd
            | Segment::ContextWindow
            | Segment::SessionId
            | Segment::Status
            | Segment::HooksHealth
            | Segment::AppServerHealth => "\x1b[2m",
        };
    }
    if matches!(theme, Theme::Ox96f) {
        return match segment {
            Segment::App => "\x1b[1m\x1b[38;2;179;224;58m",
            Segment::Model
            | Segment::Reasoning
            | Segment::Agents
            | Segment::AgentCount
            | Segment::Tools
            | Segment::Tokens
            | Segment::InputTokens
            | Segment::CachedTokens
            | Segment::OutputTokens => "\x1b[38;2;0;205;232m",
            Segment::Work => "\x1b[1m\x1b[38;2;255;199;57m",
            Segment::Context | Segment::ContextRemaining | Segment::ContextUsed => {
                match snapshot.context_percent.unwrap_or(0) {
                    80..=u8::MAX => "\x1b[1m\x1b[38;2;255;102;109m",
                    60..=79 => "\x1b[1m\x1b[38;2;255;199;57m",
                    _ => "\x1b[1m\x1b[38;2;179;224;58m",
                }
            }
            Segment::RateLimits
            | Segment::FiveHourLimit
            | Segment::WeeklyLimit
            | Segment::ResetCredits => "\x1b[1m\x1b[38;2;255;199;57m",
            Segment::Git
            | Segment::GitDirty
            | Segment::GitStaged
            | Segment::GitModified
            | Segment::GitSync
            | Segment::Worktree
            | Segment::ProjectRoot
            | Segment::Plan
            | Segment::Compactions => "\x1b[38;2;163;146;232m",
            Segment::Safety => "\x1b[38;2;255;102;109m",
            Segment::Elapsed
            | Segment::Cwd
            | Segment::ContextWindow
            | Segment::SessionId
            | Segment::Status
            | Segment::HooksHealth
            | Segment::AppServerHealth => "\x1b[38;2;157;234;246m",
        };
    }
    if matches!(theme, Theme::PastelSyntax) {
        return match segment {
            Segment::App => "\x1b[1m\x1b[38;2;238;238;242m",
            Segment::Model
            | Segment::Reasoning
            | Segment::Agents
            | Segment::AgentCount
            | Segment::Tools
            | Segment::Tokens
            | Segment::InputTokens
            | Segment::CachedTokens
            | Segment::OutputTokens => "\x1b[38;2;126;162;224m",
            Segment::Work => "\x1b[1m\x1b[38;2;225;145;170m",
            Segment::Context | Segment::ContextRemaining | Segment::ContextUsed => {
                "\x1b[1m\x1b[38;2;225;145;170m"
            }
            Segment::RateLimits
            | Segment::FiveHourLimit
            | Segment::WeeklyLimit
            | Segment::ResetCredits => "\x1b[38;2;191;190;218m",
            Segment::Git
            | Segment::GitDirty
            | Segment::GitStaged
            | Segment::GitModified
            | Segment::GitSync
            | Segment::Worktree
            | Segment::ProjectRoot
            | Segment::Plan
            | Segment::Compactions => "\x1b[38;2;151;201;144m",
            Segment::Safety => "\x1b[38;2;225;145;170m",
            Segment::Status | Segment::HooksHealth | Segment::AppServerHealth => {
                "\x1b[38;2;112;190;181m"
            }
            Segment::Elapsed | Segment::Cwd | Segment::ContextWindow | Segment::SessionId => {
                "\x1b[38;2;174;176;202m"
            }
        };
    }
    if let Some(palette) = color_palette(theme) {
        return palette_segment(palette, segment, snapshot);
    }
    let dark = matches!(theme, Theme::CodexDark);
    match segment {
        Segment::App => {
            if dark {
                "\x1b[1m\x1b[38;2;110;231;168m"
            } else {
                "\x1b[1m\x1b[38;2;17;128;82m"
            }
        }
        Segment::Model
        | Segment::Reasoning
        | Segment::Agents
        | Segment::AgentCount
        | Segment::Tools
        | Segment::Tokens
        | Segment::InputTokens
        | Segment::CachedTokens
        | Segment::OutputTokens => {
            if dark {
                "\x1b[38;2;139;233;253m"
            } else {
                "\x1b[38;2;0;113;138m"
            }
        }
        Segment::Work => {
            if dark {
                "\x1b[1m\x1b[38;2;243;201;105m"
            } else {
                "\x1b[1m\x1b[38;2;154;103;0m"
            }
        }
        Segment::Context | Segment::ContextRemaining | Segment::ContextUsed => {
            match snapshot.context_percent.unwrap_or(0) {
                80..=u8::MAX => "\x1b[1m\x1b[38;2;243;123;131m",
                60..=79 => "\x1b[1m\x1b[38;2;243;201;105m",
                _ if dark => "\x1b[38;2;110;231;168m",
                _ => "\x1b[38;2;17;128;82m",
            }
        }
        Segment::RateLimits
        | Segment::FiveHourLimit
        | Segment::WeeklyLimit
        | Segment::ResetCredits => {
            if dark {
                "\x1b[1m\x1b[38;2;243;201;105m"
            } else {
                "\x1b[1m\x1b[38;2;154;103;0m"
            }
        }
        Segment::Git
        | Segment::GitDirty
        | Segment::GitStaged
        | Segment::GitModified
        | Segment::GitSync
        | Segment::Worktree
        | Segment::ProjectRoot
        | Segment::Plan
        | Segment::Compactions => {
            if dark {
                "\x1b[38;2;196;167;231m"
            } else {
                "\x1b[38;2;111;78;154m"
            }
        }
        Segment::Safety => {
            if dark {
                "\x1b[38;2;243;123;131m"
            } else {
                "\x1b[38;2;181;53;62m"
            }
        }
        Segment::Elapsed
        | Segment::Cwd
        | Segment::ContextWindow
        | Segment::SessionId
        | Segment::Status
        | Segment::HooksHealth
        | Segment::AppServerHealth => {
            if dark {
                "\x1b[2m\x1b[38;2;173;184;177m"
            } else {
                "\x1b[2m\x1b[38;2;91;103;96m"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgentInputAction, AgentPanelMode, AgentPanelState, StatusRenderer, agent_panel_layouts,
        preview_ansi, preview_line, safety_text, status_layouts,
    };
    use crate::config::{DisplayConfig, Segment, Theme};
    use crate::state::{LiveProxyStatus, StatusSnapshot};
    use std::sync::{Arc, RwLock};
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn preview_always_fits_requested_width() {
        for width in [20, 40, 80, 120] {
            let line = preview_line(width, &DisplayConfig::default());
            for row in line.lines() {
                assert_eq!(UnicodeWidthStr::width(row), width as usize);
            }
        }
    }

    #[test]
    fn preview_respects_segment_order_and_visibility() {
        let display = DisplayConfig {
            segments: vec![crate::config::Segment::Elapsed, crate::config::Segment::App],
            separator: " :: ".into(),
            ..DisplayConfig::default()
        };
        let line = preview_line(120, &display);
        assert!(line.contains("8s :: ● Codex"));
        assert!(!line.contains("companion active"));
    }

    #[test]
    fn context_summary_always_labels_used_capacity() {
        let display = DisplayConfig {
            rows: 1,
            segments: vec![Segment::Context],
            ..DisplayConfig::default()
        };
        let mut snapshot = StatusSnapshot {
            context_percent: Some(0),
            context_live: true,
            ..StatusSnapshot::default()
        };
        let fresh = status_layouts(80, &display, &snapshot, 8);
        assert_eq!(fresh[0][0].1, "CTX ░░░░░ 0% used");

        snapshot.context_percent = Some(75);
        let used = status_layouts(80, &display, &snapshot, 8);
        assert_eq!(used[0][0].1, "CTX ████░ 75% used");
    }

    #[test]
    fn startup_settings_are_labeled_and_auto_review_is_not_rendered_as_ask() {
        let display = DisplayConfig {
            rows: 1,
            segments: vec![Segment::Model, Segment::Safety, Segment::Context],
            ..DisplayConfig::default()
        };
        let snapshot = StatusSnapshot {
            model: Some("gpt-start".into()),
            reasoning: Some("medium".into()),
            context_percent: Some(0),
            sandbox: Some("workspace-write".into()),
            approval_policy: Some("on-request".into()),
            approvals_reviewer: Some("guardian_subagent".into()),
            ..StatusSnapshot::default()
        };
        let text = status_layouts(200, &display, &snapshot, 8)
            .into_iter()
            .flatten()
            .map(|(_, text)| text)
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(text.contains("gpt-start medium · default"));
        assert!(text.contains("workspace · approve for me · default"));
        assert!(text.contains("CTX ░░░░░ 0% used · start"));
    }

    #[test]
    fn full_access_suppresses_stale_approval_policy() {
        let snapshot = StatusSnapshot {
            sandbox: Some("danger-full-access".into()),
            approval_policy: Some("on-request".into()),
            settings_live: true,
            ..StatusSnapshot::default()
        };

        assert_eq!(safety_text(&snapshot).as_deref(), Some("full access"));
    }

    #[test]
    fn failed_live_source_is_visible_instead_of_looking_like_local_data() {
        let display = DisplayConfig {
            rows: 1,
            segments: vec![Segment::Status, Segment::AppServerHealth],
            ..DisplayConfig::default()
        };
        let snapshot = StatusSnapshot {
            app_server_active: true,
            live_proxy_status: LiveProxyStatus::Closed,
            live_proxy_error: Some("connection reset".into()),
            ..StatusSnapshot::default()
        };
        let text = status_layouts(80, &display, &snapshot, 8)
            .into_iter()
            .flatten()
            .map(|(_, text)| text)
            .collect::<Vec<_>>()
            .join(" | ");
        assert_eq!(text, "ACCOUNT · LIVE ! | LIVE !");
    }

    #[test]
    fn granular_segments_render_real_snapshot_values() {
        let display = DisplayConfig {
            rows: 3,
            segments: vec![
                Segment::Reasoning,
                Segment::ContextRemaining,
                Segment::ContextWindow,
                Segment::CachedTokens,
                Segment::FiveHourLimit,
                Segment::WeeklyLimit,
                Segment::ResetCredits,
                Segment::GitStaged,
                Segment::GitModified,
                Segment::GitSync,
                Segment::ProjectRoot,
                Segment::AgentCount,
                Segment::SessionId,
                Segment::HooksHealth,
                Segment::AppServerHealth,
            ],
            ..DisplayConfig::default()
        };
        let snapshot = StatusSnapshot::showcase();
        let text = status_layouts(300, &display, &snapshot, 8)
            .into_iter()
            .flatten()
            .map(|(_, text)| text)
            .collect::<Vec<_>>()
            .join(" | ");

        for expected in [
            "reasoning high",
            "ctx 58% left",
            "window 200.0k",
            "cache 51.2k",
            "5H 66%",
            "WEEK 35%",
            "reset ×1",
            "staged 1",
            "modified 3",
            "sync ↑2 ↓0",
            "ROOT ~/pro/codex-cli-codexline",
            "agents 2/3",
            "thread thr_showcase",
            "HOOK ✓",
            "LIVE ✓",
        ] {
            assert!(text.contains(expected), "missing `{expected}` in `{text}`");
        }
    }

    #[test]
    fn full_layout_keeps_workspace_and_health_context_at_common_widths() {
        let line = preview_line(90, &DisplayConfig::default());
        assert!(line.contains("git:(feat/statusline*)"));
        assert!(line.contains("DIR ~/pro/codex-cli-codexline"));
        assert!(line.contains("LIVE+HOOK ✓"));
    }

    #[test]
    fn inherit_theme_never_paints_a_background() {
        let inherited = preview_ansi(100, &DisplayConfig::default()).unwrap();
        assert!(!inherited.contains("\x1b[48;"));

        let ox96f = preview_ansi(
            100,
            &DisplayConfig {
                theme: Theme::Ox96f,
                ..DisplayConfig::default()
            },
        )
        .unwrap();
        assert!(!ox96f.contains("\x1b[48;"));
        assert!(ox96f.contains("\x1b[38;2;0;205;232m"));

        for (theme, signature) in [
            (Theme::TokyoNight, "\x1b[38;2;125;207;255m"),
            (Theme::CatppuccinMocha, "\x1b[38;2;116;199;236m"),
            (Theme::Dracula, "\x1b[38;2;139;233;253m"),
            (Theme::Nord, "\x1b[38;2;136;192;208m"),
            (Theme::Gruvbox, "\x1b[38;2;142;192;124m"),
            (Theme::RosePine, "\x1b[38;2;235;188;186m"),
            (Theme::PastelSyntax, "\x1b[1m\x1b[38;2;225;145;170m"),
        ] {
            let preview = preview_ansi(
                100,
                &DisplayConfig {
                    theme,
                    ..DisplayConfig::default()
                },
            )
            .unwrap();
            assert!(!preview.contains("\x1b[48;"));
            assert!(preview.contains(signature));
        }

        let fixed_dark = preview_ansi(
            100,
            &DisplayConfig {
                theme: Theme::CodexDark,
                ..DisplayConfig::default()
            },
        )
        .unwrap();
        assert!(fixed_dark.contains("\x1b[48;"));
    }

    #[test]
    fn agent_panel_is_visible_and_keyboard_driven() {
        let snapshot = crate::state::StatusSnapshot::showcase();
        let mut panel = AgentPanelState::default();
        let passive = agent_panel_layouts(100, &snapshot, panel);
        assert!(passive[0][0].1.contains("F2 inspect agents"));
        assert!(passive.iter().any(|row| row[0].1.contains("explore")));

        assert_eq!(
            panel.handle_input(b"\x1bOQ", snapshot.agents.len()),
            AgentInputAction::Consumed
        );
        assert_eq!(
            panel.handle_input(b"\x1b[1;1B\x1b[1;1C", snapshot.agents.len()),
            AgentInputAction::Consumed
        );
        let detail = agent_panel_layouts(100, &snapshot, panel);
        assert!(detail.iter().any(|row| row[0].1.starts_with("Goal")));
        assert!(detail.iter().any(|row| row[0].1.starts_with("Latest")));

        assert_eq!(
            panel.handle_input(b"\x1b[D", snapshot.agents.len()),
            AgentInputAction::Consumed
        );
        let list = agent_panel_layouts(100, &snapshot, panel);
        assert!(list[0][0].1.contains("AGENT INSPECTOR"));
        assert!(list.iter().any(|row| row[0].1.starts_with("▶")));
    }

    #[test]
    fn inspector_accepts_common_f2_encodings_without_stealing_ctrl_g() {
        for f2 in [
            b"\x1bOQ".as_slice(),
            b"\x1b[12~",
            b"\x1b[1;1Q",
            b"\x1b[57377u",
            b"\x1b[57377;1:1u",
        ] {
            let mut panel = AgentPanelState::default();
            assert_eq!(panel.handle_input(f2, 2), AgentInputAction::Consumed);
        }
        let mut passive = AgentPanelState::default();
        assert_eq!(passive.handle_input(&[0x07], 2), AgentInputAction::Forward);
        assert_eq!(
            passive.handle_input(b"\x1b[103;5u", 2),
            AgentInputAction::Forward
        );
        assert!(passive.should_buffer_input(b"\x1b"));
        assert!(passive.should_buffer_input(b"\x1b[57377"));
        assert!(!passive.should_buffer_input(b"\x1b[57377u"));
    }

    #[test]
    fn inspector_navigates_with_kitty_arrows_and_jk_fallbacks() {
        let mut panel = AgentPanelState::default();
        assert_eq!(
            panel.handle_input(b"\x1b[57377u", 3),
            AgentInputAction::Consumed
        );
        assert_eq!(
            panel.handle_input(b"\x1b[57353u", 3),
            AgentInputAction::Consumed
        );
        assert_eq!(panel.selected, 1);
        assert_eq!(panel.handle_input(b"j", 3), AgentInputAction::Consumed);
        assert_eq!(panel.selected, 2);
        assert_eq!(
            panel.handle_input(b"\x1b[57352;1:1u", 3),
            AgentInputAction::Consumed
        );
        assert_eq!(panel.selected, 1);
        assert_eq!(panel.handle_input(b"k", 3), AgentInputAction::Consumed);
        assert_eq!(panel.selected, 0);
        assert_eq!(
            panel.handle_input(b"\x1b[57351u", 3),
            AgentInputAction::Consumed
        );
        assert_eq!(panel.mode, AgentPanelMode::Detail);

        assert_eq!(
            panel.handle_input(b"\x1b[D\x1b[57345u", 3),
            AgentInputAction::OpenOfficialPicker
        );
        assert_eq!(panel.mode, AgentPanelMode::Passive);
    }

    #[test]
    fn renderer_diffs_rows_without_resetting_the_scroll_region() {
        let snapshot = Arc::new(RwLock::new(StatusSnapshot::showcase()));
        let mut renderer = StatusRenderer::new(DisplayConfig::default(), snapshot);
        let mut first = Vec::new();
        renderer.draw(&mut first, 100, 40).unwrap();
        let text = String::from_utf8_lossy(&first);
        assert!(text.match_indices("\x1b[1;").all(|(start, _)| {
            text[start + 4..].chars().find(char::is_ascii_alphabetic) != Some('r')
        }));
        assert!(!first.windows(2).any(|window| window == b"2K"));

        let mut unchanged = Vec::new();
        renderer.draw(&mut unchanged, 100, 40).unwrap();
        assert!(unchanged.is_empty());
    }

    #[test]
    fn invalidation_redraws_every_unchanged_hud_row() {
        let display = DisplayConfig::default();
        let snapshot = Arc::new(RwLock::new(StatusSnapshot::default()));
        let mut renderer = StatusRenderer::new(display, snapshot);
        let mut first = Vec::new();
        renderer.draw(&mut first, 100, 40).unwrap();

        renderer.invalidate();
        let mut restored = Vec::new();
        renderer.draw(&mut restored, 100, 40).unwrap();

        let restored = String::from_utf8(restored).unwrap();
        assert!(restored.contains("\x1b[38;1H"));
        assert!(restored.contains("\x1b[39;1H"));
        assert!(restored.contains("\x1b[40;1H"));
    }
}
