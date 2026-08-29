use std::collections::BTreeMap;
use std::fmt::Write;

use camino::Utf8PathBuf;
use serde::Serialize;
use sha2::{Digest, Sha256};

#[cfg(unix)]
use camino::Utf8Path;
#[cfg(unix)]
use cap_std::fs_utf8::{Dir, MetadataExt};

use crate::{Error, MAX_WALK_DEPTH, Manifest, Result};

/// Lowercase hex SHA-256 of `bytes` — the one hash convention, everywhere a
/// hash is recorded or compared ([`ManifestEntry::hash`]): a file hashes
/// its contents whole, a symlink hashes its target string, and a
/// [`Block`](crate::EntryKind::Block) entry hashes its region's body alone.
///
/// [`ManifestEntry::hash`]: crate::ManifestEntry::hash
pub fn sha256_hex(bytes: &[u8]) -> String {
    to_hex(&Sha256::digest(bytes))
}

/// Lowercase hex of a digest.
fn to_hex(digest: &[u8]) -> String {
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex
}

/// [`sha256_hex`] of everything `reader` yields, streamed through a
/// fixed-size buffer — peak memory stays at the copy buffer no matter how
/// large the file, so observing a destination that happens to hold a huge
/// foreign file never materializes it.
#[cfg(unix)]
pub(crate) fn sha256_hex_of_reader(mut reader: impl std::io::Read) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        hasher.update(&buffer[..read]);
    }
    Ok(to_hex(&hasher.finalize()))
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
pub(crate) struct Observations {
    /// Per-path observations, keyed by path relative to the destination.
    pub paths: BTreeMap<Utf8PathBuf, Observation>,
}

/// What one path in the union of the destination directory and the
/// manifest looked like on disk, with lstat semantics: a symlink is
/// observed as itself, never as what it points at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) enum Observation {
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
    /// The managed region inside a regular file recorded as a
    /// [`Block`](crate::EntryKind::Block) — never the container around it.
    /// Observing a region takes the manifest's marker and placement, so this
    /// variant appears at recorded block paths alone; an unrecorded container
    /// is an ordinary [`File`](Self::File).
    Block {
        /// [`sha256_hex`] of the region's body, or `None` where the container
        /// holds no marker occurrence — the region is gone, which classifies
        /// [`Missing`](crate::PathState::Missing) like any other absent node.
        hash: Option<String>,
        /// Whether the author's side — the container with the region stripped
        /// — is empty or ends with `\n`, which is what
        /// [`Append`](crate::Placement::Append) requires of it. Free from the
        /// same read.
        newline_terminated: bool,
        /// How many whole-line marker occurrences the container holds. One is
        /// the ordinary reading: the body may carry no marker line, so a
        /// container the projection alone has written holds exactly one. More
        /// than one and the marker no longer says which bytes are the
        /// projection's — the region is found by taking an extreme
        /// occurrence, and somebody else has written another. Such a
        /// container identifies no region, so it classifies
        /// [`Drifted`](crate::PathState::Drifted) and every action on it
        /// refuses under either policy ([`EntryKind::Block`](crate::EntryKind::Block)).
        occurrences: usize,
    },
}

/// What one container looked like through the single descriptor
/// [`read_container`] opened.
#[cfg(unix)]
pub(crate) enum Container {
    /// A regular file: its bytes and the mode to preserve across the rename
    /// that republishes it, both taken from that one descriptor.
    File {
        /// The container's bytes, read whole — locating a marker line and
        /// splicing a region both need them together.
        bytes: Vec<u8>,
        /// The container's permission bits. The mode is the author's: it is
        /// never taken from the entry, and a `chmod` of the container is not
        /// drift.
        mode: u32,
    },
    /// Nothing at the path.
    Absent,
    /// Something that is not a regular file — a directory, a FIFO, or a
    /// symlink, which `O_NOFOLLOW` declines to open rather than resolve.
    Other,
}

