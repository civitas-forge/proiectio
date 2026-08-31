use std::collections::{BTreeMap, BTreeSet};

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

use crate::EntryKind;

/// The manifest format version this crate writes and accepts.
pub const MANIFEST_VERSION: u32 = 1;

/// The manifest's file name inside the caller-chosen state directory.
pub const MANIFEST_FILE_NAME: &str = "manifest.json";

/// The single-writer lock file's name, beside [`MANIFEST_FILE_NAME`] in the
/// state directory.
pub const LOCK_FILE_NAME: &str = "proiectio.lock";

/// The one rule an owner keeps, wherever the name comes from. Owners are
/// opaque otherwise: the crate records the name verbatim and never reads it.
pub const OWNER_RULE: &str =
    "an owner names a producer in the manifest, and neither an empty nor a blank string names one";

/// Whether `owner` names one, per [`OWNER_RULE`]. The name a manifest records
/// is the name a removal has to spell back and a listing prints, and neither
/// an empty nor a blank string is one a reader of that file can see.
pub fn names_an_owner(owner: &str) -> bool {
    !owner.trim().is_empty()
}

/// [`OWNER_RULE`] as the planning entry points enforce it, which is where a
/// name first reaches the manifest.
pub(crate) fn require_owner(owner: &str) -> crate::Result<()> {
    match names_an_owner(owner) {
        true => Ok(()),
        false => Err(crate::Error::OwnerNotNamed {
            owner: owner.to_owned(),
        }),
    }
}

/// The recorded state of a projection: one JSON file in a caller-chosen state
/// directory, mapping each projected path to what was last written there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Format version; see [`MANIFEST_VERSION`].
    pub version: u32,
    /// Every path the projection owns, relative to the destination.
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
    /// the symlink target string, or a block's region body alone.
    pub hash: String,
    /// Whether the written file carries the executable bit; always `false`
    /// for symlinks and blocks.
    pub executable: bool,
    /// The opaque owner names holding this path.
    pub owners: BTreeSet<String>,
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;
