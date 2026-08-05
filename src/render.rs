use std::io::{self, Write};
use std::time::Instant;

use anyhow::Result;
use crossterm::terminal;
use unicode_width::UnicodeWidthStr;

use crate::config::{DisplayConfig, Glyphs, Segment, Theme};
use crate::state::StatusSnapshot;

pub struct TerminalGuard {
    child_rows: std::cell::Cell<u16>,
}

impl TerminalGuard {
    pub fn enter(child_rows: u16) -> Result<Self> {
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout().lock();
        write!(stdout, "\x1b[1;{child_rows}r\x1b[?25h")?;
        stdout.flush()?;
        Ok(Self {
            child_rows: std::cell::Cell::new(child_rows),
        })
    }

    pub fn update_reserved_rows(&self, child_rows: u16) -> Result<()> {
        self.child_rows.set(child_rows);
        let mut stdout = io::stdout().lock();
        write!(stdout, "\x1b[r\x1b[1;{child_rows}r")?;
        stdout.flush()?;
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout().lock();
        let status_row = self.child_rows.get().saturating_add(1);
        let _ = write!(
            stdout,
            "\x1b7\x1b[{status_row};1H\x1b[2K\x1b8\x1b[r\x1b[0m\x1b[?25h"
        );
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
        let text = status_line(
            width,
            &self.display,
            &self.snapshot,
            self.started.elapsed().as_secs(),
        );
        let style = theme_style(self.display.theme);
        write!(
            output,
            "\x1b7\x1b[1;{}r\x1b[{};1H\x1b[2K{style}{text}\x1b[0m\x1b8",
            rows.saturating_sub(1),
            rows,
        )?;
        Ok(())
    }
}

pub fn preview_line(width: u16, display: &DisplayConfig) -> String {
    status_line(width, display, &StatusSnapshot::showcase(), 8)
}

fn status_line(
    width: u16,
    display: &DisplayConfig,
    snapshot: &StatusSnapshot,
    elapsed: u64,
) -> String {
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
    let mut segments = display
        .segments
        .iter()
        .filter_map(|segment| {
            segment_text(*segment, spinner, &cwd, elapsed, snapshot).map(|text| (*segment, text))
        })
        .collect::<Vec<(Segment, String)>>();
    while rendered_width(&segments, &display.separator) > width as usize && segments.len() > 1 {
        let remove = segments
            .iter()
            .enumerate()
            .min_by_key(|(_, (segment, _))| segment_priority(*segment))
            .map(|(index, _)| index)
            .unwrap_or(segments.len() - 1);
        segments.remove(remove);
    }
    let text = segments
        .into_iter()
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join(&display.separator);
    fit(format!(" {text} "), width as usize)
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
        Segment::Git => snapshot.git_branch.as_ref().map(|branch| {
            format!(
                "{}{}",
                sanitize_dynamic(branch),
                if snapshot.git_dirty == Some(true) {
                    "*"
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

fn fit(mut text: String, width: usize) -> String {
    while UnicodeWidthStr::width(text.as_str()) > width {
        text.pop();
    }
    let padding = width.saturating_sub(UnicodeWidthStr::width(text.as_str()));
    text.push_str(&" ".repeat(padding));
    text
}

fn theme_style(theme: Theme) -> &'static str {
    match theme {
        Theme::CodexDark => "\x1b[48;2;17;20;22m\x1b[38;2;110;231;168m",
        Theme::CodexLight => "\x1b[48;2;238;242;239m\x1b[38;2;26;108;74m",
        Theme::Minimal => "\x1b[2m",
        Theme::Mono => "",
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
            assert_eq!(UnicodeWidthStr::width(line.as_str()), width as usize);
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
