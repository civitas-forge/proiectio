use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use serde::Serialize;

use crate::Manifest;

/// What apply did to one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ApplyOutcome {
    /// The path did not exist and was created; for a block entry, the region
    /// did not exist.
    Written,
    /// The path existed and was replaced.
    Overwritten,
    /// Disk already matched desired; nothing was written.
    Skipped,
    /// The orphaned path was removed.
    Removed,
    /// This owner was dropped from the path's manifest entry; other owners
    /// still hold the path, so the disk was not touched.
    Released,
}

/// What a successful apply run did: one outcome per path, and the manifest as
/// persisted at the end of the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApplyReport {
    /// Per-path outcomes, keyed by path relative to the destination.
    pub outcomes: BTreeMap<Utf8PathBuf, ApplyOutcome>,
    /// The manifest as written after the run.
    pub manifest: Manifest,
}
