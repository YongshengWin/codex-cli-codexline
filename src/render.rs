use std::io::{self, Write};
use std::time::Instant;

use anyhow::Result;
use crossterm::terminal;
use unicode_width::UnicodeWidthStr;

use crate::config::{DisplayConfig, Glyphs, Segment, Theme};
use crate::state::StatusSnapshot;

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
        self.child_rows.set(child_rows);
        self.reserved_rows.set(reserved_rows);
        let mut stdout = io::stdout().lock();
        write!(stdout, "\x1b[r\x1b[1;{child_rows}r")?;
        stdout.flush()?;
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
    snapshot: StatusSnapshot,
    started: Instant,
}

impl StatusRenderer {
    pub fn new(display: DisplayConfig, snapshot: StatusSnapshot) -> Self {
        Self {
            display,
            snapshot,
            started: Instant::now(),
        }
    }

    pub fn draw(&mut self, output: &mut impl Write, width: u16, rows: u16) -> Result<()> {
        let layouts = status_layouts(
            width,
            &self.display,
            &self.snapshot,
            self.started.elapsed().as_secs(),
        );
        let first_row = rows.saturating_sub(layouts.len() as u16).saturating_add(1);
        write!(output, "\x1b7\x1b[1;{}r", first_row.saturating_sub(1))?;
        for (offset, layout) in layouts.iter().enumerate() {
            let row = first_row.saturating_add(offset as u16);
            write!(output, "\x1b[{row};1H\x1b[2K")?;
            write_styled_row(
                output,
                layout,
                width as usize,
                &self.display,
                &self.snapshot,
            )?;
        }
        write!(output, "\x1b[0m\x1b8")?;
        Ok(())
    }
}

#[cfg(test)]
pub fn preview_line(width: u16, display: &DisplayConfig) -> String {
    status_layouts(width, display, &StatusSnapshot::showcase(), 8)
        .iter()
        .map(|layout| plain_row(layout, width as usize, display))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn preview_ansi(width: u16, display: &DisplayConfig) -> Result<String> {
    let snapshot = StatusSnapshot::showcase();
    let layouts = status_layouts(width, display, &snapshot, 8);
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
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "workspace".into());
    let cwd = sanitize_dynamic(&cwd);
    let segments = display
        .segments
        .iter()
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
                    Segment::App | Segment::Model | Segment::Work | Segment::Elapsed
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        let workspace = segments
            .iter()
            .filter(|(segment, _)| {
                matches!(segment, Segment::Git | Segment::Worktree | Segment::Cwd)
            })
            .cloned()
            .collect::<Vec<_>>();
        let activity = segments
            .iter()
            .filter(|(segment, _)| {
                matches!(
                    segment,
                    Segment::Context
                        | Segment::Agents
                        | Segment::Plan
                        | Segment::Safety
                        | Segment::Status
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
        Segment::Model => snapshot
            .model
            .as_ref()
            .map(|model| match &snapshot.reasoning {
                Some(reasoning) => format!("{model} {reasoning}"),
                None => model.clone(),
            }),
        Segment::Work => snapshot.work.clone(),
        Segment::Context => snapshot.context_percent.map(context_bar),
        Segment::Git => snapshot
            .git_branch
            .as_ref()
            .map(|branch| git_text(branch, snapshot)),
        Segment::Worktree => snapshot.worktree.as_ref().map(|worktree| {
            format!(
                "wt:{}{}",
                sanitize_dynamic(worktree),
                if snapshot.linked_worktree == Some(true) {
                    " ↗"
                } else {
                    ""
                }
            )
        }),
        Segment::Agents => match (snapshot.agents_active, snapshot.agents_total) {
            (Some(active), Some(total)) => Some(format!("↑{active}/{total} agents")),
            _ => None,
        },
        Segment::Plan => match (snapshot.plan_completed, snapshot.plan_total) {
            (Some(completed), Some(total)) => Some(format!("{completed}/{total} plan")),
            _ => None,
        },
        Segment::Safety => snapshot.safety.as_deref().map(sanitize_dynamic),
        Segment::Elapsed => Some(format!("{elapsed}s")),
        Segment::Cwd => Some(cwd.to_owned()),
        Segment::Status => Some("healthy".into()),
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
    let filled = usize::from(percent.div_ceil(20));
    format!(
        "ctx {}{} {percent}%",
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
        Segment::Model => 90,
        Segment::Git => 85,
        Segment::Worktree => 82,
        Segment::App => 80,
        Segment::Agents => 70,
        Segment::Plan => 65,
        Segment::Safety => 60,
        Segment::Elapsed => 50,
        Segment::Cwd => 40,
        Segment::Status => 10,
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
        Theme::CodexDark => "\x1b[0m\x1b[48;2;17;20;22m\x1b[38;2;214;222;217m",
        Theme::CodexLight => "\x1b[0m\x1b[48;2;238;242;239m\x1b[38;2;35;42;38m",
        Theme::Minimal | Theme::Mono => "\x1b[0m",
    }
}

fn theme_separator(theme: Theme) -> &'static str {
    match theme {
        Theme::CodexDark => "\x1b[38;2;74;85;79m",
        Theme::CodexLight => "\x1b[38;2;153;164;157m",
        Theme::Minimal => "\x1b[2m",
        Theme::Mono => "",
    }
}

fn theme_segment(theme: Theme, segment: Segment, snapshot: &StatusSnapshot) -> &'static str {
    if matches!(theme, Theme::Mono) {
        return "";
    }
    if matches!(theme, Theme::Minimal) {
        return match segment {
            Segment::Work | Segment::Context | Segment::Model => "\x1b[1m",
            _ => "\x1b[2m",
        };
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
        Segment::Model | Segment::Agents => {
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
        Segment::Context => match snapshot.context_percent.unwrap_or(0) {
            80..=u8::MAX => "\x1b[1m\x1b[38;2;243;123;131m",
            60..=79 => "\x1b[1m\x1b[38;2;243;201;105m",
            _ if dark => "\x1b[38;2;110;231;168m",
            _ => "\x1b[38;2;17;128;82m",
        },
        Segment::Git | Segment::Worktree | Segment::Plan => {
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
        Segment::Elapsed | Segment::Cwd | Segment::Status => {
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
    use super::preview_line;
    use crate::config::DisplayConfig;
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
}
