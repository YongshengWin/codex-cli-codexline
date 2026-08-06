use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::{self, IsTerminal, Write};

use crate::config::{self, Config, Glyphs, LaunchMode, Segment, SourcesConfig, Theme};
use crate::native_status;
use crate::process::{self, LaunchRequest};
use crate::render;
use crate::sources;

#[derive(Debug, Parser)]
#[command(name = "codexline", version, about)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the official Codex CLI through the companion.
    Run {
        /// Arguments passed unchanged to Codex.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Configure launch behavior and display defaults.
    Config,
    /// Preview the status line without starting Codex.
    Preview {
        /// Simulated terminal width.
        #[arg(short, long)]
        width: Option<u16>,
    },
    /// Diagnose Codex discovery and terminal capabilities.
    Doctor,
    /// Receive one Codex lifecycle hook event.
    #[command(hide = true)]
    Hook,
}

pub fn run() -> Result<i32> {
    if invoked_as_codex_shim() {
        return run_codex(std::env::args().skip(1).collect());
    }
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Run { args }) => run_codex(args),
        Some(Command::Config) => configure(),
        Some(Command::Preview { width }) => preview(width),
        Some(Command::Doctor) => doctor(),
        Some(Command::Hook) => crate::events::emit_hook(),
        None => run_codex(Vec::new()),
    }
}

fn invoked_as_codex_shim() -> bool {
    std::env::args_os()
        .next()
        .and_then(|path| {
            std::path::PathBuf::from(path)
                .file_stem()
                .map(|name| name.to_owned())
        })
        .is_some_and(|name| name.eq_ignore_ascii_case("codex"))
}

fn run_codex(mut args: Vec<String>) -> Result<i32> {
    let explicit_bypass = remove_bypass_flag(&mut args);
    let config = match Config::load_or_default() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("codexline: ignoring invalid configuration ({error})");
            Config::default()
        }
    };
    let bypass = process::bypass_reason(explicit_bypass);
    let is_exec = args.first().is_some_and(|arg| arg == "exec");
    if bypass.is_none() && !is_exec {
        native_status::disable_for_companion(&mut args);
    }
    let executable = process::discover_codex()?;
    let snapshot = sources::local_snapshot(&args);
    let request = LaunchRequest {
        executable,
        args,
        bypass,
        display: config.display,
        snapshot,
        app_server: config.sources.app_server,
        remote_proxy: config.sources.remote_proxy,
    };
    process::launch(request)
}

fn remove_bypass_flag(args: &mut Vec<String>) -> bool {
    let original_len = args.len();
    args.retain(|arg| arg != "--no-companion");
    args.len() != original_len
}

fn preview(width: Option<u16>) -> Result<i32> {
    let config = Config::load_or_default()?;
    let width = width
        .or_else(|| crossterm::terminal::size().ok().map(|(columns, _)| columns))
        .unwrap_or(100);
    println!("Simulated rich preview (unknown live data is hidden):");
    println!("{}", render::preview_ansi(width, &config.display)?);
    Ok(0)
}

fn doctor() -> Result<i32> {
    let config = Config::load_or_default()?;
    println!("Codexline doctor");
    println!("  config: {}", config::path()?.display());
    println!(
        "  stdin/stdout TTY: {}/{}",
        io::stdin().is_terminal(),
        io::stdout().is_terminal()
    );
    println!(
        "  TERM: {}",
        std::env::var("TERM").unwrap_or_else(|_| "<unset>".into())
    );
    match process::discover_codex() {
        Ok(path) => println!("  official Codex: {}", path.display()),
        Err(error) => println!("  official Codex: not found ({error})"),
    }
    if let Some(reason) = process::bypass_reason(false) {
        println!("  overlay: disabled ({})", reason.label());
    } else {
        println!("  overlay: available");
    }
    println!("  backend: {}", process::backend_name());
    println!(
        "  data source: {}",
        match source_preset_number(&config.sources) {
            "1" => "safe sidecar (recommended)",
            "2" => "local only",
            "3" => "experimental live proxy",
            _ => unreachable!(),
        }
    );
    let native = native_status::detect(&[]);
    println!("  native status line: {} ({})", native.state, native.source);
    Ok(0)
}

