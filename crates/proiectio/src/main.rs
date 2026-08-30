//! The shell-facing application over `libproiectio`.

mod app;
mod cli;
mod exit;
mod handlers;
mod settings;
#[cfg(test)]
mod testing;
mod views;

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let app = match app::build() {
        Ok(app) => app,
        Err(error) => {
            let _ = writeln!(std::io::stderr().lock(), "Error: {error}");
            return ExitCode::from(exit::FAILURE);
        }
    };
    let result = app.run_to_string(cli::command(), std::env::args_os());
    ExitCode::from(exit::emit(&result))
}
