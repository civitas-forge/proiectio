//! The 0/1/2 exit contract, which `main` owns because Standout spends 2
//! on a command line clap rejects and this CLI spends it on refusals.

use libproiectio::Error;
use standout::cli::{ExternalFailure, RunError, RunErrorKind, RunResult};

pub(crate) const OK: u8 = 0;
pub(crate) const FAILURE: u8 = 1;
pub(crate) const REFUSAL: u8 = 2;

pub(crate) fn of_error(error: &Error) -> u8 {
    match error {
        Error::Refused(_) => REFUSAL,
        _ => FAILURE,
    }
}

pub(crate) fn failure(error: Error) -> anyhow::Error {
    let status = of_error(&error);
    let diagnostic = format!("Error: {error}\n");
    match ExternalFailure::new(status, diagnostic) {
        Ok(external) => anyhow::Error::new(external.with_source(error)),
        Err(_) => anyhow::Error::new(error),
    }
}

pub(crate) fn status(result: &RunResult) -> u8 {
    match result {
        RunResult::Error(error) => of_run_error(error),
        RunResult::NoMatch(_) => FAILURE,
        _ => OK,
    }
}

fn of_run_error(error: &RunError) -> u8 {
    match error.kind() {
        RunErrorKind::External => error.exit_status().code(),
        _ => FAILURE,
    }
}

pub(crate) fn emit(result: &RunResult) {
    match result {
        RunResult::Handled(output) if output.is_empty() => {}
        RunResult::Handled(output) => println!("{output}"),
        RunResult::Error(error) if error.kind() == RunErrorKind::External => eprint!("{error}"),
        RunResult::Error(error) => eprintln!("{error}"),
        _ => {}
    }
}

#[cfg(test)]
#[path = "exit_tests.rs"]
mod tests;
