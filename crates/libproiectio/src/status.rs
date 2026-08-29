use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use serde::Serialize;

/// The classification of one path in the union of the manifest and the
/// destination directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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

/// The classification of every path in the union of the manifest and the
/// destination directory, with nothing written. Non-UTF-8 entries on disk
/// stay outside the map; unrecorded directories read `Foreign`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Status {
    /// Per-path states, keyed by path relative to the destination.
    pub paths: BTreeMap<Utf8PathBuf, PathState>,
}

// These tests project through `Run`, so they carry `run`'s target predicate
// rather than the `cfg(unix)` on `Projection::status`.
#[cfg(all(
    test,
    unix,
    not(any(
        target_os = "espidf",
        target_os = "horizon",
        target_os = "solaris",
        target_os = "vita"
    ))
))]
#[path = "status_tests.rs"]
mod tests;
