mod cli;
mod config;
mod native_status;
mod process;
mod render;
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
