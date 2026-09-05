//! The shell-facing application over `libproiectio`.

mod app;
mod cli;
#[cfg(test)]
mod e2e_tests;
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
    // `run_emitted` is `run` up to the exit: it writes the result, the
    // warnings and any failure, and reports the status the process leaves
    // with — a refusal's included, which a handler declared on its output.
    let outcome = app.run_emitted(cli::command(), std::env::args_os());
    ExitCode::from(outcome.status.code())
}
