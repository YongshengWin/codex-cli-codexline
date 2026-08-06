use anyhow::Result;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::queue;
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{
    self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};
use std::io::{self, Write};

use crate::config::{Config, Glyphs, LaunchMode, Segment, SourcesConfig, Theme};
use crate::render;

const MIN_COLUMNS: u16 = 72;
const MIN_ROWS: u16 = 24;
const SYNC_BEGIN: &str = "\x1b[?2026h";
const SYNC_END: &str = "\x1b[?2026l";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Launch,
    Preset,
    Modules,
    Appearance,
    Sources,
    Review,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModuleCategory {
    Core,
    Usage,
    Workspace,
    Activity,
    Runtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusLevel {
    Primary,
    Secondary,
    Options,
}

impl ModuleCategory {
    const ALL: [Self; 5] = [
        Self::Core,
        Self::Usage,
        Self::Workspace,
        Self::Activity,
        Self::Runtime,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Core => "Core",
            Self::Usage => "Usage",
            Self::Workspace => "Workspace",
            Self::Activity => "Activity",
            Self::Runtime => "Runtime",
        }
    }
}

impl Page {
    const ALL: [Self; 6] = [
        Self::Launch,
        Self::Preset,
        Self::Modules,
        Self::Appearance,
        Self::Sources,
        Self::Review,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Launch => "Launch",
            Self::Preset => "Preset",
            Self::Modules => "Modules",
            Self::Appearance => "Appearance",
            Self::Sources => "Data",
            Self::Review => "Review",
        }
    }
}

#[derive(Debug)]
struct Editor {
    original: Config,
    config: Config,
    page: Page,
    module_category: ModuleCategory,
    focus: FocusLevel,
    cursor: usize,
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Continue,
    Save,
    Cancel,
}

impl Editor {
    fn new(config: Config) -> Self {
        Self {
            original: config.clone(),
            config,
            page: Page::Launch,
            module_category: ModuleCategory::Core,
            focus: FocusLevel::Primary,
            cursor: 0,
            message: "Changes are staged until you save.".into(),
        }
    }

    fn dirty(&self) -> bool {
        toml::to_string(&self.original).ok() != toml::to_string(&self.config).ok()
    }

    fn option_count(&self) -> usize {
        match self.page {
            Page::Launch => 2,
            Page::Preset => 4,
            Page::Modules => module_choices(self.module_category).len(),
            Page::Appearance => theme_choices().len() + 5,
            Page::Sources => 3,
            Page::Review => 2,
        }
    }

    fn switch_page(&mut self, delta: isize) {
        let current = Page::ALL
            .iter()
            .position(|page| *page == self.page)
            .unwrap_or(0) as isize;
        let last = Page::ALL.len() as isize - 1;
        self.page = Page::ALL[(current + delta).clamp(0, last) as usize];
        self.cursor = 0;
        self.focus = FocusLevel::Primary;
    }

    fn select_page(&mut self, index: usize) {
        if let Some(page) = Page::ALL.get(index) {
            self.page = *page;
            self.cursor = 0;
            self.focus = FocusLevel::Primary;
        }
    }

    fn switch_module_category(&mut self, delta: isize) {
        let current = ModuleCategory::ALL
            .iter()
            .position(|category| *category == self.module_category)
            .unwrap_or(0) as isize;
        let last = ModuleCategory::ALL.len() as isize - 1;
        self.module_category = ModuleCategory::ALL[(current + delta).clamp(0, last) as usize];
        self.cursor = self
            .cursor
            .min(module_choices(self.module_category).len().saturating_sub(1));
    }

    fn move_cursor(&mut self, delta: isize) {
        let last = self.option_count().saturating_sub(1) as isize;
        self.cursor = (self.cursor as isize + delta).clamp(0, last) as usize;
    }

    fn navigate_up(&mut self) {
        match self.focus {
            FocusLevel::Options if self.cursor > 0 => self.move_cursor(-1),
            FocusLevel::Options if self.page == Page::Modules => {
                self.focus = FocusLevel::Secondary;
            }
            FocusLevel::Options => self.focus = FocusLevel::Primary,
            FocusLevel::Secondary => self.focus = FocusLevel::Primary,
            FocusLevel::Primary => {}
        }
    }

