mod app_server;
mod cli;
mod config;
mod config_ui;
mod events;
mod native_status;
mod process;
mod render;
mod shim;
mod sources;
mod state;

use std::process::ExitCode;

fn main() -> ExitCode {
    match cli::run() {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(error) => {
            eprintln!("codexline: {error:#}");
            ExitCode::FAILURE
        }
    }
}
