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

impl Status {
    /// Whether every row the report holds is [`Clean`](PathState::Clean): no
    /// recorded path edited, none gone, and no unrecorded file on disk. What
    /// the report leaves out — an unrecorded directory holding nothing, a
    /// non-UTF-8 entry, the state subtree — is no row here either, and so
    /// cannot make a destination unclean.
    ///
    /// A report of no rows is clean, and an empty manifest agrees with an
    /// empty destination, so this cannot tell a destination nothing was ever
    /// projected onto from one every owner has released.
    pub fn is_clean(&self) -> bool {
        self.rows.values().all(|row| match row.verdict {
            PathState::Clean => true,
            PathState::Drifted | PathState::Missing | PathState::Foreign => false,
        })
    }
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
