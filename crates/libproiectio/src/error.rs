use std::collections::{BTreeMap, BTreeSet};

use camino::Utf8PathBuf;
use thiserror::Error;

/// The crate-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything the engine can fail with.
///
/// Variants split into *refusals* — the projection declining to touch a
/// path, each carrying the offending paths — and runtime failures (I/O,
/// manifest format). [`Error::is_refusal`] names the split, and a CLI's
/// 0/1/2 exit contract falls out of a single match:
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
    /// Refusal: recorded paths whose state on disk differs from the
    /// recorded entry — bytes, kind, or executable bit — user edits the
    /// projection will not overwrite or remove unless the caller passes
    /// [`DriftPolicy::Overwrite`](crate::DriftPolicy::Overwrite).
    #[error("refusing to touch drifted paths (edited on disk): {}", join(paths))]
    Drift {
        /// The drifted paths, relative to the destination.
        paths: BTreeSet<Utf8PathBuf>,
    },
    /// Refusal: paths on disk that the manifest does not record. A
    /// projection never overwrites or removes a file it did not write; no
    /// policy lifts this.
    ///
    /// Foreignness is judged over what the projection would own. For a
    /// [`Block`](crate::EntryKind::Block) entry that is the delimited
    /// region, not the file around it, so a pre-existing container file
    /// does not make the path foreign. Until block-region classification
    /// lands, the deciding stage cannot yet see regions and refuses a
    /// desired block over an unrecorded container with this error —
    /// conservative, never a wrong write ([`decide`](crate::decide)'s
    /// rustdoc names the seam).
    #[error(
        "refusing to touch foreign paths (not written by this projection): {}",
        join(paths)
    )]
    Foreign {
        /// The foreign paths, relative to the destination.
        paths: BTreeSet<Utf8PathBuf>,
    },
    /// Refusal: desired-tree paths the projection may not write — paths
    /// refused by [`contained_join`](crate::contained_join) (absolute,
    /// climbing out via `..`, empty or `.` components, backslashes, and
    /// component shapes Windows resolves specially — its rustdoc is the
    /// full list), paths resolving through a symlinked ancestor, and paths
    /// entering the projection's own state directory. The symlink half is
    /// two rules, not one applied twice: deciding refuses a desired path
    /// beneath *any* link that outlives the plan, while applying refuses
    /// an ancestor link that is unowned, graded external, or cyclic — and
    /// still follows one the manifest owns whose target resolves inside
    /// the destination (`docs/security.lex` section 2). An ancestor link
    /// the manifest owns whose on-disk target changed is
    /// [`Drift`](Error::Drift), not this.
    #[error("refusing paths that violate containment: {}", join(paths))]
    Containment {
        /// The offending paths as given by the desired tree.
        paths: BTreeSet<Utf8PathBuf>,
    },
    /// Refusal: the desired entry for a path — bytes, kind, or executable
    /// bit — differs from what another owner holds there. Two owners may
    /// hold one path only while writing identical entries. No policy lifts
    /// this: the owners' trees must agree first.
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
    /// parent directory, land outside the destination — absolute targets,
    /// ones climbing out, and the spellings graded external on every host
    /// (`docs/security.lex` section 3 carries the whole rule). The
    /// caller lifts this per plan with
    /// [`ExternalTargetPolicy::Allow`](crate::ExternalTargetPolicy::Allow)
    /// (a CLI's `--allow-external-targets`), which writes each link with
    /// its target verbatim. Nothing is ever written *through* such a link:
    /// an external target is a pointer, and apply refuses to resolve one.
    #[error(
        "refusing symlinks with targets outside the destination: {}",
        join_links(links)
    )]
    ExternalTarget {
        /// The offending links: path of each link, relative to the
        /// destination, mapped to its target string verbatim.
        links: BTreeMap<Utf8PathBuf, String>,
    },
    /// Refusal: desired symlinks whose targets are not pathnames on any
    /// host — the empty string, which names nothing, and strings carrying a
    /// NUL byte, which terminates a pathname rather than appearing in one.
    /// Either would reach the OS as a target and come back an error partway
    /// through apply, so the pure stage refuses first. No policy lifts
    /// this, [`ExternalTargetPolicy::Allow`](crate::ExternalTargetPolicy)
    /// included: the permission is about where a pointer points, and there
    /// is no pointer here. It is not a promise that every other target is
    /// writable — a target past the host's length limit still fails at the
    /// filesystem, which nothing lexical could foresee.
    #[error(
        "refusing symlinks whose targets are not paths: {}",
        join_invalid(links)
    )]
    InvalidTarget {
        /// The offending links: path of each link, relative to the
        /// destination, mapped to its target string verbatim.
        links: BTreeMap<Utf8PathBuf, String>,
    },
    /// Refusal: desired keys claiming one on-disk location more than once
    /// — two keys normalizing to the same path, or one desired path lying
    /// beneath another. No file or block entry can hold children, and a
    /// path nesting beneath a desired *symlink* would land somewhere the
    /// plan does not name
    /// ([`Refusal::TreeConflict`](crate::Refusal::TreeConflict) carries
    /// the rationale). Both sides of a conflict are
    /// refused: there is no deterministic entry to prefer.
    /// [`load_mapping`](crate::load_mapping) rejects same-path duplicates
    /// at parse time as [`MappingDuplicate`](Error::MappingDuplicate);
    /// this refusal is the deciding stage's verdict on any tree, however
    /// built.
    #[error(
        "refusing desired paths that claim overlapping locations: {}",
        join(paths)
    )]
    TreeConflict {
        /// The conflicting desired keys, verbatim.
        paths: BTreeSet<Utf8PathBuf>,
    },
    /// A filesystem operation failed. Not a refusal: the underlying OS
    /// error stays visible.
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
    /// support. Not a refusal. Load reads the declared version before
    /// strictly decoding the rest, so an unsupported future format
    /// reports this rather than [`ManifestFormat`](Error::ManifestFormat).
    #[error("manifest {path} has version {found}; this crate supports version {supported}")]
    ManifestVersion {
        /// The manifest file's location.
        path: Utf8PathBuf,
        /// The version the file declares.
        found: u32,
        /// The version this crate supports.
        supported: u32,
    },
    /// Another writer holds the single-writer lock on the state directory
    /// (`docs/implementation.lex` section 7). `StateLock::acquire` has
    /// try-lock semantics, so a contended lock reports this immediately
    /// rather than blocking. Not a refusal — no destination path is being
    /// declined, the run simply cannot start — so [`Error::is_refusal`] is
    /// `false` and a CLI maps it to exit 1 like any other runtime failure.
    ///
    /// (The variant is spelled on every target so the exit contract does
    /// not shift under a `cfg`; the lock itself, `StateLock`, is built only
    /// where `flock(2)` is available.)
    #[error("state lock {path} is held by another writer")]
    LockHeld {
        /// The lock file's path, relative to the state directory —
        /// [`LOCK_FILE_NAME`](crate::LOCK_FILE_NAME).
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
    /// The mapping parses but declares a version this crate does not
    /// support. Not a refusal. Load reads the declared version before
    /// strictly decoding the rest, so an unsupported future format reports
    /// this rather than [`MappingFormat`](Error::MappingFormat).
    #[error("mapping {path} has version {found}; this crate supports version {supported}")]
    MappingVersion {
        /// The mapping file's location.
        path: Utf8PathBuf,
        /// The version the file declares.
        found: u32,
        /// The version this crate supports.
        supported: u32,
    },
    /// A `[files]` entry sets both `contents` and `source`, or neither;
    /// exactly one must say where the bytes come from. Not a refusal.
    #[error(
        "mapping {path}: files entry \"{key}\" must set exactly one of `contents` and `source`"
    )]
    MappingContentsXorSource {
        /// The mapping file's location.
        path: Utf8PathBuf,
        /// The offending entry's key, verbatim.
        key: Utf8PathBuf,
    },
    /// One projected path is claimed by more than one mapping entry: two
    /// entries — under `[files]`, `[links]`, or one of each — whose keys
    /// lexically normalize to the same path. Not a refusal.
    #[error("mapping {path}: \"{key}\" is projected by more than one entry")]
    MappingDuplicate {
        /// The mapping file's location.
        path: Utf8PathBuf,
        /// The normalized key both entries project.
        key: Utf8PathBuf,
    },
    /// An archive source's filename extension names no decoder this crate
    /// has. Not a refusal. The extension picks the decoder and nothing else
    /// does — the bytes are never sniffed — so a name outside the list fails
    /// here rather than being decoded as whatever it turns out to be
    /// ([`ArchiveFormat::for_path`](crate::ArchiveFormat::for_path)).
    #[error(
        "archive {path}: no decoder for this name; expected one of {}",
        crate::ARCHIVE_EXTENSIONS
    )]
    ArchiveFormatUnknown {
        /// The archive's location.
        path: Utf8PathBuf,
    },
    /// An archive does not decode as the format its extension named — a
    /// zip spelled `.tar.gz`, a truncated stream, a compression method this
    /// build does not carry. Not a refusal; the decoder's own error stays
    /// visible.
    #[error("archive {path} does not decode as {format}: {source}")]
    ArchiveDecode {
        /// The archive's location.
        path: Utf8PathBuf,
        /// The format the extension picked.
        format: crate::ArchiveFormat,
        /// The decoder's error, unchanged.
        source: std::io::Error,
    },
    /// An archive holds a member whose name is not UTF-8, so
    /// [`load_archive`](crate::load_archive) cannot key it. Not a refusal,
    /// for the same reason as
    /// [`TreeNameNotUtf8`](Error::TreeNameNotUtf8): the archive carries
    /// content this crate cannot name.
    #[error("archive {path} holds a member whose name is not UTF-8: {name:?}")]
    ArchiveMemberNameNotUtf8 {
        /// The archive's location.
        path: Utf8PathBuf,
        /// The name as the archive spells it, lossily decoded — what can be
        /// *shown* of bytes that have no UTF-8 spelling, on the same terms
        /// as [`TreeNameNotUtf8::name`](Error::TreeNameNotUtf8).
        name: String,
    },
    /// An archive holds a symlink member whose target is not UTF-8, which
    /// [`Entry::Symlink`](crate::Entry::Symlink) — a `String` — cannot
    /// carry. Not a refusal.
    #[error("archive {path}: member {member} has a target that is not UTF-8: {target:?}")]
    ArchiveMemberTargetNotUtf8 {
        /// The archive's location.
        path: Utf8PathBuf,
        /// The member's path as it projects, relative to any prefix.
        member: Utf8PathBuf,
        /// The target, lossily decoded, on the same terms as
        /// [`TreeTargetNotUtf8::target`](Error::TreeTargetNotUtf8).
        target: String,
    },
    /// An archive holds a member of a kind the projection never writes — a
    /// hardlink, a device node, a fifo, a socket, a GNU sparse member.
    /// Member kinds are restricted to files, directories, and symlinks
    /// (`docs/security.lex` section 4). Not a refusal, for the same reason
    /// as [`TreeNodeKind`](Error::TreeNodeKind).
    #[error("archive {path}: member {member} is not a file, directory, or symlink")]
    ArchiveMemberKind {
        /// The archive's location.
        path: Utf8PathBuf,
        /// The offending member's path as it projects, relative to any
        /// prefix.
        member: Utf8PathBuf,
    },
    /// Two members of one archive claim the same projected path — duplicate
    /// names, which zip permits outright and tar permits by convention, or
    /// two distinct members `strip` collapses onto one path. Not a refusal.
    ///
    /// A desired tree holds one entry per path, so a duplicate has to
    /// resolve to a single member; first-wins and last-wins both hand that
    /// choice to whoever built the archive, which is the gap between what a
    /// reader shows and what an extractor writes. The projection refuses
    /// instead, as it refuses two mapping keys claiming one location
    /// ([`MappingDuplicate`](Error::MappingDuplicate)) and two desired keys
    /// claiming one ([`TreeConflict`](Error::TreeConflict)): there is no
    /// deterministic entry to prefer.
    #[error("archive {path}: more than one member projects to {member}")]
    ArchiveMemberDuplicate {
        /// The archive's location.
        path: Utf8PathBuf,
        /// The projected path both members claim, relative to any prefix.
        member: Utf8PathBuf,
    },
    /// A file or symlink member of an archive has no path left after
    /// `strip` dropped its leading components. Not a refusal: the member is
    /// content the caller asked to project and there is nowhere to project
    /// it to, so dropping it silently would lose it. A *directory* member
    /// `strip` erases is the wrapper `strip` exists to drop, and carries no
    /// entry in any case.
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
    ///
    /// The bound is [`load_tree`](crate::load_tree)'s, deliberately:
    /// `--tree` takes a directory or an archive, so a tarball of a
    /// directory tree expands to the tree that directory would have loaded
    /// as, rather than to one the walk would have refused.
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
    /// An archive expands to more bytes than one may allocate. Not a
    /// refusal.
    ///
    /// A desired tree holds every member's contents in memory, and a
    /// compressed archive chooses how many bytes that is: a few hundred
    /// kilobytes of gzipped zeros expand to gigabytes. The bound turns a
    /// decompression bomb into this error instead of an out-of-memory abort
    /// at plan time.
    #[error("archive {path} expands past the {limit} bytes an archive may allocate")]
    ArchiveTooLarge {
        /// The archive's location.
        path: Utf8PathBuf,
        /// The most an expansion may allocate, summed over its members.
        limit: u64,
    },
    /// An archive carries more members than an expansion places. Not a
    /// refusal. The byte bound does not see this shape: a million empty
    /// members expand to no bytes at all and still cost a map entry each.
    #[error("archive {path} carries more than the {limit} members an archive may hold")]
    ArchiveTooManyMembers {
        /// The archive's location.
        path: Utf8PathBuf,
        /// The most members an expansion accepts.
        limit: usize,
    },
    /// A source tree holds an entry whose name is not UTF-8, so
    /// [`load_tree`](crate::load_tree) cannot key it. Not a refusal: no
    /// destination path is being declined — the source carries content this
    /// crate cannot name. Observation *skips* such a name, because it can
    /// never match a desired or recorded path; a source tree's names are
    /// content the caller asked to project, so skipping one would drop it
    /// silently.
    #[error("tree entry name under {path} is not UTF-8: {name:?}")]
    TreeNameNotUtf8 {
        /// The absolute path of the directory holding the entry.
        path: Utf8PathBuf,
        /// The name as the filesystem gave it, lossily decoded. It is what
        /// can be *shown* of bytes that have no UTF-8 spelling, not those
        /// bytes: every invalid sequence renders as `U+FFFD`, so two names
        /// differing only there render alike, and a name that genuinely
        /// carries `U+FFFD` renders like one that does not. A `String` is
        /// the whole vocabulary this crate has for a path — desired and
        /// recorded paths are [`Utf8PathBuf`] by construction — and a name
        /// it cannot spell is one it can never project, so what the field
        /// carries is the message's, not a caller's to act on. `path` names
        /// the directory to look in.
        name: String,
    },
    /// A source tree holds a symlink whose target is not UTF-8, which
    /// [`Entry::Symlink`](crate::Entry::Symlink) — a `String` — cannot
    /// carry. Not a refusal, for the same reason as
    /// [`TreeNameNotUtf8`](Error::TreeNameNotUtf8).
    #[error("tree symlink {path} has a target that is not UTF-8: {target:?}")]
    TreeTargetNotUtf8 {
        /// The link's absolute path.
        path: Utf8PathBuf,
        /// The target as the filesystem gave it, lossily decoded, and what
        /// can be shown of it rather than the bytes themselves — the same
        /// terms as [`TreeNameNotUtf8::name`](Error::TreeNameNotUtf8).
        target: String,
    },
    /// A source tree nests deeper than [`load_tree`](crate::load_tree)
    /// walks. Not a refusal, for the same reason as
    /// [`TreeNameNotUtf8`](Error::TreeNameNotUtf8): the source carries a
    /// shape the load cannot take, and no destination path is being
    /// declined.
    ///
    /// The walk spends a stack frame per directory level, and depth is the
    /// source tree's to choose — every lookup is relative to an open
    /// directory handle, so a filesystem holds trees far deeper than a path
    /// naming them could be spelled, and a directory made its own ancestor
    /// by a bind mount has no bottom at all. The bound is what turns both
    /// into this error instead of a stack the walk runs off the end of.
    #[error("tree directory {path} nests deeper than the {limit} levels a source tree may carry")]
    TreeTooDeep {
        /// The absolute path of the directory one level past the limit.
        path: Utf8PathBuf,
        /// The deepest nesting the walk accepts, counted in directories
        /// below the source root.
        limit: usize,
    },
    /// A source tree holds a node of a kind the projection never writes — a
    /// FIFO, a socket, or a device node. Not a refusal: the load cannot
    /// produce a desired tree at all. A node the walk's `lstat` found to be
    /// one of those is never opened, since reading a FIFO with no writer
    /// would block forever.
    ///
    /// A name that became one of those kinds *after* that `lstat` reaches
    /// this variant only when the walk's open succeeded and the handle
    /// turned out not to hold a regular file — a FIFO, which opens for
    /// reading without waiting, or a directory. An open that fails outright
    /// is [`Io`](Error::Io) instead: a name that has become a symlink, which
    /// the walk will not follow, or a socket, which the OS declines to open
    /// through the filesystem at all.
    #[error("tree node {path} is not a file, directory, or symlink")]
    TreeNodeKind {
        /// The node's absolute path.
        path: Utf8PathBuf,
    },
    /// The plan touches a [`Block`](crate::EntryKind::Block) entry —
    /// writing one, or re-checking a block signature — and block regions
    /// are not implemented in [`apply`](crate::apply) yet. Not a refusal:
    /// the plan is well-formed — this crate cannot honor it yet. Reported
    /// before anything is written.
    #[error(
        "plan touches block entries, which apply does not implement yet: {}",
        join(paths)
    )]
    ApplyBlockUnimplemented {
        /// The planned block paths, relative to the destination.
        paths: BTreeSet<Utf8PathBuf>,
    },
}

