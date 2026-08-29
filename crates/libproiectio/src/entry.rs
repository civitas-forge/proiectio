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
    Block {
        /// The line bounding the region on the inside, written verbatim and
        /// matched byte-exact.
        marker: String,
        /// Which end of the container the region occupies.
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Entry {
    /// A regular file with the given contents.
    File {
        /// The exact bytes to write.
        contents: Vec<u8>,
        /// Whether the executable bit is set on the written file.
        executable: bool,
    },
    /// A symbolic link, whose target string reaches disk verbatim.
    Symlink {
        /// The link target, written verbatim.
        target: String,
    },
    /// A managed region at one end of a container the projection does not own
    /// whole.
    Block {
        /// The bytes inside the region. No line of it may equal `marker`.
        body: Vec<u8>,
        /// The line bounding the region on the inside.
        marker: String,
        /// Which end of the container the region occupies.
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