    fn navigate_down(&mut self) {
        match self.focus {
            FocusLevel::Primary if self.page == Page::Modules => {
                self.focus = FocusLevel::Secondary;
            }
            FocusLevel::Primary => self.focus = FocusLevel::Options,
            FocusLevel::Secondary => self.focus = FocusLevel::Options,
            FocusLevel::Options => self.move_cursor(1),
        }
    }

    fn navigate_horizontal(&mut self, delta: isize) {
        match self.focus {
            FocusLevel::Primary => self.switch_page(delta),
            FocusLevel::Secondary if self.page == Page::Modules => {
                self.switch_module_category(delta);
            }
            FocusLevel::Secondary | FocusLevel::Options => {}
        }
    }

    fn activate(&mut self) -> Result<Outcome> {
        match self.page {
            Page::Launch => {
                self.config.launch.mode = if self.cursor == 0 {
                    LaunchMode::Shim
                } else {
                    LaunchMode::Explicit
                };
            }
            Page::Preset => apply_preset(&mut self.config, self.cursor),
            Page::Modules => {
                let segment = module_choices(self.module_category)[self.cursor].0;
                if let Some(index) = self
                    .config
                    .display
                    .segments
                    .iter()
                    .position(|current| *current == segment)
                {
                    anyhow::ensure!(
                        self.config.display.segments.len() > 1,
                        "at least one module must remain enabled"
                    );
                    self.config.display.segments.remove(index);
                } else {
                    self.config.display.segments.push(segment);
                }
            }
            Page::Appearance => {
                let theme_count = theme_choices().len();
                match self.cursor {
                    cursor if cursor < theme_count => {
                        self.config.display.theme = theme_choices()[cursor].0;
                    }
                    cursor if cursor == theme_count => {
                        self.config.display.glyphs = Glyphs::Unicode;
                    }
                    cursor if cursor == theme_count + 1 => {
                        self.config.display.glyphs = Glyphs::Ascii;
                    }
                    cursor if cursor <= theme_count + 4 => {
                        self.config.display.rows = (cursor - theme_count - 1) as u8;
                    }
                    _ => unreachable!(),
                }
            }
            Page::Sources => apply_source_preset(&mut self.config.sources, self.cursor),
            Page::Review if self.cursor == 0 => {
                self.config.validate()?;
                return Ok(Outcome::Save);
            }
            Page::Review => return Ok(Outcome::Cancel),
        }
        self.message = "Selection updated · Enter saves · Esc cancels".into();
        Ok(Outcome::Continue)
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<Outcome> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(Outcome::Cancel);
        }
        match key.code {
            KeyCode::Left => self.navigate_horizontal(-1),
            KeyCode::Right => self.navigate_horizontal(1),
            KeyCode::BackTab => self.switch_page(-1),
            KeyCode::Tab => self.switch_page(1),
            KeyCode::Up | KeyCode::Char('k') => self.navigate_up(),
            KeyCode::Down | KeyCode::Char('j') => self.navigate_down(),
            KeyCode::Home if self.focus == FocusLevel::Options => self.cursor = 0,
            KeyCode::End if self.focus == FocusLevel::Options => {
                self.cursor = self.option_count().saturating_sub(1)
            }
            KeyCode::Char('1'..='6') => {
                if let KeyCode::Char(value) = key.code {
                    self.select_page(value.to_digit(10).unwrap_or(1) as usize - 1);
                }
            }
            KeyCode::Char(' ') if self.focus == FocusLevel::Options => return self.activate(),
            KeyCode::Enter | KeyCode::Char('s' | 'S') => {
                self.config.validate()?;
                return Ok(Outcome::Save);
            }
            KeyCode::Esc | KeyCode::Char('q' | 'Q') => return Ok(Outcome::Cancel),
            _ => {}
        }
        Ok(Outcome::Continue)
    }
}

struct ScreenGuard;

impl ScreenGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        if let Err(error) = execute!(io::stdout(), EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        Ok(Self)
    }
}

impl Drop for ScreenGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), ResetColor, Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

