use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write;

use camino::Utf8PathBuf;
use serde::Serialize;
use sha2::{Digest, Sha256};

use camino::Utf8Path;
use cap_std::fs_utf8::{Dir, MetadataExt};

use crate::containment::contained_target;
use crate::{EntryKind, Error, IoRole, MAX_WALK_DEPTH, Manifest, Refusal, Result};

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
/// on-disk names are not UTF-8 never appear in [`paths`](Self::paths); the
/// directories holding them appear in [`unreadable`](Self::unreadable), so a
/// reader can tell an inventory that is complete from one that is not.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Observations {
    pub paths: BTreeMap<Utf8PathBuf, Observation>,
    /// Every directory the walk met a name in that it cannot represent, so
    /// [`paths`](Self::paths) is not the whole of what stands there. Keyed
    /// like every other path, which makes the destination root the empty
    /// path. Nothing may conclude such a directory empties.
    pub unreadable: BTreeSet<Utf8PathBuf>,
    pub pruned_components: BTreeSet<String>,
}

impl Observations {
    pub(crate) fn is_pruned(&self, path: &Utf8Path) -> bool {
        crate::is_pruned(path, &self.pruned_components)
    }
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
        /// [`sha256_hex`] of the raw target bytes. A non-UTF-8 target can
        /// match no recorded hash, so a recorded link edited to such bytes
        /// compares as drifted instead of failing the walk.
        hash: String,
        /// The target string verbatim, or `None` when the on-disk target
        /// is not UTF-8.
        target: Option<String>,
    },
    /// A directory.
    Directory,
    /// A FIFO, socket, or device node — never opened or hashed. Opening a
    /// FIFO with no writer blocks forever.
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
        desired: Option<DesiredRegion>,
    },
}

/// What one container looked like through the single descriptor
/// [`read_container`] opened.
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
///
/// One file description does the whole read, so a name swapped between a stat
/// and an open cannot have the verdict describe one file and the bytes
/// another.
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct DesiredRegion {
    pub(crate) occurrences: usize,
    pub(crate) hash: Option<String>,
    pub(crate) author_newline_terminated: bool,
}

pub(crate) type BlockMarkers = BTreeMap<Utf8PathBuf, (String, crate::Placement)>;

pub(crate) fn block_markers(desired: &crate::Desired) -> BlockMarkers {
    desired
        .iter()
        .filter_map(|(path, entry)| match entry {
            crate::Entry::Block {
                marker, placement, ..
            } => Some((path.clone(), (marker.clone(), *placement))),
            _ => None,
        })
        .collect()
}

/// Walks the union of the destination directory and the manifest and
/// snapshots what is on disk into [`Observations`], reading everything
/// through the capability handle `dest`. A component in `pruned_components`
/// is skipped before metadata is read, at every depth. The projection's own
/// state subtree is not excluded here.
///
/// cap-std has no read-only handle type, so "this stage writes nothing"
/// (`docs/dev/implementation.lex` §1) is a discipline the walk keeps and its
/// tests check, not a guarantee the types carry.
///
/// Symlinks are observed as themselves and never entered, so a recorded path
/// beneath one observes [`Observation::Absent`]. An entry that cannot be
/// read is an [`Error::Io`] at the path relative to the destination (`.` for
/// the destination itself); nesting past [`MAX_WALK_DEPTH`] is
/// [`Error::DestinationTooDeep`].
#[cfg(test)]
pub(crate) fn observe(
    dest: &Dir,
    manifest: &Manifest,
    wanted: &BlockMarkers,
) -> Result<Observations> {
    observe_scoped(dest, manifest, wanted, &BTreeSet::new())
}

pub(crate) fn observe_scoped(
    dest: &Dir,
    manifest: &Manifest,
    wanted: &BlockMarkers,
    pruned_components: &BTreeSet<String>,
) -> Result<Observations> {
    let mut into = Observations {
        pruned_components: pruned_components.clone(),
        ..Observations::default()
    };
    walk(
        dest,
        Utf8Path::new(""),
        0,
        manifest,
        wanted,
        pruned_components,
        &mut into,
    )?;
    for path in manifest.entries.keys() {
        if into.is_pruned(path) {
            continue;
        }
        into.paths
            .entry(path.clone())
            .or_insert(Observation::Absent);
    }
    relocated_regions(dest, manifest, wanted, &mut into)?;
    Ok(into)
}

