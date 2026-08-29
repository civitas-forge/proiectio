use serde::{Deserialize, Serialize};

/// Which end of the container a [`Block`](EntryKind::Block) region occupies.
///
/// Never the middle: fixing the region to an end is what lets one marker line
/// bound it, since the file's own edge bounds the other side.
/// [`EntryKind::Block`] carries the layout and the tradeoff between the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Placement {
    /// The region is the container's first bytes.
    Prepend,
    /// The region is the container's last bytes.
    Append,
}

/// The kind of a projected path, as recorded in the manifest.
///
/// Every desired-tree [`Entry`] and every [`ManifestEntry`](crate::ManifestEntry)
/// is one of these three.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EntryKind {
    /// A regular file, written whole and hashed whole.
    File,
    /// A symbolic link; the target string is written verbatim.
    Symlink,
    /// A managed region at one end of a *container* file the projection does
    /// not own whole — the caller's bytes inside a file somebody else wrote
    /// and keeps, such as a shell rc file under source control.
    ///
    /// For a block, every rule this crate states about "the node at a path"
    /// means the region rather than the container: `Clean`, `Drifted`,
    /// `Missing`, `Foreign`, apply's signature re-check, and removal all read
    /// that way (`docs/design.lex` section 2). The manifest hash covers the
    /// body alone, so an edit anywhere on the author's side of the marker is
    /// invisible to every comparison, and removal strips the region and
    /// leaves the container standing — even when the strip empties it.
    ///
    /// # Layout
    ///
    /// One caller-supplied `marker` line bounds the region on the inside; the
    /// file's own start or end bounds it on the outside:
    ///
    /// ```text
    /// Append:   author ++ marker ++ b"\n" ++ body
    /// Prepend:  body ++ marker ++ b"\n" ++ author
    /// ```
    ///
    /// One marker suffices where every surveyed tool uses two because the
    /// body may carry no line equal to the marker: the projection's own
    /// marker is then necessarily the last occurrence for
    /// [`Append`](Placement::Append) and the first for
    /// [`Prepend`](Placement::Prepend), and there is no duplicate-marker case
    /// to refuse.
    ///
    /// A marker *occurrence* is a whole line: anchored at a line start,
    /// matched byte-exact, and terminated by `\n`, `\r\n`, or the end of the
    /// file. An indented or quoted line carrying the marker text is not an
    /// occurrence, which is what lets a container discuss its own marker.
    ///
    /// # Why the marker exists at all
    ///
    /// Appending without one is not idempotent: a second run cannot tell last
    /// run's bytes from the file's own, so it appends again. Making repeated
    /// application safe and removal exact is the marker's whole job. nvm,
    /// rbenv and Homebrew's `shellenv` probe for a substring instead of
    /// delimiting, and consequently none of them can update or uninstall its
    /// own injection.
    ///
    /// # What the caller must supply
    ///
    /// The marker is the caller's because only the caller knows what is inert
    /// in the container's language — `#`, `//`, `;`, `<!-- -->`. Refused at
    /// plan time ([`Error::Block`](crate::Error::Block)):
    ///
    /// - a marker that is empty, that carries a `\n` or a `\r`, or that
    ///   begins or ends with a space or a tab. The whitespace rule is not
    ///   fastidiousness: editors and formatters strip trailing whitespace on
    ///   save, a stripped marker is one no read finds again, and the next run
    ///   then writes a second region while the save strips *that* marker —
    ///   the file grows a stranded body per cycle;
    /// - a body carrying a line equal to the marker, which would write a
    ///   container that cannot be read back. mise refuses the same thing for
    ///   the same reason; Ansible does not, and its absence is the root of
    ///   ansible/ansible#43523 and #47192.
    ///
    /// # Terminators, and what is never normalized
    ///
    /// The projection writes `\n` after the marker and accepts `\n`, `\r\n`,
    /// or end of file when reading it, so a line-ending conversion never
    /// makes the region unfindable. The body is a different matter and the
    /// tolerance stops there: the manifest hash covers the body's bytes
    /// exactly, terminators included, so a container under `text=auto`
    /// normalization drifts on every run. A block does not belong in one.
    ///
    /// Neither side's bytes are normalized to make room for the other.
    /// [`Append`](Placement::Append) requires the author's side — the
    /// container with any existing region stripped — to be empty or to end
    /// with `\n`; [`Prepend`](Placement::Prepend) requires the `body` to be
    /// empty or to end with `\n`. Both refuse otherwise. Ansible instead
    /// appends a newline to the author's last line, editing a byte outside
    /// the block in order to insert the block, which makes
    /// insert-then-strip lose a byte the author never had.
    ///
    /// # Which end to choose
    ///
    /// The region runs to the file's edge, so an author who writes past that
    /// edge has written inside the region: it reads as
    /// [`Drifted`](crate::PathState::Drifted) and refuses, and
    /// [`DriftPolicy::Overwrite`](crate::DriftPolicy::Overwrite) discards
    /// those bytes along with the rest of the region. No body size mitigates
    /// this. [`Prepend`](Placement::Prepend) is therefore the safer placement
    /// for a container people append to — which is most of them.
    ///
    /// # One marker line, or none of it works
    ///
    /// A container may hold `marker` as a whole line exactly once. The region
    /// is found by taking the last occurrence (the first, for
    /// [`Prepend`](Placement::Prepend)), which is the projection's own only
    /// while every other occurrence is a line outside the region — and the
    /// body may carry none, so a container the projection alone has written
    /// holds exactly one.
    ///
    /// A second bare marker line is somebody else's, and the marker is the
    /// whole of a region's identity, so nothing then says which of the two
    /// bounds the projection's bytes. A line the author wrote above an
    /// `Append` region and a copy of the region below it are the same picture
    /// from the projection's side, and the harmless one cannot be told from
    /// the ruinous one: acting on a guess would republish the container with
    /// the other region still in it and the manifest recording only one — a
    /// managed body nothing owns, per marker line, which is the growth the
    /// marker exists to prevent.
    ///
    /// So such a container identifies no region: it reads
    /// [`Drifted`](crate::PathState::Drifted) whatever its extreme occurrence
    /// holds, nothing skips, and
    /// [`DriftPolicy::Overwrite`](crate::DriftPolicy::Overwrite) does not
    /// lift it. Every action refuses until the extra line is gone. Writing
    /// the marker text indented or quoted is what lets a container mention it
    /// without this, since neither is an occurrence.
    ///
    /// # Bounds
    ///
    /// One region per path and one projection per container: two regions in
    /// one file, and two projections sharing a container, are both excluded —
    /// the caller concatenates into one body, or owns the file whole. A path
    /// never changes between [`File`](Self::File) and `Block` in either
    /// direction, and a block never creates its container, so the manifest
    /// never owns a container whole and removal never deletes one.
    ///
    /// # When not to use one
    ///
    /// A block is for adding to a file somebody else owns and keeps. Prefer a
    /// whole [`File`](Self::File) entry in a drop-in directory (`conf.d`,
    /// `sudoers.d`, ssh `Include`) wherever the application supports one, and
    /// prefer assembling the file in the caller and projecting the result
    /// wherever the caller has all the content. Do not put a block in a
    /// container subject to line-ending normalization, in one that is a mount
    /// point, or in a structured format — JSON, YAML, TOML and XML do not
    /// concatenate, and this feature does not parse.
    Block {
        /// The line bounding the region on the inside, written verbatim and
        /// matched byte-exact. Two owners share a path only while agreeing on
        /// it, since it is part of the recorded kind.
        marker: String,
        /// Which end of the container the region occupies.
        placement: Placement,
    },
}

impl EntryKind {
    /// Whether this kind is a [`Block`](Self::Block) — the one distinction a
    /// path never crosses.
    pub fn is_block(&self) -> bool {
        matches!(self, EntryKind::Block { .. })
    }
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
    /// A managed region at one end of a container the projection does not own
    /// whole. [`EntryKind::Block`] carries the rules: the layout, what the
    /// marker must be, what is never normalized, and which end to choose.
    Block {
        /// The bytes inside the region. The manifest hash covers these alone,
        /// terminators included, so an edit outside the region is never
        /// drift. No line of it may equal `marker`.
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
