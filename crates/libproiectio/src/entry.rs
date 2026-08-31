use serde::{Deserialize, Serialize};

/// Which end of the container a block region occupies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Placement {
    /// The region is the container's first bytes.
    Prepend,
    /// The region is the container's last bytes.
    Append,
}

/// The kind of a projected path, as recorded in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EntryKind {
    /// A regular file, written whole and hashed whole.
    File,
    /// A symbolic link; the target string is written verbatim.
    Symlink,
    /// A managed region at one end of a container file the projection does
    /// not own whole.
    ///
    /// [`Append`](Placement::Append) lays the container out as
    /// `author ++ marker ++ b"\n" ++ body`, [`Prepend`](Placement::Prepend) as
    /// `body ++ marker ++ b"\n" ++ author`.
    ///
    /// A marker occurrence is a whole line: anchored at a line start, matched
    /// byte-exact, terminated by `\n`, `\r\n`, or the end of the container. A
    /// line carrying the marker text indented or quoted is not one.
    ///
    /// Two or more marker lines in one container identify no region: the path
    /// classifies [`Drifted`](crate::PathState::Drifted) and every action on it
    /// refuses, [`Overwrite`](crate::DriftPolicy::Overwrite) included.
    ///
    /// The region runs to the container's edge, so bytes an author writes past
    /// that edge are inside it: they read as drift, and `Overwrite` discards
    /// them with the rest of the region.
    ///
    /// The body's hash covers its line terminators, so a container something
    /// rewrites — git under `text=auto` or `core.autocrlf` — drifts every run.
    Block {
        /// The line bounding the region on the inside, written verbatim and
        /// matched byte-exact. [`BlockFault`](crate::BlockFault) names what a
        /// marker and a body may not be.
        marker: String,
        placement: Placement,
    },
}

impl EntryKind {
    /// Whether this kind is a block.
    pub fn is_block(&self) -> bool {
        matches!(self, EntryKind::Block { .. })
    }
}

/// One node of the desired tree, keyed by its relative path in the
/// `BTreeMap<Utf8PathBuf, Entry>` the caller passes to `plan`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A regular file with the given contents.
    File { contents: Vec<u8>, executable: bool },
    /// A symbolic link, whose target string reaches disk verbatim.
    Symlink { target: String },
    /// A managed region at one end of a container the projection does not own
    /// whole. [`EntryKind::Block`] carries the region's rules.
    Block {
        /// The bytes inside the region. No line of it may equal `marker`.
        body: Vec<u8>,
        marker: String,
        placement: Placement,
    },
}

impl Entry {
    /// The [`EntryKind`] this entry is recorded as in the manifest.
    pub fn kind(&self) -> EntryKind {
        match self {
            Entry::File { .. } => EntryKind::File,
            Entry::Symlink { .. } => EntryKind::Symlink,
            Entry::Block {
                marker, placement, ..
            } => EntryKind::Block {
                marker: marker.clone(),
                placement: *placement,
            },
        }
    }
}

#[cfg(test)]
#[path = "entry_tests.rs"]
mod tests;
