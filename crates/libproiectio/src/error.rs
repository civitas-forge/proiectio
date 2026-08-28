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
    /// does not make the path foreign.
    #[error(
        "refusing to touch foreign paths (not written by this projection): {}",
        join(paths)
    )]
    Foreign {
        /// The foreign paths, relative to the destination.
        paths: BTreeSet<Utf8PathBuf>,
    },
    /// Refusal: desired-tree paths the projection may not write — paths
    /// that escape the destination (absolute paths, paths climbing out via
    /// `..`, or empty or `.` components), writes through a symlinked
    /// ancestor, or paths entering the projection's own state directory.
    #[error("refusing paths that escape the destination: {}", join(paths))]
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
    /// Refusal: symlinks whose targets resolve outside the destination.
    /// Like [`Foreign`](Error::Foreign), no policy lifts this.
    #[error(
        "refusing symlinks with targets outside the destination: {}",
        join_links(links)
    )]
    ExternalTarget {
        /// The offending links: path of each link, relative to the
        /// destination, mapped to its target string verbatim.
        links: BTreeMap<Utf8PathBuf, String>,
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
}

impl Error {
    /// Whether this error is a refusal: the projection declining to touch
    /// a path ([`Drift`](Error::Drift), [`Foreign`](Error::Foreign),
    /// [`Containment`](Error::Containment),
    /// [`OwnerConflict`](Error::OwnerConflict),
    /// [`ExternalTarget`](Error::ExternalTarget)) rather than an
    /// operation failing. A CLI maps refusals to exit 2 and everything
    /// else to exit 1.
    pub fn is_refusal(&self) -> bool {
        match self {
            Error::Drift { .. }
            | Error::Foreign { .. }
            | Error::Containment { .. }
            | Error::OwnerConflict { .. }
            | Error::ExternalTarget { .. } => true,
            Error::Io { .. } | Error::ManifestFormat { .. } | Error::ManifestVersion { .. } => {
                false
            }
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

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