impl Error {
    /// Whether this error is a refusal: the projection declining to touch
    /// a path ([`Drift`](Error::Drift), [`Foreign`](Error::Foreign),
    /// [`Containment`](Error::Containment),
    /// [`OwnerConflict`](Error::OwnerConflict),
    /// [`ExternalTarget`](Error::ExternalTarget),
    /// [`InvalidTarget`](Error::InvalidTarget),
    /// [`TreeConflict`](Error::TreeConflict)) rather than an operation
    /// failing. A CLI maps refusals to exit 2 and everything else to
    /// exit 1.
    pub fn is_refusal(&self) -> bool {
        match self {
            Error::Drift { .. }
            | Error::Foreign { .. }
            | Error::Containment { .. }
            | Error::OwnerConflict { .. }
            | Error::ExternalTarget { .. }
            | Error::InvalidTarget { .. }
            | Error::TreeConflict { .. } => true,
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
            | Error::ArchiveMemberDuplicate { .. }
            | Error::ArchiveMemberStripped { .. }
            | Error::ArchiveMemberTooDeep { .. }
            | Error::ArchiveTooLarge { .. }
            | Error::ArchiveTooManyMembers { .. }
            | Error::TreeNameNotUtf8 { .. }
            | Error::TreeTargetNotUtf8 { .. }
            | Error::TreeTooDeep { .. }
            | Error::TreeNodeKind { .. }
            | Error::ApplyBlockUnimplemented { .. } => false,
        }
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

fn join_links(links: &BTreeMap<Utf8PathBuf, String>) -> String {
    links
        .iter()
        .map(|(path, target)| format!("{path} -> {target}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// [`join_links`] with the target quoted and escaped, because the targets
/// this variant reports are the ones a bare rendering would hide: the empty
/// string prints as nothing, and a NUL byte prints as nothing visible.
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
