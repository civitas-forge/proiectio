//! The 0/1/2 exit contract, which `main` owns because Standout spends 2
//! on a command line clap rejects and this CLI spends it on refusals.

use std::cell::Cell;
use std::io::{ErrorKind, Write};
use std::rc::Rc;

use libproiectio::{Aborted, Error};
use standout::cli::{ArtifactRun, ExternalFailure, RunError, RunErrorKind, RunResult};

pub(crate) const OK: u8 = 0;
pub(crate) const FAILURE: u8 = 1;
pub(crate) const REFUSAL: u8 = 2;

/// The status a run that rendered its output nonetheless leaves with.
///
/// A Standout handler either renders and succeeds or fails with its output
/// replaced by the diagnostic. A refused dry run is neither: the plan it
/// refuses is the whole point of the run, so the handler renders it and
/// records the refusal here, and `main` reads the cell back once the run has
/// been written. The composition root owns one and the app holds a clone.
#[derive(Clone, Default)]
pub(crate) struct Verdict(Rc<Cell<u8>>);

impl Verdict {
    /// Records `status`, keeping whichever of it and the recorded one is
    /// greater.
    pub(crate) fn record(&self, status: u8) {
        self.0.set(self.0.get().max(status));
    }

    /// The status the process leaves with, over what emitting the run
    /// reported: a failed write still raises the run, and never lowers a
    /// recorded refusal.
    pub(crate) fn over(&self, emitted: u8) -> u8 {
        emitted.max(self.0.get())
    }
}

pub(crate) fn of_error(error: &Error) -> u8 {
    match error {
        Error::Refused(_) => REFUSAL,
        _ => FAILURE,
    }
}

pub(crate) fn failure(error: Error) -> anyhow::Error {
    external(of_error(&error), error)
}

/// A run that stopped part-way and has no document to render fails with what
/// it had already applied named in the diagnostic, so a partial destination
/// never reads as an untouched one.
pub(crate) fn stopped(aborted: Aborted) -> anyhow::Error {
    external(of_error(&aborted.error), aborted)
}

fn external<E: std::error::Error + Send + Sync + 'static>(status: u8, error: E) -> anyhow::Error {
    let diagnostic = format!("Error: {error}");
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

const NO_COMMAND: &str = "Error: no command matched the command line\n";
const UNSUPPORTED: &str = "Error: this build cannot write the output the run produced\n";

/// Writes the completed run and reports the status the process leaves with: a
/// reader that closed the stream is not a failure, any other write failure is.
///
/// `run_to_string` collects the framework's warnings instead of printing them,
/// so this drains them to stderr after the run's own output.
pub(crate) fn emit(result: &RunResult) -> u8 {
    emit_to(
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
        result,
        &standout::warnings::take_captured_warnings(),
    )
}

fn emit_to(
    out: &mut impl Write,
    err: &mut impl Write,
    result: &RunResult,
    warnings: &[String],
) -> u8 {
    let status = status(result);
    // `RunResult` is `#[non_exhaustive]`, so the wildcard cannot be dropped;
    // it reports a variant this build does not know how to write rather than
    // dropping the output and exiting 0.
    let written = match result {
        RunResult::Handled(output) if output.is_empty() => Ok(()),
        RunResult::Handled(output) => write(out, output.as_str()),
        RunResult::Binary(bytes, _) => write_bytes(out, bytes),
        RunResult::Artifact(run) => write_artifact(out, err, run),
        RunResult::Silent => Ok(()),
        RunResult::Error(error) => diagnostic(err, error.as_str()),
        RunResult::NoMatch(_) => write(err, NO_COMMAND),
        _ => {
            let _ = write(err, UNSUPPORTED);
            return status.max(FAILURE);
        }
    }
    .and_then(|()| warn(err, warnings));
    match written {
        Ok(()) => status,
        Err(error) if error.kind() == ErrorKind::BrokenPipe => status,
        Err(_) => status.max(FAILURE),
    }
}

/// Bytes the framework already wrote to a file are on disk; bytes bound for
/// stdout still need this writer. The report follows on whichever of the two
/// streams the bytes did not take, which is what Standout's own writer does.
fn write_artifact(
    out: &mut impl Write,
    err: &mut impl Write,
    run: &ArtifactRun,
) -> std::io::Result<()> {
    let to_stdout = run.destination().is_stdout();
    if to_stdout {
        write_bytes(out, run.bytes())?;
    }
    match run.report().filter(|report| !report.is_empty()) {
        Some(report) if to_stdout => diagnostic(err, report),
        Some(report) => diagnostic(out, report),
        None => Ok(()),
    }
}

fn warn(err: &mut impl Write, warnings: &[String]) -> std::io::Result<()> {
    for warning in warnings {
        diagnostic(err, &format!("Warning: {warning}"))?;
    }
    Ok(())
}

/// What this CLI writes about a run rather than as its output. A message
/// carries a filename, and a filename is data: every character a terminal acts
/// on leaves as an escape, so an OSC sequence a destination put in a name is
/// shown rather than run. The line breaks clap spelled are the message's own
/// layout; the terminator is this CLI's, and there is exactly one.
fn diagnostic(stream: &mut impl Write, text: &str) -> std::io::Result<()> {
    let escaped = crate::app::control_escaped_block(text.trim_end_matches('\n'));
    write(stream, &format!("{escaped}\n"))
}

fn write(stream: &mut impl Write, text: &str) -> std::io::Result<()> {
    write_bytes(stream, text.as_bytes())
}

fn write_bytes(stream: &mut impl Write, bytes: &[u8]) -> std::io::Result<()> {
    stream.write_all(bytes)?;
    stream.flush()
}

#[cfg(test)]
#[path = "exit_tests.rs"]
mod tests;
