use std::collections::{BTreeMap, BTreeSet};

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

use crate::EntryKind;

/// The manifest format version this crate writes and accepts.
pub const MANIFEST_VERSION: u32 = 1;

/// The manifest's file name inside the caller-chosen state directory.
pub const MANIFEST_FILE_NAME: &str = "manifest.json";

/// The single-writer lock file's name, the state directory's other file
/// (`docs/implementation.lex` section 7). It sits here, beside
/// [`MANIFEST_FILE_NAME`] and outside the `flock(2)`-gated `lock` module, so
/// every target can name the file: [`Error::LockHeld`](crate::Error::LockHeld)
/// reports it everywhere, and a caller on a target that builds no `Run`
/// still knows which file proiectio's own runs contend on.
pub const LOCK_FILE_NAME: &str = "proiectio.lock";

/// The recorded state of a projection: one JSON file in a caller-chosen
/// state directory, mapping each projected path to what was last written
/// there.
///
/// The manifest stores, per path, the SHA-256 of the bytes last written — a
/// hash rather than the bytes, because the caller can always recompute
/// desired content for a diff, and a projected secret is never copied into
/// state. It round-trips through JSON, and `BTreeMap` keeps the
/// serialization stable and diffable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Format version; see [`MANIFEST_VERSION`].
    pub version: u32,
    /// Every path the projection owns, keyed by path relative to the
    /// destination.
    pub entries: BTreeMap<Utf8PathBuf, ManifestEntry>,
}

impl Manifest {
    /// An empty manifest at the current [`MANIFEST_VERSION`].
    pub fn new() -> Self {
        Manifest {
            version: MANIFEST_VERSION,
            entries: BTreeMap::new(),
        }
    }
}

impl Default for Manifest {
    /// Same as [`Manifest::new`]: an empty manifest at the current
    /// [`MANIFEST_VERSION`], never version 0.
    fn default() -> Self {
        Manifest::new()
    }
}

/// What the manifest records for one projected path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// What kind of node was written.
    pub kind: EntryKind,
    /// Lowercase hex SHA-256 of the bytes last written: the file contents,
    /// the symlink target string, or — for [`EntryKind::Block`] — the
    /// region's body alone.
    pub hash: String,
    /// Whether the written file carries the executable bit. Always `false`
    /// for symlinks and blocks: a block's container keeps the author's mode,
    /// which this field says nothing about.
    pub executable: bool,
    /// The opaque owner names holding this path. The crate never interprets
    /// them; two owners may hold one path only while writing identical
    /// bytes.
    pub owners: BTreeSet<String>,
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;
