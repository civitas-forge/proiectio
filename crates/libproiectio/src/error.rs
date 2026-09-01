use camino::Utf8PathBuf;
use serde::{Serialize, Serializer};
use thiserror::Error;

use crate::Refused;

/// The crate-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

/// What a path was to the run that met an OS error on it — the word an
/// [`Error::Io`] message opens with, so a reader knows which argument to fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IoRole {
    Destination,
    StateDirectory,
    Mapping,
    SourceTree,
    Archive,
    /// A file read as the body of an entry: one a mapping's `source` names,
    /// or one whose kind is not known until it is read.
    Source,
    /// A file named on its own, projected under its basename.
    NamedFile,
    /// The run's own working paths; the message names no role for these.
    Unstated,
}

impl std::fmt::Display for IoRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let word = match self {
            IoRole::Destination => "destination ",
            IoRole::StateDirectory => "state directory ",
            IoRole::Mapping => "mapping ",
            IoRole::SourceTree => "source tree ",
            IoRole::Archive => "archive ",
            IoRole::Source => "source ",
            IoRole::NamedFile => "named file ",
            IoRole::Unstated => "",
        };
        f.write_str(word)
    }
}

/// How deep below its root a directory walk descends, counted in directories.
/// The bound exists because a bind mount can make a directory its own
/// ancestor — a tree with no bottom that no symlink check sees — and a debug
/// build's 2 MiB stack exhausts past ~200 levels.
pub const MAX_WALK_DEPTH: usize = 64;

