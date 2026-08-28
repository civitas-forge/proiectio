use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use serde::Serialize;

/// The classification of one path in the union of the manifest and the
/// destination directory.
///
/// Planning runs this classification too, then compares against the
/// desired tree to choose each path's [`Action`](crate::Action); status
/// needs no desired tree, so a path only the desired tree names has no
/// state here — it first appears in a [`Plan`](crate::Plan) as a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PathState {
    /// Disk matches the recorded entry: bytes, kind, and executable bit.
    Clean,
    /// Disk differs from the recorded entry — bytes, kind, or executable
    /// bit — a user edit.
    Drifted,
    /// Recorded, but gone from disk.
    Missing,
    /// On disk, absent from the manifest. Planning refuses to touch it —
    /// except where a desired [`Block`](crate::EntryKind::Block) entry
    /// owns only the delimited region inside it, in which case the
    /// container stays foreign as a file while the region is a write
    /// target.
    Foreign,
}

/// The classification of every path in the union of the manifest and the
/// destination directory, with nothing written.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Status {
    /// Per-path states, keyed by path relative to the destination.
    pub paths: BTreeMap<Utf8PathBuf, PathState>,
}