fn configure() -> Result<i32> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("interactive configuration requires a terminal");
    }

    if std::env::var_os("CODEXLINE_CONFIG_LINEAR").is_none() {
        return crate::config_ui::run(Config::load_or_default()?);
    }

    configure_linear()
}

fn configure_linear() -> Result<i32> {
    let mut config = Config::load_or_default()?;
    println!("Codexline setup · Launch (1/6)\n");
    println!("  1  Keep the `codex` command (recommended)");
    println!("     Reversible user-level shim; official Codex is never overwritten.\n");
    println!("  2  Use the explicit `codexline` command");
    println!("     Your existing `codex` command remains untouched.\n");
    let launch_default = match config.launch.mode {
        LaunchMode::Shim => "1",
        LaunchMode::Explicit => "2",
    };
    config.launch.mode = match prompt(&format!("Choose [{launch_default}]: "))?.as_str() {
        "" => config.launch.mode,
        "1" => LaunchMode::Shim,
        "2" => LaunchMode::Explicit,
        _ => anyhow::bail!("expected 1 or 2; no changes were saved"),
    };

    println!("\nCodexline setup · Preset (2/6)\n");
    println!("  1  Full      Three lanes: session, workspace/worktree, and live activity");
    println!("  2  Focus     Model, work, context, Git, and elapsed");
    println!("  3  Minimal   Work, context, and Git");
    println!("  4  Custom    Keep the current module selection");
    let preset_default = preset_number(&config.display.segments);
    let preset = prompt(&format!("Choose [{preset_default}]: "))?;
    let preset = if preset.is_empty() {
        preset_default
    } else {
        preset.as_str()
    };
    config.display.rows = match preset {
        "1" => 3,
        "2" => 2,
        "3" => 1,
        "4" => config.display.rows,
        _ => anyhow::bail!("expected 1, 2, 3, or 4; no changes were saved"),
    };
    config.display.segments = match preset {
        "1" => full_segments(),
        "2" => vec![
            Segment::App,
            Segment::Model,
            Segment::Work,
            Segment::Context,
            Segment::Git,
            Segment::Worktree,
            Segment::Tools,
            Segment::Agents,
            Segment::Elapsed,
        ],
        "3" => vec![Segment::Work, Segment::Context, Segment::Git],
        "4" => config.display.segments,
        _ => anyhow::bail!("expected 1, 2, 3, or 4; no changes were saved"),
    };

    println!("\nCodexline setup · Modules (3/6)\n");
    print_module_choices(&config.display.segments);
    println!("Toggle modules with numbers separated by spaces; Enter keeps them.");
    let toggles = prompt("Toggle: ")?;
    if !toggles.is_empty() {
        toggle_segments(&mut config.display.segments, &toggles)?;
    }
    anyhow::ensure!(
        !config.display.segments.is_empty(),
        "at least one module must remain enabled; no changes were saved"
    );

    println!("\nCodexline setup · Theme (4/6)\n");
    println!("  1  Inherit terminal theme (recommended)");
    println!("  2  0x96f Neon (transparent)");
    println!("  3  Codex Dark");
    println!("  4  Codex Light");
    println!("  5  Minimal");
    println!("  6  Mono");
    let theme_default = theme_number(config.display.theme);
    let theme = prompt(&format!("Theme [{theme_default}]: "))?;
    config.display.theme = match if theme.is_empty() {
        theme_default
    } else {
        theme.as_str()
    } {
        "1" => Theme::Inherit,
        "2" => Theme::Ox96f,
        "3" => Theme::CodexDark,
        "4" => Theme::CodexLight,
        "5" => Theme::Minimal,
        "6" => Theme::Mono,
        _ => anyhow::bail!("expected a theme from 1 to 6; no changes were saved"),
    };
    let glyph_default = match config.display.glyphs {
        Glyphs::Unicode => "1",
        Glyphs::Ascii => "2",
    };
    let glyphs = prompt(&format!("Glyphs: 1 Unicode, 2 ASCII [{glyph_default}]: "))?;
    config.display.glyphs = match glyphs.as_str() {
        "" => config.display.glyphs,
        "1" => Glyphs::Unicode,
        "2" => Glyphs::Ascii,
        _ => anyhow::bail!("expected 1 or 2; no changes were saved"),
    };

    println!("\nCodexline setup · Data sources (5/6)\n");
    println!("  1  Safe sidecar (recommended)");
    println!("     Quota plus local probes; a data-source failure cannot stop Codex.\n");
    println!("  2  Local only");
    println!("     No app-server process; Git, directory, safety, and elapsed time.\n");
    println!("  3  Experimental live proxy");
    println!("     Live thread data, but a WebSocket disconnect can terminate Codex.\n");
    let source_default = source_preset_number(&config.sources);
    let source = prompt(&format!("Choose [{source_default}]: "))?;
    apply_source_preset(
        &mut config.sources,
        if source.is_empty() {
            source_default
        } else {
            source.as_str()
        },
    )?;

    let official = process::discover_codex().ok();
    let shim = config::suggested_shim_path()?;
    let native = native_status::detect(&[]);
    println!("\nCodexline setup · Review (6/6)\n");
    println!("Wide preview:");
    println!("{}", render::preview_ansi(88, &config.display)?.trim_end());
    println!("Narrow preview:");
    println!("{}", render::preview_ansi(48, &config.display)?.trim_end());
    println!("\nDry run");
    println!("  mode: {}", config.launch.mode);
    println!("  native status line: {} ({})", native.state, native.source);
    println!(
        "  data source: {}",
        match source_preset_number(&config.sources) {
            "1" => "safe sidecar",
            "2" => "local only",
            "3" => "experimental live proxy",
            _ => unreachable!(),
        }
    );
    println!(
        "  official Codex: {}",
        official
            .as_ref()
            .map_or_else(|| "<not found>".into(), |path| path.display().to_string())
    );
    match config.launch.mode {
        LaunchMode::Shim => {
            println!("  planned shim: {}", shim.display());
            if official.as_ref().is_some_and(|path| path == &shim) {
                anyhow::bail!(
                    "planned shim conflicts with the official Codex binary; no changes were saved"
                );
            }
            if let Some(directory) = shim.parent() {
                println!("  PATH requirement: prepend {}", directory.display());
            }
            println!("  bypass: codex --no-companion");
            println!("  system changes: none in this development build");
        }
        LaunchMode::Explicit => println!("  planned shim: none"),
    }
    println!("  config: {}", config::path()?.display());
    let answer = prompt("\nSave this configuration? [Y/n]: ")?;
    if matches!(answer.as_str(), "n" | "N" | "no" | "NO") {
        println!("No changes saved.");
        return Ok(0);
    }
    config.save_atomic()?;
    let executable = std::env::current_exe().context("could not resolve the Codexline binary")?;
    println!("Saved configuration; no shim was installed.");
    println!("Preview with: {} preview", executable.display());
    Ok(0)
}

