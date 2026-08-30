use std::collections::BTreeSet;

use serde::Serialize;

use crate::{Dropped, Manifest, Report};

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

/// What a successful apply run did: one outcome per path, and the manifest as
/// persisted at the end of the run.
///
/// A failed apply returns an [`Error`](crate::Error) and no report; the
/// on-disk manifest still records the entries applied before the error, so a
/// partial run heals on re-run.
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
