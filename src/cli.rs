use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::{self, IsTerminal, Write};

use crate::config::{self, Config, Glyphs, LaunchMode, Segment, Theme};
use crate::native_status;
use crate::process::{self, LaunchRequest};
use crate::render;

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
    let executable = process::discover_codex()?;
    let request = LaunchRequest {
        executable,
        args,
        bypass: process::bypass_reason(explicit_bypass),
        display: config.display,
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
    println!("{}", render::preview_line(width, &config.display));
    Ok(0)
}

fn doctor() -> Result<i32> {
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
    let native = native_status::detect(&[]);
    println!("  native status line: {} ({})", native.state, native.source);
    Ok(0)
}

fn configure() -> Result<i32> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("interactive configuration requires a terminal");
    }

    let mut config = Config::load_or_default()?;
    println!("Codexline setup · Launch (1/5)\n");
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

    println!("\nCodexline setup · Preset (2/5)\n");
    println!("  1  Full      App, elapsed, directory, and status");
    println!("  2  Focus     App, elapsed, and directory");
    println!("  3  Minimal   App and elapsed");
    println!("  4  Custom    Keep the current module selection");
    let preset_default = preset_number(&config.display.segments);
    let preset = prompt(&format!("Choose [{preset_default}]: "))?;
    config.display.segments = match if preset.is_empty() {
        preset_default
    } else {
        preset.as_str()
    } {
        "1" => full_segments(),
        "2" => vec![Segment::App, Segment::Elapsed, Segment::Cwd],
        "3" => vec![Segment::App, Segment::Elapsed],
        "4" => config.display.segments,
        _ => anyhow::bail!("expected 1, 2, 3, or 4; no changes were saved"),
    };

    println!("\nCodexline setup · Modules (3/5)\n");
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

    println!("\nCodexline setup · Theme (4/5)\n");
    println!("  1  Codex Dark");
    println!("  2  Codex Light");
    println!("  3  Minimal");
    println!("  4  Mono");
    let theme_default = theme_number(config.display.theme);
    let theme = prompt(&format!("Theme [{theme_default}]: "))?;
    config.display.theme = match if theme.is_empty() {
        theme_default
    } else {
        theme.as_str()
    } {
        "1" => Theme::CodexDark,
        "2" => Theme::CodexLight,
        "3" => Theme::Minimal,
        "4" => Theme::Mono,
        _ => anyhow::bail!("expected a theme from 1 to 4; no changes were saved"),
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

    let official = process::discover_codex().ok();
    let shim = config::suggested_shim_path()?;
    let native = native_status::detect(&[]);
    println!("\nCodexline setup · Review (5/5)\n");
    println!("Wide preview:");
    println!("{}", render::preview_line(88, &config.display).trim_end());
    println!("Narrow preview:");
    println!("{}", render::preview_line(48, &config.display).trim_end());
    println!("\nDry run");
    println!("  mode: {}", config.launch.mode);
    println!("  native status line: {} ({})", native.state, native.source);
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

fn full_segments() -> Vec<Segment> {
    vec![
        Segment::App,
        Segment::Elapsed,
        Segment::Cwd,
        Segment::Status,
    ]
}

fn preset_number(segments: &[Segment]) -> &'static str {
    if segments == full_segments() {
        "1"
    } else if segments == [Segment::App, Segment::Elapsed, Segment::Cwd] {
        "2"
    } else if segments == [Segment::App, Segment::Elapsed] {
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

fn module_choices() -> [(u8, Segment, &'static str); 4] {
    [
        (1, Segment::App, "App       Codex identity"),
        (2, Segment::Elapsed, "Elapsed   Session timer"),
        (3, Segment::Cwd, "Directory Current workspace"),
        (4, Segment::Status, "Status    Companion health"),
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
        Theme::CodexDark => "1",
        Theme::CodexLight => "2",
        Theme::Minimal => "3",
        Theme::Mono => "4",
    }
}

#[cfg(test)]
mod tests {
    use super::{preset_number, remove_bypass_flag, toggle_segments};
    use crate::config::Segment;

    #[test]
    fn removes_only_companion_bypass_flag() {
        let mut args = vec!["exec".into(), "--no-companion".into(), "hello".into()];
        assert!(remove_bypass_flag(&mut args));
        assert_eq!(args, ["exec", "hello"]);
    }

    #[test]
    fn presets_and_module_toggles_are_deterministic() {
        let mut segments = vec![Segment::App, Segment::Elapsed];
        assert_eq!(preset_number(&segments), "3");
        toggle_segments(&mut segments, "2 3").unwrap();
        assert_eq!(segments, [Segment::App, Segment::Cwd]);
        assert_eq!(preset_number(&segments), "4");
    }
}