fn prompt(label: &str) -> Result<String> {
    print!("{label}");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(answer.trim().to_owned())
}

fn source_preset_number(sources: &SourcesConfig) -> &'static str {
    if sources.remote_proxy {
        "3"
    } else if sources.app_server {
        "1"
    } else {
        "2"
    }
}

fn apply_source_preset(sources: &mut SourcesConfig, choice: &str) -> Result<()> {
    match choice {
        "1" => {
            sources.app_server = true;
            sources.remote_proxy = false;
        }
        "2" => {
            sources.app_server = false;
            sources.remote_proxy = false;
        }
        "3" => {
            sources.app_server = true;
            sources.remote_proxy = true;
        }
        _ => anyhow::bail!("expected 1, 2, or 3; no changes were saved"),
    }
    Ok(())
}

fn full_segments() -> Vec<Segment> {
    vec![
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

fn preset_number(segments: &[Segment]) -> &'static str {
    if segments == full_segments() {
        "1"
    } else if segments
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
        "2"
    } else if segments == [Segment::Work, Segment::Context, Segment::Git] {
        "3"
    } else {
        "4"
    }
}

fn print_module_choices(selected: &[Segment]) {
    for (number, segment, label) in module_choices() {
        let mark = if selected.contains(&segment) {
            "x"
        } else {
            " "
        };
        println!("  {number}  [{mark}] {label}");
    }
}

