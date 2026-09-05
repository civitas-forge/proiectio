//! The 0/1/2 exit contract, which this module owns because Standout spends 2
//! on a command line clap rejects and this CLI spends it on refusals.
//!
//! Two seams carry it. A run that could not act fails with an [`AppFailure`],
//! which pins both the status and the verbatim stderr bytes. A run that
//! refused but still rendered its plan — the refusal *is* the output — keeps
//! [`crate::handlers`]'s `Output::with_exit_status`, which leaves the outcome
//! a success and changes only the status the process leaves with.

use libproiectio::Error;
use standout::cli::AppFailure;

pub(crate) const OK: u8 = 0;
pub(crate) const FAILURE: u8 = 1;
pub(crate) const REFUSAL: u8 = 2;

/// What a command line clap rejects leaves with. Standout fixes it at 2 and
/// offers no seam to move it, so it is the number a refusal leaves with too;
/// before 12 this CLI owned the process edge and spent 1 on it. A caller
/// telling the two apart reads the output, where a refusal states its rows
/// and a usage error states clap's prose.
pub(crate) const USAGE: u8 = 2;

pub(crate) fn of_error(error: &Error) -> u8 {
    match error {
        Error::Refused(_) => REFUSAL,
        _ => FAILURE,
    }
}

/// The library's error as the bytes this CLI writes about it. A message
/// carries a filename, and a filename is data: every character a terminal acts
/// on leaves as an escape, so an OSC sequence a destination put in a name is
/// shown rather than run. The terminator is this CLI's, and there is exactly
/// one, because Standout writes an `AppFailure`'s diagnostic verbatim.
pub(crate) fn failure(error: Error) -> anyhow::Error {
    let status = of_error(&error);
    let stated = crate::app::control_escaped_block(&error.to_string());
    match AppFailure::new(status, format!("Error: {stated}\n")) {
        Ok(app) => anyhow::Error::new(app.with_source(error)),
        Err(_) => anyhow::Error::new(error),
    }
}

#[cfg(test)]
#[path = "exit_tests.rs"]
mod tests;