/// Opens the container at `name` inside `dir` and takes the regular-file
/// verdict, the mode, and the bytes from that one descriptor.
///
/// One file description does the whole read, rather than a stat followed by
/// an open followed by a read: a name swapped between two of those lookups
/// would have the verdict describe one file and the bytes another. The open
/// refuses to follow a final symlink and waits for no writer, so a link or a
/// FIFO substituted for the container is answered rather than resolved or
/// parked on.
///
/// `path` is where errors are reported at, relative to the destination.
#[cfg(unix)]
pub(crate) fn read_container(dir: &Dir, name: &str, path: &Utf8Path) -> Result<Container> {
    use std::io::Read as _;
    use std::os::unix::fs::PermissionsExt as _;

    let mut file = match crate::tree::open_file_nofollow(dir.as_cap_std(), name) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Container::Absent),
        // `O_NOFOLLOW` met a symlink at the final component. Spelled as the
        // raw errno because `ErrorKind::FilesystemLoop` is still unstable.
        Err(e) if e.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) => {
            return Ok(Container::Other);
        }
        Err(e) => return Err(io_error(path)(e)),
    };
    let meta = file.metadata().map_err(io_error(path))?;
    if !meta.is_file() {
        return Ok(Container::Other);
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(io_error(path))?;
    Ok(Container::File {
        bytes,
        // Permission bits only. setuid, setgid and sticky are dropped: the
        // container is untrusted content, and `docs/security.lex` section 1
        // says content never widens what content may do — the same rule that
        // lets an archive member contribute its executable bit and nothing
        // else (section 4). Publishing replaces the inode, so preserving them
        // would re-create somebody else's setuid file under the invoker's
        // ownership.
        mode: meta.permissions().mode() & 0o0777,
    })
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
///   will need to compare — streaming each file through the hasher, so
///   peak memory never scales with file size;
/// - reads the *region* rather than the file at a path the manifest records
///   as a [`Block`](crate::EntryKind::Block): the manifest carries the marker
///   and the placement, so the walk hashes the body alone and an edit
///   elsewhere in the container never reads as drift. That container is read
///   whole, since locating a marker line needs its bytes together;
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
///
/// One shape of destination fails the observation rather than being read:
/// one nesting more than [`MAX_WALK_DEPTH`] directories below `dest`, which
/// is [`Error::DestinationTooDeep`] naming the directory a level past that —
/// a failure, not a refusal, since no path is being declined. The walk
/// spends a stack frame per level and the destination chooses the depth — a
/// mount loop under it has no bottom, and every level of one is a real
/// directory no symlink check would stop at — so unbounded, a deep enough
/// destination would run the stack off its end and abort the process before
/// any caller could report anything. Depth is an error and not a skipped
/// subtree for the same reason an unreadable entry is.
///
/// The limit is the one [`apply`](crate::apply) refuses to write past and
/// the one [`load_tree`](crate::load_tree) walks a source tree by, so a
/// destination holding what this projection wrote is inside it, and a
/// destination fails this way over what the projection did not write:
/// foreign nesting, or a mount loop.
#[cfg(unix)]
pub(crate) fn observe(dest: &Dir, manifest: &Manifest) -> Result<Observations> {
    let mut paths = BTreeMap::new();
    walk(dest, Utf8Path::new(""), 0, manifest, &mut paths)?;
    for path in manifest.entries.keys() {
        paths.entry(path.clone()).or_insert(Observation::Absent);
    }
    Ok(Observations { paths })
}

/// Observes every entry of `dir` — the destination subdirectory at
/// `prefix`, `depth` directory levels below the destination root — into
/// `into`, recursing into real subdirectories via handles opened from `dir`,
/// so every open stays anchored to the destination handle and no path is
/// resolved from the ambient filesystem.
///
/// Past [`MAX_WALK_DEPTH`] levels the walk stops and names the directory it
/// stopped at: the recursion's frames are this process's stack, and the
/// destination is free to nest without end.
#[cfg(unix)]
fn walk(
    dir: &Dir,
    prefix: &Utf8Path,
    depth: usize,
    manifest: &Manifest,
    into: &mut BTreeMap<Utf8PathBuf, Observation>,
) -> Result<()> {
    let dir_path = if prefix.as_str().is_empty() {
        Utf8Path::new(".")
    } else {
        prefix
    };
    if depth > MAX_WALK_DEPTH {
        return Err(Error::DestinationTooDeep {
            path: dir_path.to_owned(),
            limit: MAX_WALK_DEPTH,
        });
    }
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
            walk(&sub, &rel, depth + 1, manifest, into)?;
            continue;
        } else if file_type.is_file() {
            match manifest
                .entries
                .get(&rel)
                .and_then(|recorded| crate::block::block_kind(&recorded.kind))
            {
                Some((marker, placement)) => observe_region(dir, &name, &rel, marker, placement)?,
                None => {
                    let file = entry.open().map_err(io_error(&rel))?;
                    Observation::File {
                        hash: sha256_hex_of_reader(file).map_err(io_error(&rel))?,
                        executable: meta.mode() & 0o100 != 0,
                    }
                }
            }
        } else {
            Observation::Other
        };
        into.insert(rel, observation);
    }
    Ok(())
}

/// Observes the region recorded at `rel` inside the container named `name`:
/// the body's hash where the container holds a marker occurrence, whether the
/// author's side is newline-terminated either way, and how many occurrences
/// the container holds at all.
///
/// The container is read whole rather than streamed, because locating a
/// marker line needs the bytes together — so this is the one read whose peak
/// memory is a file's size, and it happens only at paths the manifest already
/// records as blocks. A container that is no longer a regular file observes
/// as [`Other`](Observation::Other), which drifts against the record.
#[cfg(unix)]
fn observe_region(
    dir: &Dir,
    name: &str,
    rel: &Utf8Path,
    marker: &str,
    placement: crate::Placement,
) -> Result<Observation> {
    let Container::File { bytes, .. } = read_container(dir, name, rel)? else {
        return Ok(Observation::Other);
    };
    let region = crate::block::locate(&bytes, marker, placement);
    Ok(Observation::Block {
        hash: region
            .as_ref()
            .map(|region| sha256_hex(&bytes[region.body.clone()])),
        newline_terminated: crate::block::newline_terminated(&crate::block::strip(
            &bytes,
            region.as_ref(),
        )),
        occurrences: crate::block::occurrence_count(&bytes, marker),
    })
}

/// Wraps an OS error as [`Error::Io`] at `path` (relative to the
/// destination).
#[cfg(unix)]
pub(crate) fn io_error(path: &Utf8Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_owned(),
        source,
    }
}

#[cfg(all(test, unix))]
#[path = "observe_tests.rs"]
mod tests;
