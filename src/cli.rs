use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::{self, IsTerminal, Write};

use crate::config::{self, Config, LaunchMode};
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
    print!("Choose [1]: ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    config.launch.mode = match answer.trim() {
        "" | "1" => LaunchMode::Shim,
        "2" => LaunchMode::Explicit,
        _ => anyhow::bail!("expected 1 or 2; no changes were saved"),
    };

    let official = process::discover_codex().ok();
    let shim = config::suggested_shim_path()?;
    println!("\nDry run");
    println!("  mode: {}", config.launch.mode);
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
    print!("\nSave this configuration? [Y/n]: ");
    io::stdout().flush()?;
    answer.clear();
    io::stdin().read_line(&mut answer)?;
    if matches!(answer.trim(), "n" | "N" | "no" | "NO") {
        println!("No changes saved.");
        return Ok(0);
    }
    config.save_atomic()?;
    let executable = std::env::current_exe().context("could not resolve the Codexline binary")?;
    println!("Saved launch preference; no shim was installed.");
    println!("Preview with: {} preview", executable.display());
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::remove_bypass_flag;

    #[test]
    fn removes_only_companion_bypass_flag() {
        let mut args = vec!["exec".into(), "--no-companion".into(), "hello".into()];
        assert!(remove_bypass_flag(&mut args));
        assert_eq!(args, ["exec", "hello"]);
    }
}
