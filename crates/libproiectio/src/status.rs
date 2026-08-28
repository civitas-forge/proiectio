use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use serde::Serialize;

/// The classification of one path in the union of the desired tree, the
/// manifest, and the destination directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PathState {
    /// Disk matches the recorded hash.
    Clean,
    /// Disk differs from the recorded hash — a user edit.
    Drifted,
    /// Recorded, but gone from disk.
    Missing,
    /// On disk, absent from the manifest. Never touched.
    Foreign,
}

/// The classification of every recorded path, with nothing written.
///
/// Status runs the same classification planning does and stops there.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Status {
    /// Per-path states, keyed by path relative to the destination.
    pub paths: BTreeMap<Utf8PathBuf, PathState>,
}