/// States the region of every block record whose ancestry walks out through
/// a recorded link, under the record's own key and read out of the container
/// the walk comes out at — [`walk`] itself only parses a container under the
/// key standing at the path it is walking, and two keys reaching one
/// container each hold a region of their own there.
fn relocated_regions(
    dest: &Dir,
    manifest: &Manifest,
    wanted: &BlockMarkers,
    into: &mut Observations,
) -> Result<()> {
    let mut regions: BTreeMap<Utf8PathBuf, Observation> = BTreeMap::new();
    for (path, recorded) in &manifest.entries {
        if into.is_pruned(path) {
            continue;
        }
        let Some((marker, placement)) = crate::block::block_kind(&recorded.kind) else {
            continue;
        };
        let Ok(Some(landing)) = walked_ancestry(path, manifest, into, &BTreeSet::new(), false)
        else {
            continue;
        };
        if landing.at == *path {
            continue;
        }
        if into.is_pruned(&landing.at) {
            continue;
        }
        // A block record at the landing already had its region parsed, and
        // only a regular file holds a region at all.
        if manifest
            .entries
            .get(&landing.at)
            .is_some_and(|recorded| recorded.kind.is_block())
            || !matches!(
                into.paths.get(&landing.at),
                Some(Observation::File { .. } | Observation::Block { .. })
            )
        {
            continue;
        }
        let observation = observe_region(
            dest,
            landing.at.as_str(),
            &landing.at,
            Some((marker, placement)),
            wanted.get(path),
        )?;
        regions.insert(path.clone(), observation);
    }
    into.paths.extend(regions);
    Ok(())
}

/// Where a no-follow walk to `path` comes out on the destination this run
/// leaves — `path` itself, unless the walk followed a recorded link — or the
/// refusal it meets on the way. `Ok(None)` is ancestry that is not there.
///
/// The snapshot side of act's `verified_parent`: the same arms in the same
/// order, read off the observations rather than the disk, so a verdict apply
/// would reach is one deciding reaches first. `create` marks the walk a write
/// makes, which builds missing ancestry and refuses a node that is not a
/// directory rather than stopping short of the leaf. `vacated` names the
/// locations this run unlinks, which stand in the way of nothing.
pub(crate) fn walked_ancestry(
    path: &Utf8Path,
    manifest: &Manifest,
    observations: &Observations,
    vacated: &BTreeSet<Utf8PathBuf>,
    create: bool,
) -> std::result::Result<Option<Landing>, Refusal> {
    let mut components: VecDeque<String> = path
        .components()
        .map(|component| component.as_str().to_owned())
        .collect();
    let leaf = components
        .pop_back()
        .expect("a decided path has a final component");
    let mut prefix = Utf8PathBuf::new();
    let mut through: Option<Utf8PathBuf> = None;
    let mut visited: BTreeSet<Utf8PathBuf> = BTreeSet::new();
    while let Some(component) = components.pop_front() {
        let here = prefix.join(&component);
        if observations.is_pruned(&here) {
            return Err(Refusal::Containment {
                through: through.clone(),
            });
        }
        let standing = if vacated.contains(&here) {
            None
        } else {
            observations.paths.get(&here)
        };
        match standing {
            None | Some(Observation::Absent) => {
                if !create {
                    return Ok(None);
                }
            }
            Some(Observation::Directory) => {}
            Some(Observation::Symlink { hash, target }) => {
                let recorded = manifest
                    .entries
                    .get(&here)
                    .filter(|recorded| recorded.kind == EntryKind::Symlink);
                let Some(recorded) = recorded else {
                    return Err(through_link(here));
                };
                if *hash != recorded.hash {
                    return Err(Refusal::Drift);
                }
                let Some(target) = target else {
                    return Err(through_link(here));
                };
                let Some(resolved) = contained_target(&prefix, target) else {
                    return Err(through_link(here));
                };
                if !visited.insert(here.clone()) {
                    return Err(through_link(here));
                }
                let mut restarted: VecDeque<String> = resolved
                    .components()
                    .map(|component| component.as_str().to_owned())
                    .collect();
                restarted.append(&mut components);
                components = restarted;
                through.get_or_insert(here);
                prefix = Utf8PathBuf::new();
                continue;
            }
            Some(Observation::File { .. } | Observation::Block { .. } | Observation::Other) => {
                if !create {
                    return Ok(None);
                }
                return Err(if manifest.entries.contains_key(&here) {
                    Refusal::Drift
                } else {
                    Refusal::Foreign
                });
            }
        }
        prefix = here;
    }
    Ok(Some(Landing {
        at: prefix.join(leaf),
        through,
    }))
}

