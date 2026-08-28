use serde::{Deserialize, Serialize};

/// The kind of a projected path, as recorded in the manifest.
///
/// Every desired-tree [`Entry`] and every [`ManifestEntry`](crate::ManifestEntry)
/// is one of these three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EntryKind {
    /// A regular file, written whole and hashed whole.
    File,
    /// A symbolic link; the target string is written verbatim.
    Symlink,
    /// A delimited managed region inside a file the projection does not own
    /// whole. Only the body between the delimiter lines is written and
    /// hashed, so an edit elsewhere in the file never reads as drift.
    ///
    /// The delimiter lines are proiectio's own fixed markers, not caller
    /// data. The container file may already exist and be otherwise
    /// unmanaged: the projection owns the region, so the surrounding file
    /// being unrecorded does not make the path foreign (see
    /// [`Error::Foreign`](crate::Error::Foreign)). A `Block` entry never
    /// owns its container whole — removal strips the region and leaves the
    /// file; deleting a whole file on removal is what [`File`](Self::File)
    /// entries do.
    Block,
}

/// One node of the desired tree, keyed by its relative path in the
/// `BTreeMap<Utf8PathBuf, Entry>` the caller passes to `plan`.
///
/// Contents are opaque bytes: the crate never interprets what it writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Entry {
    /// A regular file with the given contents.
    File {
        /// The exact bytes to write.
        contents: Vec<u8>,
        /// Whether the executable bit is set on the written file.
        executable: bool,
    },
    /// A symbolic link. The target string reaches disk verbatim, and is
    /// resolved from the link's parent directory through the destination's
    /// own links, purely to classify it as in-dest or external — at plan
    /// time, and again against the disk before the link is published.
    Symlink {
        /// The link target, written verbatim.
        target: String,
    },
    /// A delimited managed region inside a shared file: apply locates the
    /// projection's delimiter lines — the crate's own fixed markers, not
    /// caller data — and replaces only the body between them.
    Block {
        /// The bytes between the delimiter lines. The manifest hash covers
        /// the body alone. The marker format — and with it how a body that
        /// itself contains marker lines is handled — is specified where
        /// apply is implemented, not by this type.
        body: Vec<u8>,
    },
}

impl Entry {
    /// The [`EntryKind`] this entry is recorded as in the manifest.
    pub fn kind(&self) -> EntryKind {
        match self {
            Entry::File { .. } => EntryKind::File,
            Entry::Symlink { .. } => EntryKind::Symlink,
            Entry::Block { .. } => EntryKind::Block,
        }
    }
}

#[cfg(test)]
#[path = "entry_tests.rs"]
mod tests;
