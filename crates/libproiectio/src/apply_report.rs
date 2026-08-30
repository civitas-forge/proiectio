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
/// A run that stopped part-way reports the same rows for what it did apply,
/// inside the [`Aborted`] it fails with; the on-disk manifest records those
/// entries too, so a partial run heals on re-run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApplyReport {
    pub report: Report<ApplyOutcome>,
    /// Archive members `strip` erased on the way to the desired tree, which
    /// no row can state: they reached no path in the destination, so the run
    /// wrote nothing for them.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub dropped: BTreeSet<Dropped>,
    /// The manifest as written after the run.
    pub manifest: Manifest,
}

/// An apply that stopped before it reached the end of its plan: what stopped
/// it, and what it had already applied when it did.
///
/// A run applies its plan action by action, so a refusal or a failure met
/// part-way leaves the destination holding whatever the actions before it
/// wrote. Those rows are here rather than dropped, because a caller that
/// reports the error alone reports an untouched destination — which a run
/// that stopped part-way did not leave.
#[derive(Debug)]
pub struct Aborted {
    /// What stopped the run.
    pub error: Error,
    /// What the run applied before it stopped, and the manifest as persisted.
    /// The rows are empty where the run stopped before touching anything: the
    /// whole-plan check declined the plan, and the manifest is as it loaded.
    /// Boxed, so a `Result` over this carries an error the size of an error.
    pub applied: Box<ApplyReport>,
}

impl Aborted {
    /// Whether the run touched the destination before it stopped.
    pub fn applied_anything(&self) -> bool {
        !self.applied.report.is_empty()
    }
}

impl std::error::Error for Aborted {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl fmt::Display for Aborted {
    /// The error, and — where the run had already applied rows — the count it
    /// applied, so a message quoting this never reads as a run that stopped
    /// before it touched the destination.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.error)?;
        let applied = self.applied.report.rows.len();
        if applied > 0 {
            let paths = if applied == 1 { "path" } else { "paths" };
            write!(
                f,
                "; the run stopped part-way, having already applied {applied} {paths}"
            )?;
        }
        Ok(())
    }
}