pub fn run(config: Config) -> Result<i32> {
    let mut editor = Editor::new(config);
    let screen = ScreenGuard::enter()?;
    draw(&editor)?;
    let outcome = loop {
        match event::read()? {
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                match editor.handle_key(key) {
                    Ok(Outcome::Continue) => draw(&editor)?,
                    Ok(outcome) => break outcome,
                    Err(error) => {
                        editor.message = error.to_string();
                        draw(&editor)?;
                    }
                }
            }
            Event::Resize(_, _) => draw(&editor)?,
            _ => {}
        }
    };

    match outcome {
        Outcome::Save => editor.config.save_atomic()?,
        Outcome::Cancel | Outcome::Continue => {}
    }
    drop(screen);
    match outcome {
        Outcome::Save => println!("Saved {}", crate::config::path()?.display()),
        Outcome::Cancel | Outcome::Continue => println!("No changes saved."),
    }
    Ok(0)
}

fn draw(editor: &Editor) -> Result<()> {
    let (columns, rows) = terminal::size()?;
    let mut output = io::stdout().lock();
    write!(output, "{SYNC_BEGIN}")?;
    queue!(output, MoveTo(0, 0), Clear(ClearType::All))?;
    if columns < MIN_COLUMNS || rows < MIN_ROWS {
        draw_small(&mut output, columns, rows)?;
    } else {
        draw_full(&mut output, editor, columns, rows)?;
    }
    write!(output, "{SYNC_END}")?;
    output.flush()?;
    Ok(())
}

fn draw_small(output: &mut impl Write, columns: u16, rows: u16) -> Result<()> {
    let message = format!(
        "Codexline config needs {MIN_COLUMNS}×{MIN_ROWS}; current terminal is {columns}×{rows}."
    );
    queue!(
        output,
        MoveTo(2, 2),
        SetForegroundColor(Color::Yellow),
        Print(message),
        MoveTo(2, 4),
        ResetColor,
        Print("Resize the terminal, or press Esc/Q to cancel.")
    )?;
    Ok(())
}

fn draw_full(output: &mut impl Write, editor: &Editor, columns: u16, rows: u16) -> Result<()> {
    queue!(
        output,
        MoveTo(2, 1),
        SetForegroundColor(Color::Cyan),
        SetAttribute(Attribute::Bold),
        Print("Codexline Config"),
        ResetColor,
        SetAttribute(Attribute::Reset),
        SetForegroundColor(Color::DarkGrey),
        Print(if editor.dirty() {
            "  ● unsaved"
        } else {
            "  ✓ saved"
        }),
        ResetColor
    )?;

    let mut tab_x = 2;
    for (index, page) in Page::ALL.iter().enumerate() {
        let active = *page == editor.page;
        let focused = active && editor.focus == FocusLevel::Primary;
        queue!(
            output,
            MoveTo(tab_x, 3),
            SetForegroundColor(if focused {
                Color::Black
            } else if active {
                Color::Cyan
            } else {
                Color::Grey
            }),
            crossterm::style::SetBackgroundColor(if focused { Color::Cyan } else { Color::Reset }),
            SetAttribute(if active {
                Attribute::Bold
            } else {
                Attribute::Reset
            }),
            Print(format!(" {} {} ", index + 1, page.title())),
            SetAttribute(Attribute::Reset),
            ResetColor
        )?;
        tab_x += page.title().len() as u16 + 5;
    }

    let content_width = columns.saturating_sub(4);
    let preview = render::preview_ansi(content_width, &editor.config.display)?;
    let preview_lines = preview.lines().collect::<Vec<_>>();
    let footer_y = rows - 3;
    let preview_y = footer_y.saturating_sub(preview_lines.len() as u16);
    let divider_y = preview_y.saturating_sub(2);
    if editor.page == Page::Modules {
        draw_module_categories(output, editor, 2, 5)?;
        draw_options(
            output,
            editor,
            2,
            7,
            content_width,
            divider_y.saturating_sub(7),
        )?;
    } else {
        let options_y = 6;
        draw_options(
            output,
            editor,
            2,
            options_y,
            content_width,
            divider_y.saturating_sub(options_y),
        )?;
    }
    draw_preview_divider(output, editor, 2, divider_y, content_width)?;
    draw_preview(output, &preview_lines, 2, preview_y)?;

    queue!(
        output,
        MoveTo(2, footer_y),
        SetForegroundColor(Color::DarkGrey),
        Print(truncate(
            &editor.message,
            columns.saturating_sub(4) as usize
        )),
        MoveTo(2, rows - 2),
        SetForegroundColor(Color::Grey),
        Print(if editor.page == Page::Modules {
            "↑↓ level/move   ←→ current tab   Space toggle   Enter save   Esc cancel"
        } else {
            "↑↓ level/move   ←→ current tab   Space select   Enter save   Esc cancel"
        }),
        ResetColor
    )?;
    Ok(())
}

