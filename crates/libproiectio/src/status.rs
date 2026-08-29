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
    /// except that a [`Block`](crate::EntryKind::Block) is judged over its
    /// region, so an unrecorded container is a write target rather than a
    /// refusal (`docs/design.lex` section 2).
    Foreign,
}

/// The classification of every path in the union of the manifest and the
/// destination directory, with nothing written.
///
/// Classification covers what UTF-8 can name: a non-UTF-8 entry on disk
/// can never match a desired or recorded path, so it stays outside this
/// map — never overwritten, never removed, and a directory holding one
/// is never pruned.
///
/// Directories classify like anything else, which surprises readers: the
/// manifest records none, so every directory the walk meets is unrecorded
/// and reads [`Foreign`](PathState::Foreign) — including the parents a past
/// run created for the owned files inside them, which read
/// [`Clean`](PathState::Clean) beneath a foreign parent. A row carries that
/// relationship and not the kind of node standing there, so nothing here
/// separates a foreign directory from a foreign file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Status {
    /// Per-path states, keyed by path relative to the destination.
    pub paths: BTreeMap<Utf8PathBuf, PathState>,
}

// These tests record what a read reports after a write pass, so they project
// through `Run` and carry the same target predicate `run` does — narrower
// than the `cfg(unix)` on `Projection::status` itself, which they would
// otherwise be compiled against a `begin` that does not exist there.
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
