use std::io::{self, Write};
use std::time::Instant;

use anyhow::Result;
use crossterm::terminal;
use unicode_width::UnicodeWidthStr;

use crate::config::{DisplayConfig, Glyphs, Theme};

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
        let _ = write!(stdout, "\x1b[r\x1b[0m\x1b[?25h");
        let _ = stdout.flush();
        let _ = terminal::disable_raw_mode();
    }
}

pub struct StatusRenderer {
    display: DisplayConfig,
    started: Instant,
}

impl StatusRenderer {
    pub fn new(display: DisplayConfig) -> Self {
        Self {
            display,
            started: Instant::now(),
        }
    }

    pub fn draw(&mut self, output: &mut impl Write, width: u16, rows: u16) -> Result<()> {
        let text = status_line(width, &self.display, self.started.elapsed().as_secs());
        let style = theme_style(self.display.theme);
        write!(
            output,
            "\x1b[1;{}r\x1b7\x1b[{};1H\x1b[2K{style}{text}\x1b[0m\x1b8",
            rows.saturating_sub(1),
            rows,
        )?;
        Ok(())
    }
}

pub fn preview_line(width: u16, display: &DisplayConfig) -> String {
    status_line(width, display, 8)
}

fn status_line(width: u16, display: &DisplayConfig, elapsed: u64) -> String {
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
    let full = format!(" {spinner} Codex · {elapsed}s │ {cwd} │ companion active ");
    let compact = format!(" {spinner} Codex · {elapsed}s │ {cwd} ");
    fit(
        if UnicodeWidthStr::width(full.as_str()) <= width as usize {
            full
        } else {
            compact
        },
        width as usize,
    )
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
}
