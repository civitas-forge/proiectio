use std::collections::BTreeSet;
use std::fmt;

use serde::Serialize;

use crate::{Dropped, Error, Manifest, Report};

/// What apply did to one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum ApplyOutcome {
    /// The path did not exist and was created; for a block entry, the region
    /// did not exist.
    Written,
    /// The path existed and was replaced.
    Overwritten,
    /// Disk already matched desired; nothing was written. A planned write
    /// reports this too where it found the region already carrying the
    /// desired body and adopted it.
    Skipped,
    /// The orphaned path was removed.
    Removed,
    /// The record was dropped and nothing was unlinked: the path was already
    /// gone when the plan was decided. Directories the absent path left empty
    /// are pruned all the same.
    Forgot,
    /// This owner was dropped from the path's manifest entry; other owners
    /// still hold the path, so the disk was not touched.
    Released,
    /// The removal named the path and this owner did not hold it; no record
    /// changed and the disk was not touched.
    NotRecorded,
}

/// What an apply run did: one outcome per path, and the manifest as persisted
/// at the end of the run.
///
/// A run that could not finish reports the same rows for what it did apply,
/// inside the [`Aborted`] it fails with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApplyReport {
    pub report: Report<ApplyOutcome>,
    /// Archive members `strip` erased on the way to the desired tree, which
    /// no row can state: they reached no path in the destination, so the run
    /// wrote nothing for them.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub dropped: BTreeSet<Dropped>,
    /// The manifest the run decided on; whether the state directory holds it
    /// is [`Stopped::recorded`]'s to say.
    pub manifest: Manifest,
}

/// Why an apply could not finish, and whether the state directory came out of
/// it recording what the destination holds.
///
/// A run writes the destination action by action and records the whole lot in
/// one manifest at the end, so the two can fail independently: an action can
/// stop the run with the rows before it recorded, and the record can fail
/// with every action applied. Both failures are kept, because either one
/// alone describes a destination the run did not leave — so a caller that
/// wants one error takes the variant apart and says which it is dropping,
/// rather than asking this type to choose.
#[derive(Debug)]
pub enum Stopped {
    /// An action refused or failed, so the actions after it never ran. The
    /// state directory records the rows that did apply.
    Applying(Error),
    /// An action refused or failed, and writing the manifest that records the
    /// rows before it failed as well. The destination holds writes nothing on
    /// disk records, so a later run reads them as foreign rather than healing
    /// them.
    ApplyingAndRecording {
        /// What stopped the run.
        applying: Error,
        /// What stopped the record.
        recording: Error,
    },
    /// Every action applied and writing the manifest that records them
    /// failed, leaving the destination whole and unrecorded.
    Recording(Error),
}

impl Stopped {
    /// What stopped the run: the action that refused or failed, and — where
    /// every action applied — the failure to record them.
    pub fn error(&self) -> &Error {
        match self {
            Stopped::Applying(error) | Stopped::Recording(error) => error,
            Stopped::ApplyingAndRecording { applying, .. } => applying,
        }
    }

    /// What stopped the manifest reaching the state directory, where writing
    /// it failed.
    pub fn recording(&self) -> Option<&Error> {
        match self {
            Stopped::Applying(_) => None,
            Stopped::ApplyingAndRecording { recording, .. } => Some(recording),
            Stopped::Recording(error) => Some(error),
        }
    }

    /// Whether the state directory records what the run applied.
    pub fn recorded(&self) -> bool {
        matches!(self, Stopped::Applying(_))
    }
}

/// An apply that could not finish: why it stopped, and what it had applied
/// when it did.
///
/// A run applies its plan action by action, so a refusal or a failure met
/// part-way leaves the destination holding whatever the actions before it
/// wrote. Those rows are here rather than dropped, because a caller that
/// reports the error alone reports an untouched destination — which a run
/// that stopped part-way did not leave.
#[derive(Debug)]
pub struct Aborted {
    /// Why the run stopped, and whether the state directory records what it
    /// applied.
    pub stopped: Stopped,
    /// What the run applied before it stopped. The rows are empty where the
    /// run stopped before touching anything: the whole-plan check declined
    /// the plan, and the manifest is as it loaded.
    pub applied: ApplyReport,
}

impl Aborted {
    /// Whether the run touched the destination before it stopped.
    pub fn applied_anything(&self) -> bool {
        !self.applied.report.is_empty()
    }
}

impl std::error::Error for Aborted {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.stopped.error())
    }
}

impl fmt::Display for Aborted {
    /// What stopped the run, then how far it got and whether the state
    /// directory records that.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let applied = self.applied.report.rows.len();
        let paths = if applied == 1 { "path" } else { "paths" };
        match &self.stopped {
            Stopped::Applying(error) => {
                write!(f, "{error}")?;
                if applied > 0 {
                    write!(
                        f,
                        "; the run stopped part-way, having already applied {applied} {paths}"
                    )?;
                }
            }
            Stopped::ApplyingAndRecording {
                applying,
                recording,
            } => write!(
                f,
                "{applying}; the run stopped part-way, having applied {applied} {paths} that \
                 the state directory does not record: {recording}"
            )?,
            Stopped::Recording(error) => write!(
                f,
                "the run applied {applied} {paths} and could not record them: {error}"
            )?,
        }
        Ok(())
    }
}
