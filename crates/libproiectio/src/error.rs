use camino::Utf8PathBuf;
use serde::{Serialize, Serializer};
use thiserror::Error;

use crate::Refused;

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
#[derive(Debug, Error, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Error {
    /// A refusal: the projection declining to touch paths. Everything else
    /// is an operation failing.
    #[error(transparent)]
    Refused(#[from] Refused),
    /// A filesystem operation failed. Not a refusal.
    #[error("{path}: {source}")]
    Io {
        /// The path the operation touched.
        path: Utf8PathBuf,
        /// The OS error, unchanged.
        #[serde(serialize_with = "display_string")]
        source: std::io::Error,
    },
    /// The manifest file exists but does not parse as manifest JSON. Not a
    /// refusal.
    #[error("manifest {path} is not valid: {source}")]
    ManifestFormat {
        /// The manifest file's location.
        path: Utf8PathBuf,
        /// The parse error, unchanged.
        #[serde(serialize_with = "display_string")]
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
    /// The mapping file does not parse as mapping TOML — a syntax error, a
    /// missing required field, or an unknown key. Not a refusal.
    #[error("mapping {path} is not valid: {source}")]
    MappingFormat {
        /// The mapping file's location.
        path: Utf8PathBuf,
        /// The parse error, unchanged.
        #[serde(serialize_with = "display_string")]
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
        #[serde(serialize_with = "display_string")]
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
    /// A path handed to [`load_files`](crate::load_files) is neither a regular
    /// file nor a symlink, or carries no file name to project under. Not a
    /// refusal.
    #[error("{path}: named files must be regular files or symlinks")]
    FilesNodeKind {
        /// The path's absolute spelling.
        path: Utf8PathBuf,
    },
    /// More than one path handed to [`load_files`](crate::load_files) carries
    /// the same file name, which is the key each would project under. Not a
    /// refusal.
    #[error(
        "more than one named path projects as {}: {first}, {second}",
        first.file_name().unwrap_or_default()
    )]
    FilesDuplicate {
        /// The first path carrying the shared file name.
        first: Utf8PathBuf,
        /// The next path carrying it.
        second: Utf8PathBuf,
    },
    #[error("source {path} is a directory: strip drops components of archive members")]
    StripOnDirectory { path: Utf8PathBuf },
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