fn module_choices() -> [(u8, Segment, &'static str); 16] {
    [
        (1, Segment::App, "App       Codex identity"),
        (2, Segment::Model, "Model     Model and reasoning"),
        (3, Segment::Work, "Work      Turn phase and active tool"),
        (4, Segment::Context, "Context   Context pressure"),
        (5, Segment::Git, "Git       Branch and dirty state"),
        (
            6,
            Segment::Worktree,
            "Worktree  Name and linked-worktree state",
        ),
        (7, Segment::Tools, "Tools     Current and recent activity"),
        (
            8,
            Segment::Agents,
            "Agents    Roles, state, and elapsed time",
        ),
        (9, Segment::Plan, "Plan      Current plan progress"),
        (
            10,
            Segment::Compactions,
            "Compact   Context compaction count",
        ),
        (11, Segment::Safety, "Safety    Sandbox and approval mode"),
        (12, Segment::Elapsed, "Elapsed   Session timer"),
        (13, Segment::Cwd, "Directory Current workspace"),
        (14, Segment::Status, "Status    Live data source health"),
        (
            15,
            Segment::Tokens,
            "Tokens    Input, cached, and output tokens",
        ),
        (
            16,
            Segment::RateLimits,
            "Limits    5h/weekly quota and reset",
        ),
    ]
}

fn toggle_segments(selected: &mut Vec<Segment>, input: &str) -> Result<()> {
    for token in input.split([',', ' ']).filter(|token| !token.is_empty()) {
        let number: u8 = token
            .parse()
            .with_context(|| format!("invalid module number `{token}`"))?;
        let segment = module_choices()
            .into_iter()
            .find(|choice| choice.0 == number)
            .map(|choice| choice.1)
            .with_context(|| format!("module number {number} is not available"))?;
        if let Some(index) = selected.iter().position(|current| *current == segment) {
            selected.remove(index);
        } else {
            selected.push(segment);
        }
    }
    Ok(())
}

fn theme_number(theme: Theme) -> &'static str {
    match theme {
        Theme::Inherit => "1",
        Theme::Ox96f => "2",
        Theme::CodexDark => "3",
        Theme::CodexLight => "4",
        Theme::Minimal => "5",
        Theme::Mono => "6",
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_source_preset, preset_number, remove_bypass_flag, toggle_segments};
    use crate::config::{Segment, SourcesConfig};

    #[test]
    fn removes_only_companion_bypass_flag() {
        let mut args = vec!["exec".into(), "--no-companion".into(), "hello".into()];
        assert!(remove_bypass_flag(&mut args));
        assert_eq!(args, ["exec", "hello"]);
    }

    #[test]
    fn presets_and_module_toggles_are_deterministic() {
        let mut segments = vec![Segment::Work, Segment::Context, Segment::Git];
        assert_eq!(preset_number(&segments), "3");
        toggle_segments(&mut segments, "4 13").unwrap();
        assert_eq!(segments, [Segment::Work, Segment::Git, Segment::Cwd]);
        assert_eq!(preset_number(&segments), "4");
    }

    #[test]
    fn safe_source_preset_never_enables_the_remote_proxy() {
        let mut sources = SourcesConfig {
            app_server: true,
            remote_proxy: true,
        };
        apply_source_preset(&mut sources, "1").unwrap();
        assert!(sources.app_server);
        assert!(!sources.remote_proxy);
    }
}