fn draw_module_categories(output: &mut impl Write, editor: &Editor, x: u16, y: u16) -> Result<()> {
    let mut category_x = x;
    for category in ModuleCategory::ALL {
        let active = category == editor.module_category;
        let focused = active && editor.focus == FocusLevel::Secondary;
        queue!(
            output,
            MoveTo(category_x, y),
            SetForegroundColor(if focused {
                Color::Black
            } else if active {
                Color::Cyan
            } else {
                Color::DarkGrey
            }),
            crossterm::style::SetBackgroundColor(if focused { Color::Cyan } else { Color::Reset }),
            SetAttribute(if active {
                Attribute::Bold
            } else {
                Attribute::Reset
            }),
            Print(format!(
                "{} {} ",
                if active { "◆" } else { "·" },
                category.title()
            )),
            SetAttribute(Attribute::Reset),
            ResetColor
        )?;
        category_x += category.title().len() as u16 + 4;
    }
    Ok(())
}

fn draw_options(
    output: &mut impl Write,
    editor: &Editor,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
) -> Result<()> {
    let options = option_lines(editor);
    let visible = usize::from(height.max(1));
    let start = editor
        .cursor
        .saturating_sub(visible.saturating_sub(1))
        .min(options.len().saturating_sub(visible));
    for (offset, (label, selected)) in options.iter().skip(start).take(visible).enumerate() {
        let index = start + offset;
        let focused = editor.focus == FocusLevel::Options && index == editor.cursor;
        let label = if let Some(rest) = label.strip_prefix("( )") {
            format!("({}){rest}", if *selected { "●" } else { " " })
        } else {
            label.clone()
        };
        queue!(
            output,
            MoveTo(x, y + offset as u16),
            SetForegroundColor(if focused {
                Color::Cyan
            } else if *selected {
                Color::Green
            } else {
                Color::Grey
            }),
            SetAttribute(if focused {
                Attribute::Bold
            } else {
                Attribute::Reset
            }),
            Print(if focused { "› " } else { "  " }),
            Print(truncate(&label, width.saturating_sub(2) as usize)),
            SetAttribute(Attribute::Reset),
            ResetColor
        )?;
    }
    Ok(())
}

fn draw_preview(output: &mut impl Write, preview_lines: &[&str], x: u16, y: u16) -> Result<()> {
    for (offset, line) in preview_lines.iter().enumerate() {
        queue!(output, MoveTo(x, y + offset as u16), Print(line))?;
    }
    Ok(())
}

fn draw_preview_divider(
    output: &mut impl Write,
    editor: &Editor,
    x: u16,
    y: u16,
    width: u16,
) -> Result<()> {
    let summary = format!(
        " Live preview · {} · {} rows · {} modules · {} ",
        preset_name(&editor.config),
        editor.config.display.rows,
        editor.config.display.segments.len(),
        source_name(&editor.config.sources),
    );
    queue!(
        output,
        MoveTo(x, y),
        SetForegroundColor(Color::DarkGrey),
        Print("─".repeat(width as usize)),
        MoveTo(x, y),
        SetForegroundColor(Color::Magenta),
        SetAttribute(Attribute::Bold),
        Print(truncate(&summary, width as usize)),
        SetAttribute(Attribute::Reset),
        ResetColor
    )?;
    Ok(())
}

