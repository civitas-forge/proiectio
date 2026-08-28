use std::collections::BTreeMap;
use std::fmt::Write;

use camino::Utf8PathBuf;
use serde::Serialize;
use sha2::{Digest, Sha256};

#[cfg(unix)]
use camino::Utf8Path;
#[cfg(unix)]
use cap_std::fs_utf8::{Dir, MetadataExt};

use crate::{Error, Manifest, Result};

/// Lowercase hex SHA-256 of `bytes` — the one hash convention, everywhere a
/// hash is recorded or compared ([`ManifestEntry::hash`]): a file hashes
/// its contents whole, a symlink hashes its target string, and a
/// [`Block`](crate::EntryKind::Block) entry hashes the body between the
/// delimiter lines alone.
///
/// [`ManifestEntry::hash`]: crate::ManifestEntry::hash
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex
}

/// What observe saw at every path in the union of the destination directory
/// and the manifest — the observe→decide seam.
///
/// Plain serializable data, keyed by path relative to the destination:
/// deciding consumes this snapshot together with the desired tree and the
/// manifest and touches no filesystem itself, so every judgment it makes is
/// reproducible from the snapshot alone. The map covers what UTF-8 can
/// name; a non-UTF-8 entry on disk can never match a desired or recorded
/// path, so it never appears here (`docs/design.lex` section 2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Observations {
    /// Per-path observations, keyed by path relative to the destination.
    pub paths: BTreeMap<Utf8PathBuf, Observation>,
}

/// What one path in the union of the destination directory and the
/// manifest looked like on disk, with lstat semantics: a symlink is
/// observed as itself, never as what it points at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Observation {
    /// Recorded in the manifest, but not reached by the walk: gone from
    /// disk — or beneath an ancestor that is no longer a real directory,
    /// which the walk never reads through.
    Absent,
    /// A regular file.
    File {
        /// [`sha256_hex`] of the file's contents.
        hash: String,
        /// Whether the owner-executable bit is set.
        executable: bool,
    },
    /// A symbolic link, observed as itself — never followed.
    Symlink {
        /// [`sha256_hex`] of the raw target bytes. For a UTF-8 target this
        /// equals the hash of `target`'s string, matching the manifest
        /// convention; for a non-UTF-8 target it can match no recorded
        /// hash, so a recorded link whose target was edited to such bytes
        /// compares as drifted instead of failing the walk.
        hash: String,
        /// The target string verbatim, or `None` when the on-disk target
        /// is not UTF-8.
        target: Option<String>,
    },
    /// A directory. Nothing to hash: directories carry no recorded entry.
    Directory,
    /// A node of a kind the projection never writes — a FIFO, socket, or
    /// device node. Never opened or hashed (reading a writerless FIFO
    /// would block forever); observed so the deciding stage sees the path
    /// is occupied rather than mistaking it for absent.
    Other,
}

/// The read-only stage: walks the union of the destination directory and
/// the manifest and snapshots what is on disk into [`Observations`].
///
/// `dest` is a capability handle rooted at the destination — every read
/// goes through it, so nothing outside the destination is ever opened.
/// cap-std has no read-only handle type, so "observe writes nothing" is a
/// discipline this function keeps and its tests check, not a type-level
/// guarantee (`docs/implementation.lex` section 3).
///
/// The walk:
///
/// - uses lstat semantics throughout: a symlink is observed as itself,
///   target verbatim, and nothing beneath it is entered — so a recorded
///   path whose on-disk ancestor is a symlink observes as
///   [`Observation::Absent`] rather than being read through the link;
/// - hashes every regular file it can name, recorded or not — observe
///   never sees the desired tree, so it cannot know which paths deciding
///   will need to compare;
/// - skips entries whose names are not UTF-8: desired and recorded paths
///   are `Utf8PathBuf` by construction, so such an entry can never match a
///   row of the classification and stays invisible to it;
/// - reads link targets raw, via the plain-`Dir` view, so a target edited
///   to non-UTF-8 bytes still observes (with `target: None`) instead of
///   failing the walk;
/// - completes the union by inserting [`Observation::Absent`] for every
///   recorded path it did not reach.
///
/// The projection's own state subtree is *not* excluded here: observe does
/// not know where the state directory lives. Excluding it from
/// classification is the deciding stage's job.
///
/// `BTreeMap` makes the result deterministic regardless of directory read
/// order. Errors are [`Error::Io`] carrying the path relative to the
/// destination (`.` for the destination itself); an unreadable entry is an
/// error, not a skip, because a snapshot that silently omitted paths would
/// let a later stage mistake unreadable for absent.
#[cfg(unix)]
pub fn observe(dest: &Dir, manifest: &Manifest) -> Result<Observations> {
    let mut paths = BTreeMap::new();
    walk(dest, Utf8Path::new(""), &mut paths)?;
    for path in manifest.entries.keys() {
        paths.entry(path.clone()).or_insert(Observation::Absent);
    }
    Ok(Observations { paths })
}

/// Observes every entry of `dir` — the destination subdirectory at
/// `prefix` — into `into`, recursing into real subdirectories via handles
/// opened from `dir`, so every open stays anchored to the destination
/// handle and no path is resolved from the ambient filesystem.
#[cfg(unix)]
fn walk(dir: &Dir, prefix: &Utf8Path, into: &mut BTreeMap<Utf8PathBuf, Observation>) -> Result<()> {
    let dir_path = if prefix.as_str().is_empty() {
        Utf8Path::new(".")
    } else {
        prefix
    };
    let entries = dir.entries().map_err(io_error(dir_path))?;
    for entry in entries {
        let entry = entry.map_err(io_error(dir_path))?;
        let Ok(name) = entry.file_name() else {
            // Not UTF-8: invisible to classification (`docs/design.lex`
            // section 2), so not an error — there is nothing to report it as.
            continue;
        };
        let rel = prefix.join(&name);
        let meta = entry.metadata().map_err(io_error(&rel))?;
        let file_type = meta.file_type();
        let observation = if file_type.is_symlink() {
            // The plain-Dir view returns the target bytes raw; the fs_utf8
            // wrapper would error on a non-UTF-8 target, and an edited
            // target must observe as drift bait, not fail the walk.
            let target = dir
                .as_cap_std()
                .read_link_contents(&name)
                .map_err(io_error(&rel))?;
            let bytes = std::os::unix::ffi::OsStrExt::as_bytes(target.as_os_str());
            Observation::Symlink {
                hash: sha256_hex(bytes),
                target: String::from_utf8(bytes.to_vec()).ok(),
            }
        } else if file_type.is_dir() {
            let sub = entry.open_dir().map_err(io_error(&rel))?;
            into.insert(rel.clone(), Observation::Directory);
            walk(&sub, &rel, into)?;
            continue;
        } else if file_type.is_file() {
            let contents = dir.read(&name).map_err(io_error(&rel))?;
            Observation::File {
                hash: sha256_hex(&contents),
                executable: meta.mode() & 0o100 != 0,
            }
        } else {
            Observation::Other
        };
        into.insert(rel, observation);
    }
    Ok(())
}

/// Wraps an OS error as [`Error::Io`] at `path` (relative to the
/// destination).
#[cfg(unix)]
fn io_error(path: &Utf8Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_owned(),
        source,
    }
}

#[cfg(all(test, unix))]
#[path = "observe_tests.rs"]
mod tests;
