use std::collections::{BTreeMap, BTreeSet};

use camino::Utf8PathBuf;
use thiserror::Error;

use crate::Origin;

/// The crate-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

/// How deep below its root a directory walk descends, counted in directories.
/// Measured on a debug build, a walk exhausts a 2 MiB stack past ~200 levels.
/// A bind mount making a directory its own ancestor gives a tree no bottom,
/// and no symlink check sees that one: every level of it is a real directory.
pub const MAX_WALK_DEPTH: usize = 64;

/// Everything the engine can fail with.
///
/// [`Error::is_refusal`] splits refusals from runtime failures, which a CLI's
/// 0/1/2 exit contract falls out of:
///
/// ```
/// # use libproiectio::{Error, Result};
/// fn exit_code(result: Result<()>) -> i32 {
///     match result {
///         Ok(()) => 0,
///         Err(error) if error.is_refusal() => 2,
///         Err(_) => 1,
///     }
/// }
/// ```
#[derive(Debug, Error)]
pub enum Error {
    /// Refusal: recorded paths whose state on disk differs from the recorded
    /// entry — bytes, kind, or executable bit.
    #[error("refusing to touch drifted paths (edited on disk): {}", join(paths))]
    Drift {
        /// The drifted paths, relative to the destination.
        paths: BTreeSet<Utf8PathBuf>,
    },
    /// Refusal: paths on disk that the manifest does not record. For a
    /// [`Block`](crate::EntryKind::Block) entry the region is judged, not the
    /// container around it.
    #[error(
        "refusing to touch foreign paths (not written by this projection): {}",
        join(paths)
    )]
    Foreign {
        /// The foreign paths, relative to the destination.
        paths: BTreeSet<Utf8PathBuf>,
    },
    /// Refusal: locations the projection may not act at — paths refused by
    /// [`contained_join`](crate::contained_join), paths resolving through a
    /// symlinked ancestor, and paths overlapping the state directory.
    #[error(
        "refusing paths that violate containment{}: {}",
        note(origin),
        join(paths)
    )]
    Containment {
        /// The offending locations, spelled as whatever named them.
        paths: BTreeSet<Utf8PathBuf>,
        /// Where the tree holding them came from.
        origin: Origin,
    },
    /// Refusal: the desired entry for a path — bytes, kind, or executable bit
    /// — differs from what another owner holds there.
    #[error(
        "refusing paths whose desired entries conflict with another owner's: {}",
        join_conflicts(conflicts)
    )]
    OwnerConflict {
        /// The conflicting paths, relative to the destination, each mapped
        /// to the other owners holding it.
        conflicts: BTreeMap<Utf8PathBuf, BTreeSet<String>>,
    },
    /// Refusal: desired symlinks whose targets, resolved from each link's
    /// parent, land outside the destination. Lifted by
    /// [`ExternalTargetPolicy::Allow`](crate::ExternalTargetPolicy::Allow).
    #[error(
        "refusing symlinks with targets outside the destination{}: {}",
        note(origin),
        join_links(links)
    )]
    ExternalTarget {
        /// The offending links: path of each link, relative to the
        /// destination, mapped to its target string verbatim.
        links: BTreeMap<Utf8PathBuf, String>,
        /// Where the tree holding them came from.
        origin: Origin,
    },
    /// Refusal: desired symlinks whose targets are not pathnames on any host
    /// — the empty string, or a string carrying a NUL byte.
    #[error(
        "refusing symlinks whose targets are not paths{}: {}",
        note(origin),
        join_invalid(links)
    )]
    InvalidTarget {
        /// The offending links: path of each link, relative to the
        /// destination, mapped to its target string verbatim.
        links: BTreeMap<Utf8PathBuf, String>,
        /// Where the tree holding them came from.
        origin: Origin,
    },
    /// Refusal: desired keys claiming one on-disk location more than once —
    /// two keys normalizing to the same path, or one desired path lying
    /// beneath another. Both sides of a conflict are refused.
    #[error(
        "refusing desired paths that claim overlapping locations{}: {}",
        note(origin),
        join(paths)
    )]
    TreeConflict {
        /// The conflicting desired keys, verbatim.
        paths: BTreeSet<Utf8PathBuf>,
        /// Where the tree holding them came from.
        origin: Origin,
    },
    /// A filesystem operation failed. Not a refusal.
    #[error("{path}: {source}")]
    Io {
        /// The path the operation touched.
        path: Utf8PathBuf,
        /// The OS error, unchanged.
        source: std::io::Error,
    },
    /// The manifest file exists but does not parse as manifest JSON. Not a
    /// refusal.
    #[error("manifest {path} is not valid: {source}")]
    ManifestFormat {
        /// The manifest file's location.
        path: Utf8PathBuf,
        /// The parse error, unchanged.
        source: serde_json::Error,
    },
    /// The manifest parses but declares a version this crate does not
    /// support. Not a refusal. Both loaders read the declared version before
    /// decoding the rest strictly, so a future format reports this or
    /// [`MappingVersion`](Error::MappingVersion), not the format error.
    #[error("manifest {path} has version {found}; this crate supports version {supported}")]
    ManifestVersion {
        /// The manifest file's location.
        path: Utf8PathBuf,
        /// The version the file declares.
        found: u32,
        /// The version this crate supports.
        supported: u32,
    },
    /// Another writer holds the single-writer lock on the state directory.
    /// Acquisition is try-lock, so a contended lock reports this immediately.
    /// Not a refusal.
    #[error("state lock {path} is held by another writer")]
    LockHeld {
        /// The lock file's path, relative to the state directory.
        path: Utf8PathBuf,
    },
    /// The mapping file does not parse as mapping TOML — a syntax error, a
    /// missing required field, or an unknown key. Not a refusal.
    #[error("mapping {path} is not valid: {source}")]
    MappingFormat {
        /// The mapping file's location.
        path: Utf8PathBuf,
        /// The parse error, unchanged.
        source: toml::de::Error,
    },
    /// The mapping parses but declares a version this crate does not support.
    /// Not a refusal.
    #[error("mapping {path} has version {found}; this crate supports version {supported}")]
    MappingVersion {
        /// The mapping file's location.
        path: Utf8PathBuf,
        /// The version the file declares.
        found: u32,
        /// The version this crate supports.
        supported: u32,
    },
    /// A `[files]` entry sets both `contents` and `source`, or neither. Not a
    /// refusal.
    #[error(
        "mapping {path}: files entry \"{key}\" must set exactly one of `contents` and `source`"
    )]
    MappingContentsXorSource {
        /// The mapping file's location.
        path: Utf8PathBuf,
        /// The offending entry's key, verbatim.
        key: Utf8PathBuf,
    },
    /// One projected path is claimed by more than one mapping entry — two
    /// table entries, two archive prefixes, or an archive member landing on a
    /// path another entry claimed. Not a refusal.
    #[error("mapping {path}: \"{key}\" is projected by more than one entry")]
    MappingDuplicate {
        /// The mapping file's location.
        path: Utf8PathBuf,
        /// The normalized key both entries project.
        key: Utf8PathBuf,
    },
    /// An archive source's filename extension names no decoder this crate
    /// has. Not a refusal.
    #[error(
        "archive {path}: no decoder for this name; expected one of {}",
        crate::ARCHIVE_EXTENSIONS
    )]
    ArchiveFormatUnknown {
        /// The archive's location.
        path: Utf8PathBuf,
    },
    /// An archive does not decode as the format its extension named. Not a
    /// refusal.
    #[error("archive {path} does not decode as {format}: {source}")]
    ArchiveDecode {
        /// The archive's location.
        path: Utf8PathBuf,
        /// The format the extension picked.
        format: crate::ArchiveFormat,
        /// The decoder's error, unchanged.
        source: std::io::Error,
    },
    /// An archive holds a member whose name is not UTF-8. Not a refusal.
    #[error("archive {path} holds a member whose name is not UTF-8: {name:?}")]
    ArchiveMemberNameNotUtf8 {
        /// The archive's location.
        path: Utf8PathBuf,
        /// The name as the archive spells it, lossily decoded.
        name: String,
    },
    /// An archive holds a symlink member whose target is not UTF-8. Not a
    /// refusal.
    #[error("archive {path}: member {member} has a target that is not UTF-8: {target:?}")]
    ArchiveMemberTargetNotUtf8 {
        /// The archive's location.
        path: Utf8PathBuf,
        /// The member's path as it projects, relative to any prefix.
        member: Utf8PathBuf,
        /// The target, lossily decoded.
        target: String,
    },
    /// An archive holds a member of a kind the projection never writes — a
    /// hardlink, a device node, a fifo, a socket, a GNU sparse member. Not a
    /// refusal.
    #[error("archive {path}: member {member} is not a file, directory, or symlink")]
    ArchiveMemberKind {
        /// The archive's location.
        path: Utf8PathBuf,
        /// The offending member, as the archive spells it — before `strip`
        /// and the containment gateway.
        member: Utf8PathBuf,
    },
    /// A zip member's two spellings of its kind disagree: the trailing `/`
    /// and the file-type bits of the Unix mode it may also record. Not a
    /// refusal.
    #[error("archive {path}: member {member} is one kind by name and another by mode")]
    ArchiveMemberKindDisagrees {
        /// The archive's location.
        path: Utf8PathBuf,
        /// The offending member, as the archive spells it, trailing slash
        /// included.
        member: Utf8PathBuf,
    },
    /// Two members of one archive claim the same projected path — duplicate
    /// names, or two members `strip` collapses onto one path. Not a refusal.
    #[error("archive {path}: more than one member projects to {member}")]
    ArchiveMemberDuplicate {
        /// The archive's location.
        path: Utf8PathBuf,
        /// The projected path both members claim, relative to any prefix.
        member: Utf8PathBuf,
    },
    /// A file or symlink member of an archive has no path left after `strip`
    /// dropped its leading components. Not a refusal.
    #[error("archive {path}: member {member} has nothing left after strip {strip}")]
    ArchiveMemberStripped {
        /// The archive's location.
        path: Utf8PathBuf,
        /// The member's path as the archive spells it, normalized.
        member: Utf8PathBuf,
        /// The number of leading components dropped.
        strip: u32,
    },
    /// An archive member nests deeper than an expansion places. Not a
    /// refusal.
    #[error(
        "archive {path}: member {member} nests deeper than the {limit} levels a tree may carry"
    )]
    ArchiveMemberTooDeep {
        /// The archive's location.
        path: Utf8PathBuf,
        /// The offending member's path, relative to any prefix.
        member: Utf8PathBuf,
        /// The deepest nesting an expansion accepts, counted in directories
        /// above the member as it projects — after `strip`.
        limit: usize,
    },
    /// An archive expands to more bytes than one may allocate. Not a refusal.
    #[error("archive {path} expands past the {limit} bytes an archive may allocate")]
    ArchiveTooLarge {
        /// The archive's location.
        path: Utf8PathBuf,
        /// The most an expansion may allocate, summed over its members.
        limit: u64,
    },
    /// An archive carries more members than an expansion places. Not a
    /// refusal.
    #[error("archive {path} carries more than the {limit} members an archive may hold")]
    ArchiveTooManyMembers {
        /// The archive's location.
        path: Utf8PathBuf,
        /// The most members an expansion accepts.
        limit: usize,
    },
    /// A source tree holds an entry whose name is not UTF-8, so
    /// [`load_tree`](crate::load_tree) cannot key it. Not a refusal.
    #[error("tree entry name under {path} is not UTF-8: {name:?}")]
    TreeNameNotUtf8 {
        /// The absolute path of the directory holding the entry.
        path: Utf8PathBuf,
        /// The name as the filesystem gave it, lossily decoded — every
        /// invalid sequence renders as `U+FFFD`.
        name: String,
    },
    /// A source tree holds a symlink whose target is not UTF-8. Not a
    /// refusal.
    #[error("tree symlink {path} has a target that is not UTF-8: {target:?}")]
    TreeTargetNotUtf8 {
        /// The link's absolute path.
        path: Utf8PathBuf,
        /// The target as the filesystem gave it, lossily decoded.
        target: String,
    },
    /// A source tree nests deeper than [`load_tree`](crate::load_tree) walks.
    /// Not a refusal.
    #[error("tree directory {path} nests deeper than the {limit} levels a source tree may carry")]
    TreeTooDeep {
        /// The absolute path of the directory one level past the limit.
        path: Utf8PathBuf,
        /// The deepest nesting the walk accepts, counted in directories below
        /// the source root — [`MAX_WALK_DEPTH`].
        limit: usize,
    },
    /// The destination nests deeper than the destination walk descends, or a
    /// plan writes a path past that depth. Not a refusal.
    #[error(
        "destination directory {path} nests deeper than the {limit} levels a destination may carry"
    )]
    DestinationTooDeep {
        /// The path of the directory one level past the limit, relative to
        /// the destination.
        path: Utf8PathBuf,
        /// The deepest nesting the walk accepts, counted in directories below
        /// the destination root — [`MAX_WALK_DEPTH`].
        limit: usize,
    },
    /// A source tree holds a node of a kind the projection never writes — a
    /// FIFO, a socket, or a device node. Not a refusal.
    #[error("tree node {path} is not a file, directory, or symlink")]
    TreeNodeKind {
        /// The node's absolute path.
        path: Utf8PathBuf,
    },
    /// Refusal: [`Block`](crate::EntryKind::Block) entries the projection
    /// declines, each with the [`BlockFault`] that declined it.
    #[error("refusing block entries: {}", join_blocks(blocks))]
    Block {
        /// The offending paths, relative to the destination, each mapped to
        /// what is wrong with it.
        blocks: BTreeMap<Utf8PathBuf, BlockFault>,
    },
}