fn option_lines(editor: &Editor) -> Vec<(String, bool)> {
    match editor.page {
        Page::Launch => vec![
            (
                "( ) Keep `codex` command · recommended".into(),
                editor.config.launch.mode == LaunchMode::Shim,
            ),
            (
                "( ) Use explicit `codexline` command".into(),
                editor.config.launch.mode == LaunchMode::Explicit,
            ),
        ],
        Page::Preset => [
            "Full · 3 rich rows",
            "Focus · 2 rows",
            "Minimal · 1 row",
            "Custom",
        ]
        .into_iter()
        .enumerate()
        .map(|(index, label)| {
            (
                format!("( ) {label}"),
                preset_index(&editor.config) == index,
            )
        })
        .collect(),
        Page::Modules => module_choices(editor.module_category)
            .iter()
            .map(|(segment, label)| {
                (
                    format!(
                        "[{}] {label}",
                        if editor.config.display.segments.contains(segment) {
                            "x"
                        } else {
                            " "
                        }
                    ),
                    editor.config.display.segments.contains(segment),
                )
            })
            .collect(),
        Page::Appearance => {
            let mut lines = theme_choices()
                .iter()
                .copied()
                .map(|(theme, label)| {
                    (
                        format!("( ) {label}"),
                        theme_index(editor.config.display.theme) == theme_index(theme),
                    )
                })
                .collect::<Vec<_>>();
            lines.push((
                "( ) Glyphs · Unicode".into(),
                matches!(editor.config.display.glyphs, Glyphs::Unicode),
            ));
            lines.push((
                "( ) Glyphs · ASCII".into(),
                matches!(editor.config.display.glyphs, Glyphs::Ascii),
            ));
            for rows in 1..=3 {
                lines.push((
                    format!(
                        "( ) Layout · {rows} row{}",
                        if rows == 1 { "" } else { "s" }
                    ),
                    editor.config.display.rows == rows,
                ));
            }
            lines
        }
        Page::Sources => [
            "Safe sidecar · recommended",
            "Local only",
            "Experimental live proxy · may disconnect Codex",
        ]
        .into_iter()
        .enumerate()
        .map(|(index, label)| {
            (
                format!("( ) {label}"),
                source_index(&editor.config.sources) == index,
            )
        })
        .collect(),
        Page::Review => vec![
            ("Save configuration".into(), false),
            ("Cancel without saving".into(), false),
        ],
    }
}

fn module_choices(category: ModuleCategory) -> &'static [(Segment, &'static str)] {
    match category {
        ModuleCategory::Core => &[
            (Segment::App, "App · Codex identity"),
            (Segment::Model, "Model summary · model and reasoning"),
            (Segment::Reasoning, "Reasoning effort"),
            (Segment::Work, "Work · phase and active tool"),
            (Segment::Elapsed, "Elapsed · session timer"),
            (Segment::SessionId, "Thread ID · current session"),
        ],
        ModuleCategory::Usage => &[
            (
                Segment::Context,
                "Context summary · pressure bar and tokens",
            ),
            (Segment::ContextRemaining, "Context remaining · percentage"),
            (Segment::ContextUsed, "Context used · percentage"),
            (Segment::ContextWindow, "Context window · token capacity"),
            (Segment::Tokens, "Token summary · input/cache/output"),
            (Segment::InputTokens, "Input tokens"),
            (Segment::CachedTokens, "Cached input tokens"),
            (Segment::OutputTokens, "Output tokens"),
            (Segment::RateLimits, "Limit summary · quota and reset"),
            (Segment::FiveHourLimit, "5-hour limit · remaining and reset"),
            (Segment::WeeklyLimit, "Weekly limit · remaining and reset"),
            (Segment::ResetCredits, "Extra usage resets"),
        ],
        ModuleCategory::Workspace => &[
            (Segment::Git, "Git summary · branch and changes"),
            (Segment::GitDirty, "Git working-tree state"),
            (Segment::GitStaged, "Git staged file count"),
            (Segment::GitModified, "Git modified file count"),
            (Segment::GitSync, "Git ahead/behind"),
            (Segment::Worktree, "Worktree · linked workspace"),
            (Segment::Cwd, "Directory · workspace path"),
            (Segment::ProjectRoot, "Project root"),
        ],
        ModuleCategory::Activity => &[
            (Segment::Tools, "Tools · recent activity"),
            (
                Segment::Agents,
                "Agent detail panel · role/task/message/time",
            ),
            (Segment::AgentCount, "Agent count · active/total"),
            (Segment::Plan, "Plan · progress"),
            (Segment::Compactions, "Compactions · count"),
        ],
        ModuleCategory::Runtime => &[
            (Segment::Safety, "Safety · sandbox and approval"),
            (Segment::Status, "Source health summary"),
            (Segment::HooksHealth, "Hooks event health"),
            (Segment::AppServerHealth, "App-server health"),
        ],
    }
}