/// Everything the engine can fail with. [`Error::Refused`] is the projection
/// declining to touch paths; every other variant is an operation failing.
///
/// [`Error::is_refusal`] splits the two, which a CLI's 0/1/2 exit contract
/// falls out of:
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
#[derive(Debug, Error, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Error {
    #[error(transparent)]
    Refused(#[from] Refused),
    #[error("{role}{path}: {source}")]
    Io {
        role: IoRole,
        path: Utf8PathBuf,
        #[serde(serialize_with = "display_string")]
        source: std::io::Error,
    },
    #[error("manifest {path} is not valid: {source}")]
    ManifestFormat {
        path: Utf8PathBuf,
        #[serde(serialize_with = "display_string")]
        source: serde_json::Error,
    },
    /// Both loaders read the declared version before decoding the rest
    /// strictly, so a future format reports this or
    /// [`MappingVersion`](Error::MappingVersion), not the format error.
    #[error("manifest {path} has version {found}; this crate supports version {supported}")]
    ManifestVersion {
        path: Utf8PathBuf,
        found: u32,
        supported: u32,
    },
    #[error("manifest records {path}, which enters a pruned path component")]
    ManifestPathPruned { path: Utf8PathBuf },
    /// Acquisition is try-lock, so a contended lock reports this immediately.
    #[error("state lock {path} is held by another writer")]
    LockHeld { path: Utf8PathBuf },
    #[error("the current directory cannot be read: {source}")]
    CurrentDirectory {
        #[serde(serialize_with = "display_string")]
        source: std::io::Error,
    },
    #[error("path is not UTF-8: {path:?}")]
    PathNotUtf8 { path: String },
    #[error(
        "state directory {path} is the target directory: the projection's own \
         state files would classify as foreign"
    )]
    StateDirIsTarget { path: Utf8PathBuf },
    #[error(
        "state directory {path} enters pruned component {component:?}: the projection must be able to read and write its state"
    )]
    StateDirPruned {
        path: Utf8PathBuf,
        component: String,
    },
    #[error("pruned component {component:?} is not one path component")]
    InvalidPrunedComponent { component: String },
    #[error("mapping {path} is not valid: {source}")]
    MappingFormat {
        path: Utf8PathBuf,
        #[serde(serialize_with = "display_string")]
        source: toml::de::Error,
    },
    /// Reported before the file is opened, so the message names the option a
    /// directory belongs to rather than whatever OS error a read would raise.
    #[error(
        "mapping {path} is a directory: a mapping is a TOML file; pass a directory as --tree \
         to project the tree it holds"
    )]
    MappingIsDirectory { path: Utf8PathBuf },
    #[error("mapping {path} has version {found}; this crate supports version {supported}")]
    MappingVersion {
        path: Utf8PathBuf,
        found: u32,
        supported: u32,
    },
    #[error(
        "mapping {path}: files entry \"{key}\" must set exactly one of `contents` and `source`"
    )]
    MappingContentsXorSource { path: Utf8PathBuf, key: Utf8PathBuf },
    #[error("mapping {path}: \"{key}\" is projected by more than one entry")]
    MappingDuplicate { path: Utf8PathBuf, key: Utf8PathBuf },
    #[error(
        "archive {path}: no decoder for this name; expected one of {}",
        crate::ARCHIVE_EXTENSIONS
    )]
    ArchiveFormatUnknown { path: Utf8PathBuf },
    #[error("archive {path} does not decode as {format}: {source}")]
    ArchiveDecode {
        path: Utf8PathBuf,
        format: crate::ArchiveFormat,
        #[serde(serialize_with = "display_string")]
        source: std::io::Error,
    },
    #[error("archive {path} holds a member whose name is not UTF-8: {name:?}")]
    ArchiveMemberNameNotUtf8 { path: Utf8PathBuf, name: String },
    #[error("archive {path}: member {member} has a target that is not UTF-8: {target:?}")]
    ArchiveMemberTargetNotUtf8 {
        path: Utf8PathBuf,
        member: Utf8PathBuf,
        target: String,
    },
    /// A member of a kind the projection never writes — a hardlink, a device
    /// node, a fifo, a socket, a GNU sparse member.
    #[error("archive {path}: member {member} is not a file, directory, or symlink")]
    ArchiveMemberKind {
        path: Utf8PathBuf,
        /// As the archive spells it — before `strip` and containment.
        member: Utf8PathBuf,
    },
    /// A zip member's two spellings of its kind disagree: the trailing `/`
    /// and the file-type bits of the Unix mode it may also record.
    #[error("archive {path}: member {member} is one kind by name and another by mode")]
    ArchiveMemberKindDisagrees {
        path: Utf8PathBuf,
        member: Utf8PathBuf,
    },
    #[error("archive {path}: more than one member projects to {member}")]
    ArchiveMemberDuplicate {
        path: Utf8PathBuf,
        member: Utf8PathBuf,
    },
    /// A member dropped among surviving ones is tolerated — that is what
    /// skipping an AppleDouble sibling is for — but an expansion left with
    /// nothing to project is a `strip` deeper than the archive, not a desired
    /// empty tree. An archive that drops nothing is outside this rule.
    #[error("archive {path}: strip {strip} left nothing to project ({dropped} members dropped)")]
    ArchiveFullyStripped {
        path: Utf8PathBuf,
        strip: u32,
        dropped: usize,
    },
    #[error(
        "archive {path}: member {member} nests deeper than the {limit} levels a tree may carry"
    )]
    ArchiveMemberTooDeep {
        path: Utf8PathBuf,
        member: Utf8PathBuf,
        limit: usize,
    },
    #[error("archive {path} expands past the {limit} bytes one load may hold in memory")]
    ArchiveTooLarge { path: Utf8PathBuf, limit: u64 },
    /// A zip is read index-first and that index is not smaller than the file
    /// carrying it, so the file itself is weighed before parsing; every other
    /// format is weighed by what it expands to.
    #[error(
        "archive {path} is {size} bytes on disk, and a zip's index is read whole before any \
         member, so the file itself has to fit: {remaining} bytes are left of the {limit} bytes \
         one load may hold in memory"
    )]
    ArchiveFileTooLarge {
        path: Utf8PathBuf,
        size: u64,
        remaining: u64,
        limit: u64,
    },
    /// The named file is the one being read when the bytes ran out, which
    /// need not be the largest:
    /// [`Limits::max_source_bytes`](crate::Limits::max_source_bytes) is spent
    /// across every source the load reads.
    #[error("source {path} reads past the {limit} bytes one load may hold in memory")]
    SourceTooLarge { path: Utf8PathBuf, limit: u64 },
    #[error("archive {path} carries more than the {limit} members an archive may hold")]
    ArchiveTooManyMembers { path: Utf8PathBuf, limit: usize },
    #[error("tree entry name under {path} is not UTF-8: {name:?}")]
    TreeNameNotUtf8 {
        /// The directory holding the entry.
        path: Utf8PathBuf,
        name: String,
    },
    #[error("tree symlink {path} has a target that is not UTF-8: {target:?}")]
    TreeTargetNotUtf8 { path: Utf8PathBuf, target: String },
    #[error("tree directory {path} nests deeper than the {limit} levels a source tree may carry")]
    TreeTooDeep {
        /// The directory one level past the limit.
        path: Utf8PathBuf,
        limit: usize,
    },
    /// The destination nests deeper than the destination walk descends, or a
    /// plan writes a path past that depth.
    #[error(
        "destination directory {path} nests deeper than the {limit} levels a destination may carry"
    )]
    DestinationTooDeep { path: Utf8PathBuf, limit: usize },
    #[error("tree node {path} is not a file, directory, or symlink")]
    TreeNodeKind { path: Utf8PathBuf },
    #[error("{path}: named files must be regular files or symlinks")]
    FilesNodeKind { path: Utf8PathBuf },
    /// The shared file name is the key each path would project under.
    #[error(
        "more than one named path projects as {}: {first}, {second}",
        first.file_name().unwrap_or_default()
    )]
    FilesDuplicate {
        first: Utf8PathBuf,
        second: Utf8PathBuf,
    },
    #[error("source {path} is a directory: strip drops components of archive members")]
    StripOnDirectory { path: Utf8PathBuf },
    /// Refused where the plan is decided, before a write can record the name
    /// or a removal look it up.
    #[error("{owner:?} is not an owner: {}", crate::OWNER_RULE)]
    OwnerNotNamed { owner: String },
}

fn display_string<T: std::fmt::Display, S: Serializer>(
    value: &T,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.collect_str(value)
}

impl Error {
    /// Whether this error is a refusal — the projection declining to touch a
    /// path — rather than an operation failing.
    pub fn is_refusal(&self) -> bool {
        matches!(self, Error::Refused(_))
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