/// Why one [`Block`](crate::EntryKind::Block) entry is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum BlockFault {
    /// The marker is the empty string, which would match every line start.
    MarkerEmpty,
    /// The marker carries a `\n` or a `\r`; a marker is one line.
    MarkerNotOneLine,
    /// The marker begins or ends with a space or a tab.
    MarkerEdgeWhitespace,
    /// A line of the body equals the marker.
    BodyCarriesMarker,
    /// The placement is [`Prepend`](crate::Placement::Prepend) and the body
    /// neither is empty nor ends with `\n`.
    BodyNotNewlineTerminated,
    /// The placement is [`Append`](crate::Placement::Append) and the author's
    /// side of the container neither is empty nor ends with `\n`.
    ContainerNotNewlineTerminated,
    /// The container, or a directory above it, is not there.
    ContainerMissing,
    /// The path is recorded as a whole node and desired as a block, or the
    /// other way round.
    KindChange,
    /// A plan's expected signature names a marker or placement the manifest
    /// does not record at that path.
    SignatureNotRecorded,
    MarkerInAuthorText,
}

impl std::fmt::Display for BlockFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self {
            BlockFault::MarkerEmpty => "the marker is empty",
            BlockFault::MarkerNotOneLine => "the marker carries a line break",
            BlockFault::MarkerEdgeWhitespace => "the marker begins or ends with a space or a tab",
            BlockFault::BodyCarriesMarker => "a line of the body equals the marker",
            BlockFault::BodyNotNewlineTerminated => {
                "prepending needs a body that is empty or ends with a newline"
            }
            BlockFault::ContainerNotNewlineTerminated => {
                "appending needs a container that is empty or ends with a newline"
            }
            BlockFault::ContainerMissing => {
                "the container is not there, and a block never creates one"
            }
            BlockFault::KindChange => "a path never changes between a whole node and a block",
            BlockFault::SignatureNotRecorded => {
                "the expected signature names a region the manifest does not record"
            }
            BlockFault::MarkerInAuthorText => {
                "the container's author side already holds the marker being migrated to"
            }
        };
        f.write_str(reason)
    }
}