fn full_segments() -> [Segment; 16] {
    [
        Segment::App,
        Segment::Model,
        Segment::Work,
        Segment::Context,
        Segment::Tokens,
        Segment::RateLimits,
        Segment::Git,
        Segment::Worktree,
        Segment::Tools,
        Segment::Agents,
        Segment::Plan,
        Segment::Compactions,
        Segment::Safety,
        Segment::Elapsed,
        Segment::Cwd,
        Segment::Status,
    ]
}

fn apply_preset(config: &mut Config, index: usize) {
    match index {
        0 => {
            config.display.rows = 3;
            config.display.segments = full_segments().to_vec();
        }
        1 => {
            config.display.rows = 2;
            config.display.segments = vec![
                Segment::App,
                Segment::Model,
                Segment::Work,
                Segment::Context,
                Segment::Git,
                Segment::Worktree,
                Segment::Tools,
                Segment::Agents,
                Segment::Elapsed,
            ];
        }
        2 => {
            config.display.rows = 1;
            config.display.segments = vec![Segment::Work, Segment::Context, Segment::Git];
        }
        3 => {}
        _ => unreachable!(),
    }
}

fn apply_source_preset(sources: &mut SourcesConfig, index: usize) {
    match index {
        0 => {
            sources.app_server = true;
            sources.remote_proxy = false;
        }
        1 => {
            sources.app_server = false;
            sources.remote_proxy = false;
        }
        2 => {
            sources.app_server = true;
            sources.remote_proxy = true;
        }
        _ => unreachable!(),
    }
}

fn preset_index(config: &Config) -> usize {
    if config.display.rows == 3 && config.display.segments == full_segments() {
        0
    } else if config.display.rows == 2
        && config.display.segments
            == [
                Segment::App,
                Segment::Model,
                Segment::Work,
                Segment::Context,
                Segment::Git,
                Segment::Worktree,
                Segment::Tools,
                Segment::Agents,
                Segment::Elapsed,
            ]
    {
        1
    } else if config.display.rows == 1
        && config.display.segments == [Segment::Work, Segment::Context, Segment::Git]
    {
        2
    } else {
        3
    }
}

fn preset_name(config: &Config) -> &'static str {
    ["Full", "Focus", "Minimal", "Custom"][preset_index(config)]
}

fn source_index(sources: &SourcesConfig) -> usize {
    if sources.remote_proxy {
        2
    } else if sources.app_server {
        0
    } else {
        1
    }
}

fn source_name(sources: &SourcesConfig) -> &'static str {
    ["Safe sidecar", "Local only", "Experimental proxy"][source_index(sources)]
}

fn theme_index(theme: Theme) -> usize {
    match theme {
        Theme::Inherit => 0,
        Theme::Ox96f => 1,
        Theme::TokyoNight => 2,
        Theme::CatppuccinMocha => 3,
        Theme::Dracula => 4,
        Theme::Nord => 5,
        Theme::Gruvbox => 6,
        Theme::RosePine => 7,
        Theme::CodexDark => 8,
        Theme::CodexLight => 9,
        Theme::Minimal => 10,
        Theme::Mono => 11,
    }
}

