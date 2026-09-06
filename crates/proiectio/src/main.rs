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
    // A run whose result could not be written reports the status alone, and
    // the primary write is the one failure Standout does not put on stderr
    // itself. The cause is already spelled into the error's own message.
    if let Some(failure) = &outcome.final_write_failure {
        let _ = writeln!(std::io::stderr().lock(), "Error: {failure}");
    }
    ExitCode::from(outcome.status.code())
}