impl Error {
    /// Whether this error is a refusal — the projection declining to touch a
    /// path — rather than an operation failing.
    pub fn is_refusal(&self) -> bool {
        match self {
            Error::Drift { .. }
            | Error::Foreign { .. }
            | Error::Containment { .. }
            | Error::OwnerConflict { .. }
            | Error::ExternalTarget { .. }
            | Error::InvalidTarget { .. }
            | Error::TreeConflict { .. }
            | Error::Block { .. } => true,
            Error::Io { .. }
            | Error::ManifestFormat { .. }
            | Error::ManifestVersion { .. }
            | Error::LockHeld { .. }
            | Error::MappingFormat { .. }
            | Error::MappingVersion { .. }
            | Error::MappingContentsXorSource { .. }
            | Error::MappingDuplicate { .. }
            | Error::ArchiveFormatUnknown { .. }
            | Error::ArchiveDecode { .. }
            | Error::ArchiveMemberNameNotUtf8 { .. }
            | Error::ArchiveMemberTargetNotUtf8 { .. }
            | Error::ArchiveMemberKind { .. }
            | Error::ArchiveMemberKindDisagrees { .. }
            | Error::ArchiveMemberDuplicate { .. }
            | Error::ArchiveMemberStripped { .. }
            | Error::ArchiveMemberTooDeep { .. }
            | Error::ArchiveTooLarge { .. }
            | Error::ArchiveTooManyMembers { .. }
            | Error::TreeNameNotUtf8 { .. }
            | Error::TreeTargetNotUtf8 { .. }
            | Error::TreeTooDeep { .. }
            | Error::DestinationTooDeep { .. }
            | Error::TreeNodeKind { .. } => false,
        }
    }

