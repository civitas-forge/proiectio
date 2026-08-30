//! The 0/1/2 exit contract, which `main` owns because Standout spends 2
//! on a command line clap rejects and this CLI spends it on refusals.

use std::io::{ErrorKind, Write};

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

/// Writes the completed run and reports the status the process leaves with: a
/// reader that closed the stream is not a failure, any other write failure is.
pub(crate) fn emit(result: &RunResult) -> u8 {
    emit_to(
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
        result,
    )
}

fn emit_to(out: &mut impl Write, err: &mut impl Write, result: &RunResult) -> u8 {
    let status = status(result);
    let written = match result {
        RunResult::Handled(output) if output.is_empty() => Ok(()),
        RunResult::Handled(output) => write(out, output.as_str()),
        RunResult::Error(error) if error.kind() == RunErrorKind::External => {
            write(err, error.as_str())
        }
        RunResult::Error(error) => write(err, &format!("{error}\n")),
        _ => Ok(()),
    };
    match written {
        Ok(()) => status,
        Err(error) if error.kind() == ErrorKind::BrokenPipe => status,
        Err(_) => status.max(FAILURE),
    }
}

fn write(stream: &mut impl Write, text: &str) -> std::io::Result<()> {
    stream.write_all(text.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
#[path = "exit_tests.rs"]
mod tests;
