use serde::Serialize;

use crate::Report;

/// The classification of one path in the union of the manifest and the
/// destination directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum PathState {
    /// Disk matches the recorded entry: bytes, kind, and executable bit.
    Clean,
    /// Disk differs from the recorded entry — bytes, kind, or executable
    /// bit — a user edit.
    Drifted,
    /// Recorded, but gone from disk.
    Missing,
    /// On disk, absent from the manifest. Planning refuses to touch it,
    /// except that a [`Block`](crate::EntryKind::Block) is judged over its
    /// region: an unrecorded container is a write target, not a refusal.
    Foreign,
}

/// Every recorded path, plus every unrecorded non-directory path on disk;
/// the state subtree and non-UTF-8 entries stay outside the report.
pub type Status = Report<PathState>;

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