    /// Names `origin` as the source of this refusal, where it is one of the
    /// four that carry an [`Origin`]; every other variant is returned
    /// unchanged.
    pub(crate) fn with_origin(self, origin: &Origin) -> Error {
        match self {
            Error::Containment { paths, .. } => Error::Containment {
                paths,
                origin: origin.clone(),
            },
            Error::TreeConflict { paths, .. } => Error::TreeConflict {
                paths,
                origin: origin.clone(),
            },
            Error::ExternalTarget { links, .. } => Error::ExternalTarget {
                links,
                origin: origin.clone(),
            },
            Error::InvalidTarget { links, .. } => Error::InvalidTarget {
                links,
                origin: origin.clone(),
            },
            other => other,
        }
    }
}

fn note(origin: &Origin) -> String {
    match origin {
        Origin::Caller => String::new(),
        named => format!(" ({named})"),
    }
}

fn join(paths: &BTreeSet<Utf8PathBuf>) -> String {
    paths
        .iter()
        .map(|path| path.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn join_conflicts(conflicts: &BTreeMap<Utf8PathBuf, BTreeSet<String>>) -> String {
    conflicts
        .iter()
        .map(|(path, owners)| {
            let owners = owners.iter().cloned().collect::<Vec<_>>().join("+");
            format!("{path} (held by {owners})")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn join_blocks(blocks: &BTreeMap<Utf8PathBuf, BlockFault>) -> String {
    blocks
        .iter()
        .map(|(path, fault)| format!("{path} ({fault})"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn join_links(links: &BTreeMap<Utf8PathBuf, String>) -> String {
    links
        .iter()
        .map(|(path, target)| format!("{path} -> {target}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// [`join_links`] with the target quoted and escaped, so an empty string or a
/// NUL byte renders visibly.
fn join_invalid(links: &BTreeMap<Utf8PathBuf, String>) -> String {
    links
        .iter()
        .map(|(path, target)| format!("{path} -> {target:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
