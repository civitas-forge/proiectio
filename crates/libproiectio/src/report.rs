use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use serde::Serialize;

use crate::Manifest;

/// What apply did to one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ApplyOutcome {
    /// The path did not exist and was created. For a
    /// [`Block`](crate::EntryKind::Block) entry the unit is the delimited
    /// region: the container file may have existed, the region did not.
    Written,
    /// The path existed and was replaced.
    Overwritten,
    /// Disk already matched desired; nothing was written.
    Skipped,
    /// The orphaned path was removed.
    Removed,
    /// This owner was dropped from the path's manifest entry; the disk was
    /// not touched because other owners still hold the path.
    Released,
}

/// What a successful apply run did: one outcome per path, and the
/// manifest as persisted at the end of the run.
///
/// A failed apply returns an [`Error`](crate::Error) alone — no report —
/// but the on-disk manifest still records the entries applied before the
/// error, so a partial run heals on re-run instead of classifying its own
/// writes as foreign.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApplyReport {
    /// Per-path outcomes, keyed by path relative to the destination.
    pub outcomes: BTreeMap<Utf8PathBuf, ApplyOutcome>,
    /// The manifest as written after the run.
    pub manifest: Manifest,
}