fn theme_choices() -> &'static [(Theme, &'static str)] {
    &[
        (Theme::Inherit, "Theme · Inherit terminal"),
        (Theme::Ox96f, "Theme · 0x96f Neon · transparent"),
        (Theme::TokyoNight, "Theme · Tokyo Night · transparent"),
        (
            Theme::CatppuccinMocha,
            "Theme · Catppuccin Mocha · transparent",
        ),
        (Theme::Dracula, "Theme · Dracula · transparent"),
        (Theme::Nord, "Theme · Nord · transparent"),
        (Theme::Gruvbox, "Theme · Gruvbox · transparent"),
        (Theme::RosePine, "Theme · Rosé Pine · transparent"),
        (Theme::CodexDark, "Theme · Codex Dark · fixed background"),
        (Theme::CodexLight, "Theme · Codex Light · fixed background"),
        (Theme::Minimal, "Theme · Minimal"),
        (Theme::Mono, "Theme · Mono"),
    ]
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_owned();
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    let mut output = value.chars().take(width - 1).collect::<String>();
    output.push('…');
    output
}

#[cfg(test)]
mod tests {
    use super::{Editor, FocusLevel, ModuleCategory, Outcome, Page, preset_index, source_index};
    use crate::config::{Config, Segment};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn keyboard_navigation_and_module_toggle_are_deterministic() {
        let mut editor = Editor::new(Config::default());
        editor.select_page(2);
        assert_eq!(editor.page, Page::Modules);
        assert!(editor.config.display.segments.contains(&Segment::App));
        editor.handle_key(key(KeyCode::Down)).unwrap();
        editor.handle_key(key(KeyCode::Down)).unwrap();
        assert_eq!(
            editor.handle_key(key(KeyCode::Char(' '))).unwrap(),
            Outcome::Continue
        );
        assert!(!editor.config.display.segments.contains(&Segment::App));
        assert!(editor.dirty());
    }

    #[test]
    fn module_categories_use_horizontal_navigation_without_leaving_the_page() {
        let mut editor = Editor::new(Config::default());
        editor.select_page(2);

        editor.handle_key(key(KeyCode::Down)).unwrap();
        assert_eq!(editor.focus, FocusLevel::Secondary);
        editor.handle_key(key(KeyCode::Right)).unwrap();
        assert_eq!(editor.page, Page::Modules);
        assert_eq!(editor.module_category, ModuleCategory::Usage);
        assert_eq!(editor.cursor, 0);

        editor.handle_key(key(KeyCode::Tab)).unwrap();
        assert_eq!(editor.page, Page::Appearance);
    }

    #[test]
    fn arrows_move_between_levels_space_edits_and_enter_saves() {
        let mut editor = Editor::new(Config::default());
        editor.select_page(2);
        assert_eq!(editor.focus, FocusLevel::Primary);

        editor.handle_key(key(KeyCode::Down)).unwrap();
        assert_eq!(editor.focus, FocusLevel::Secondary);
        editor.handle_key(key(KeyCode::Down)).unwrap();
        assert_eq!(editor.focus, FocusLevel::Options);

        assert!(editor.config.display.segments.contains(&Segment::App));
        editor.handle_key(key(KeyCode::Char(' '))).unwrap();
        assert!(!editor.config.display.segments.contains(&Segment::App));

        editor.handle_key(key(KeyCode::Up)).unwrap();
        assert_eq!(editor.focus, FocusLevel::Secondary);
        editor.handle_key(key(KeyCode::Up)).unwrap();
        assert_eq!(editor.focus, FocusLevel::Primary);

        assert_eq!(
            editor.handle_key(key(KeyCode::Enter)).unwrap(),
            Outcome::Save
        );
    }

    #[test]
    fn presets_and_sources_update_the_same_staged_snapshot() {
        let mut editor = Editor::new(Config::default());
        editor.select_page(1);
        editor.cursor = 2;
        editor.activate().unwrap();
        assert_eq!(preset_index(&editor.config), 2);

        editor.select_page(4);
        editor.cursor = 1;
        editor.activate().unwrap();
        assert_eq!(source_index(&editor.config.sources), 1);
        assert!(!editor.config.sources.app_server);
    }

    #[test]
    fn escape_cancels_without_mutating_the_original_snapshot() {
        let mut editor = Editor::new(Config::default());
        editor.config.display.rows = 1;
        assert_eq!(
            editor.handle_key(key(KeyCode::Esc)).unwrap(),
            Outcome::Cancel
        );
        assert_eq!(editor.original.display.rows, 3);
    }
}
