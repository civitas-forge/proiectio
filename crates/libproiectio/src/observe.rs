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

/// Lowercase hex SHA-256 of `bytes` — the hash convention everywhere a hash
/// is recorded: file contents, a symlink's target string, a block's body.
pub fn sha256_hex(bytes: &[u8]) -> String {
    to_hex(&Sha256::digest(bytes))
}

fn to_hex(digest: &[u8]) -> String {
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex
}

/// [`sha256_hex`] of everything `reader` yields, streamed through a
/// fixed-size buffer so peak memory never scales with the file.
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
/// and the manifest, keyed by path relative to the destination. Paths whose
/// on-disk names are not UTF-8 never appear.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Observations {
    pub paths: BTreeMap<Utf8PathBuf, Observation>,
}

/// What one path looked like on disk, with lstat semantics: a symlink is
/// observed as itself, never as what it points at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) enum Observation {
    /// Recorded in the manifest, but not reached by the walk.
    Absent,
    /// A regular file.
    File {
        /// [`sha256_hex`] of the file's contents.
        hash: String,
        /// Whether the owner-executable bit is set.
        executable: bool,
    },
    /// A symbolic link, observed as itself.
    Symlink {
        /// [`sha256_hex`] of the raw target bytes.
        hash: String,
        /// The target string verbatim, or `None` when the on-disk target
        /// is not UTF-8.
        target: Option<String>,
    },
    /// A directory.
    Directory,
    /// A FIFO, socket, or device node — never opened or hashed.
    Other,
    /// The managed region inside a regular file recorded as a
    /// [`Block`](crate::EntryKind::Block), located with the manifest's marker
    /// and placement; an unrecorded container is an ordinary [`File`](Self::File).
    Block {
        /// [`sha256_hex`] of the region's body, or `None` where the container
        /// holds no marker occurrence.
        hash: Option<String>,
        /// Whether the container with the region stripped is empty or ends
        /// with `\n`.
        newline_terminated: bool,
        /// How many whole-line marker occurrences the container holds; more
        /// than one identifies no region.
        occurrences: usize,
    },
}

/// What one container looked like through the single descriptor
/// [`read_container`] opened.
#[cfg(unix)]
pub(crate) enum Container {
    /// A regular file.
    File {
        /// The container's bytes, read whole.
        bytes: Vec<u8>,
        /// The container's permission bits, which are the author's and are
        /// preserved across the rename that republishes it.
        mode: u32,
    },
    /// Nothing at the path.
    Absent,
    /// Something that is not a regular file — a directory, a FIFO, or a
    /// symlink, which `O_NOFOLLOW` declines to open rather than resolve.
    Other,
}

/// Opens the container at `name` inside `dir` and takes the regular-file
/// verdict, the mode, and the bytes from that one descriptor. `path` is where
/// errors are reported at, relative to the destination.
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
        // Permission bits only: setuid, setgid and sticky are dropped.
        mode: meta.permissions().mode() & 0o0777,
    })
}

/// Walks the union of the destination directory and the manifest and
/// snapshots what is on disk into [`Observations`], reading everything
/// through the capability handle `dest`.
///
/// Symlinks are observed as themselves and never entered, so a recorded path
/// beneath one observes [`Observation::Absent`]. Entries whose names are not
/// UTF-8 are skipped; any other unreadable entry is an [`Error::Io`] carrying
/// the path relative to the destination (`.` for the destination itself).
/// A destination nesting more than [`MAX_WALK_DEPTH`] directories below
/// `dest` is [`Error::DestinationTooDeep`] naming the directory a level past
/// that. The projection's own state subtree is not excluded here.
#[cfg(unix)]
pub(crate) fn observe(dest: &Dir, manifest: &Manifest) -> Result<Observations> {
    let mut paths = BTreeMap::new();
    walk(dest, Utf8Path::new(""), 0, manifest, &mut paths)?;
    for path in manifest.entries.keys() {
        paths.entry(path.clone()).or_insert(Observation::Absent);
    }
    Ok(Observations { paths })
}

/// Observes every entry of `dir` — the destination subdirectory at `prefix`,
/// `depth` levels below the root — into `into`, recursing through handles
/// opened from `dir` so no path is resolved from the ambient filesystem.
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
            continue;
        };
        let rel = prefix.join(&name);
        let meta = entry.metadata().map_err(io_error(&rel))?;
        let file_type = meta.file_type();
        let observation = if file_type.is_symlink() {
            // The plain-Dir view returns the target bytes raw; the fs_utf8
            // wrapper errors on a non-UTF-8 target.
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

/// Observes the region recorded at `rel` inside the container named `name`.
/// A container that is no longer a regular file observes as
/// [`Other`](Observation::Other).
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

/// Wraps an OS error as [`Error::Io`] at `path`, relative to the destination.
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
