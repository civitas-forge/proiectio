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

/// Named for the contract rather than for a caller: Standout spends this one
/// itself, so only the tests that pin the contract read it.
#[cfg_attr(not(test), expect(dead_code, reason = "the tests pin the contract"))]
pub(crate) const OK: u8 = 0;
pub(crate) const FAILURE: u8 = 1;
pub(crate) const REFUSAL: u8 = 2;

/// What a command line clap rejects leaves with. Standout fixes it at 2 and
/// offers no seam to move it, so it is the number a refusal leaves with too;
/// before 12 this CLI owned the process edge and spent 1 on it. A caller
/// telling the two apart reads the output, where a refusal states its rows
/// and a usage error states clap's prose.
#[cfg_attr(not(test), expect(dead_code, reason = "the tests pin the contract"))]
pub(crate) const USAGE: u8 = 2;

pub(crate) fn of_error(error: &Error) -> u8 {
    match error {
        Error::Refused(_) => REFUSAL,
        _ => FAILURE,
    }
}

/// The library's error as this CLI hands it to Standout.
///
/// A message carries a filename, and a filename is data: every character a
/// terminal acts on leaves as an escape, so an OSC sequence a destination put
/// in a name is shown rather than run. Both arms carry [`Stated`], so the
/// escaping is the same one whichever seam the error leaves by.
///
/// An operational failure is an ordinary handler error, which is what
/// Standout already spends 1 on: it frames the message on stderr, or writes
/// the diagnostic document to stdout and leaves stderr alone under a
/// structured encoding. A refusal is the one status Standout would not choose
/// for itself, so it goes through [`AppFailure`], which pins the status and
/// the bytes; the cost is that those bytes reach stderr under every encoding.
pub(crate) fn failure(error: Error) -> anyhow::Error {
    let status = of_error(&error);
    let stated = Stated::over(error);
    if status != REFUSAL {
        return anyhow::Error::new(stated);
    }
    match AppFailure::new(REFUSAL, format!("Error: {stated}\n")) {
        Ok(app) => anyhow::Error::new(app.with_source(stated)),
        Err(_) => anyhow::Error::new(stated),
    }
}

/// Every other error this CLI reports, on the same terms: a clapfig message
/// quotes a key, a value or a path the invocation named, and those are data
/// too.
pub(crate) fn stated<E>(error: E) -> anyhow::Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    anyhow::Error::new(Stated::over(error))
}

/// A message this CLI writes about a run rather than as its output, as the
/// bytes it writes. Standout owns the process edge, so what a message quotes
/// has to arrive quoted: every character a terminal acts on leaves as an
/// escape, and an OSC sequence a destination put in a name is shown rather
/// than run. The line breaks a message spelled are its own layout.
pub(crate) fn warning(message: &str) -> String {
    crate::app::control_escaped_block(message)
}

/// An error stated as this CLI writes it, over the error itself as the source
/// a reader of the chain still gets.
#[derive(Debug)]
pub(crate) struct Stated {
    stated: String,
    source: Box<dyn std::error::Error + Send + Sync + 'static>,
}

impl Stated {
    fn over<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self {
            stated: warning(&error.to_string()),
            source: Box::new(error),
        }
    }
}

impl std::fmt::Display for Stated {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.stated)
    }
}

impl std::error::Error for Stated {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

#[cfg(test)]
#[path = "exit_tests.rs"]
mod tests;
