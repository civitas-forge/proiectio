//! The shell-facing application over `libproiectio`.

mod app;
mod cli;
mod exit;
mod handlers;
mod settings;
#[cfg(test)]
mod testing;
mod views;

use std::process::ExitCode;

fn main() -> ExitCode {
    let app = match app::build() {
        Ok(app) => app,
        Err(error) => {
            eprintln!("Error: {error}");
            return ExitCode::from(exit::FAILURE);
        }
    };
    let result = app.run_to_string(cli::command(), std::env::args());
    exit::emit(&result);
    ExitCode::from(exit::status(&result))
}