/// Where a walk came out, and the first recorded link it followed to get
/// there — the one that explains a landing the caller did not ask for.
pub(crate) struct Landing {
    pub(crate) at: Utf8PathBuf,
    pub(crate) through: Option<Utf8PathBuf>,
}

/// The refusal a landing raises where the manifest records it: unlinking
/// there takes a node the landing's owners hold. `None` where the walk
/// followed no link, or where nothing records where it came out.
pub(crate) fn recorded_landing(landing: &Landing, manifest: &Manifest) -> Option<Refusal> {
    let through = landing.through.clone()?;
    let recorded = manifest.entries.get(&landing.at)?;
    Some(Refusal::RecordedLanding {
        through,
        at: landing.at.clone(),
        owners: recorded.owners.clone(),
    })
}

fn through_link(link: Utf8PathBuf) -> Refusal {
    Refusal::Containment {
        through: Some(link),
    }
}

/// Observes every entry of `dir` — the destination subdirectory at `prefix`,
/// `depth` levels below the root — into `into`, recursing through handles
/// opened from `dir` so no path is resolved from the ambient filesystem.
fn walk(
    dir: &Dir,
    prefix: &Utf8Path,
    depth: usize,
    manifest: &Manifest,
    wanted: &BlockMarkers,
    pruned_components: &BTreeSet<String>,
    into: &mut Observations,
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
            // No key names this entry, so the directory holding it is one
            // whose contents these observations do not state in full.
            into.unreadable.insert(prefix.to_owned());
            continue;
        };
        if pruned_components.contains(name.as_str()) {
            continue;
        }
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
            into.paths.insert(rel.clone(), Observation::Directory);
            walk(
                &sub,
                &rel,
                depth + 1,
                manifest,
                wanted,
                pruned_components,
                into,
            )?;
            continue;
        } else if file_type.is_file() {
            let recorded = manifest.entries.get(&rel);
            let recorded_block =
                recorded.and_then(|recorded| crate::block::block_kind(&recorded.kind));
            let desired_block = wanted.get(&rel);
            match (recorded_block, recorded.is_none(), desired_block) {
                (Some((marker, placement)), _, desired) => {
                    observe_region(dir, &name, &rel, Some((marker, placement)), desired)?
                }
                (None, true, Some(desired)) => {
                    observe_region(dir, &name, &rel, None, Some(desired))?
                }
                _ => {
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
        into.paths.insert(rel, observation);
    }
    Ok(())
}

/// Observes the region recorded at `rel` inside the container named `name`.
/// A container that is no longer a regular file observes as
/// [`Other`](Observation::Other).
fn observe_region(
    dir: &Dir,
    name: &str,
    rel: &Utf8Path,
    recorded: Option<(&str, crate::Placement)>,
    desired: Option<&(String, crate::Placement)>,
) -> Result<Observation> {
    let Container::File { bytes, .. } = read_container(dir, name, rel)? else {
        return Ok(Observation::Other);
    };
    let region =
        recorded.and_then(|(marker, placement)| crate::block::locate(&bytes, marker, placement));
    let author = crate::block::strip(&bytes, region.as_ref());
    Ok(Observation::Block {
        hash: region
            .as_ref()
            .map(|region| sha256_hex(&bytes[region.body.clone()])),
        newline_terminated: crate::block::newline_terminated(&author),
        occurrences: recorded.map_or(0, |(marker, _)| {
            crate::block::occurrence_count(&bytes, marker)
        }),
        desired: desired.map(|(marker, placement)| {
            let region = crate::block::locate(&bytes, marker, *placement);
            DesiredRegion {
                occurrences: crate::block::occurrence_count(&author, marker),
                hash: region
                    .as_ref()
                    .map(|region| sha256_hex(&bytes[region.body.clone()])),
                author_newline_terminated: crate::block::newline_terminated(&crate::block::strip(
                    &bytes,
                    region.as_ref(),
                )),
            }
        }),
    })
}

/// Wraps an OS error as [`Error::Io`] at `path`, relative to the destination.
pub(crate) fn io_error(path: &Utf8Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |source| Error::Io {
        role: IoRole::Unstated,
        path: path.to_owned(),
        source,
    }
}

#[cfg(test)]
#[path = "observe_tests.rs"]
mod tests;
