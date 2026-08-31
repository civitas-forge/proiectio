use std::collections::{BTreeMap, BTreeSet};
use std::os::fd::AsFd;
use std::path::Path;

use camino::{Utf8Path, Utf8PathBuf};
use cap_primitives::fs::{FollowSymlinks, OpenOptions};
use cap_std::ambient_authority;
use cap_std::fs::Dir;

use crate::limits::Budget;
use crate::{
    Desired, Entry, Error, IoRole, Limits, MAX_WALK_DEPTH, Origin, Refusal, Refused, Result,
};

/// Walks `source` into a desired tree: every regular file becomes an
/// [`Entry::File`] with its bytes and owner-executable bit, every symlink an
/// [`Entry::Symlink`] carrying the target verbatim, keyed relative to
/// `source`. Directories carry no entry of their own.
///
/// Walked keys the containment gateway refuses are aggregated into one
/// [`Refusal::Containment`]; a name or target that is not UTF-8, a node kind
/// the projection never writes, a tree nesting past [`MAX_WALK_DEPTH`], and a
/// walk whose contents sum past [`Limits::max_source_bytes`] each fail the
/// load. That sum is everything the walk holds: file bytes, the key each
/// entry is filed under, each symlink's target, and each name containment
/// refused.
pub fn load_tree(source: &Utf8Path, limits: Limits) -> Result<Desired> {
    let source = crate::absolutize(source)?;
    let source = source.as_path();
    let root = Dir::open_ambient_dir(source, ambient_authority()).map_err(|e| Error::Io {
        role: IoRole::SourceTree,
        path: source.to_owned(),
        source: e,
    })?;
    let budget = Budget::new(limits);
    let mut walk = Walk {
        source,
        tree: BTreeMap::new(),
        refused: BTreeSet::new(),
        budget: &budget,
    };
    walk.descend(&root, Utf8Path::new(""), 0)?;
    if !walk.refused.is_empty() {
        let origin = Origin::Tree {
            path: source.to_owned(),
        };
        return Err(Refused::aggregate(
            walk.refused
                .into_iter()
                .map(|path| (path, Refusal::Containment { through: None }, origin.clone())),
        )
        .expect("refused is not empty")
        .into());
    }
    Ok(Desired::from_source(
        walk.tree,
        Origin::Tree {
            path: source.to_owned(),
        },
    ))
}

/// One [`load_tree`] walk: the tree built so far and the keys containment
/// refused.
struct Walk<'a> {
    source: &'a Utf8Path,
    tree: BTreeMap<Utf8PathBuf, Entry>,
    refused: BTreeSet<Utf8PathBuf>,
    /// One budget across the whole walk: everything it reads is held in the
    /// tree at once.
    budget: &'a Budget,
}

impl Walk<'_> {
    /// Walks every entry of `dir` — the source subdirectory at `prefix`,
    /// `depth` levels below the source root — into the tree.
    fn descend(&mut self, dir: &Dir, prefix: &Utf8Path, depth: usize) -> Result<()> {
        if depth > MAX_WALK_DEPTH {
            return Err(Error::TreeTooDeep {
                path: self.absolute(prefix),
                limit: MAX_WALK_DEPTH,
            });
        }
        let entries = dir.entries().map_err(io_at(self.absolute(prefix)))?;
        for entry in entries {
            let entry = entry.map_err(io_at(self.absolute(prefix)))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|raw| Error::TreeNameNotUtf8 {
                    path: self.absolute(prefix),
                    name: raw.to_string_lossy().into_owned(),
                })?;
            let rel = prefix.join(&name);
            let Some(key) = self.admit(&rel)? else {
                continue;
            };
            // `metadata` and not `file_type`: cap-std reads the latter from
            // the directory stream, and where the filesystem leaves that
            // field unset — XFS without `ftype`, some network filesystems —
            // it answers "unknown" rather than falling back to a stat.
            let meta = entry.metadata().map_err(io_at(self.absolute(&rel)))?;
            let file_type = meta.file_type();
            let node = if file_type.is_symlink() {
                let target = dir
                    .read_link_contents(&name)
                    .map_err(io_at(self.absolute(&rel)))?;
                let target = target.into_os_string().into_string().map_err(|raw| {
                    Error::TreeTargetNotUtf8 {
                        path: self.absolute(&rel),
                        target: raw.to_string_lossy().into_owned(),
                    }
                })?;
                Entry::Symlink { target }
            } else if file_type.is_dir() {
                let sub = open_dir_nofollow(dir, &name).map_err(io_at(self.absolute(&rel)))?;
                self.descend(&sub, &rel, depth + 1)?;
                continue;
            } else if file_type.is_file() {
                let mut file =
                    open_file_nofollow(dir, &name).map_err(io_at(self.absolute(&rel)))?;
                let meta = file.metadata().map_err(io_at(self.absolute(&rel)))?;
                if !meta.is_file() {
                    return Err(Error::TreeNodeKind {
                        path: self.absolute(&rel),
                    });
                }
                let executable = is_executable(&meta);
                let contents = self
                    .budget
                    .read_to_end(&mut file)
                    .map_err(io_at(self.absolute(&rel)))?
                    .ok_or_else(|| Error::SourceTooLarge {
                        path: self.absolute(&rel),
                        limit: self.budget.limit(),
                    })?;
                Entry::File {
                    contents,
                    executable,
                }
            } else {
                return Err(Error::TreeNodeKind {
                    path: self.absolute(&rel),
                });
            };
            self.retain(&rel, key, node)?;
        }
        Ok(())
    }

    /// Enters one walked node in the tree, spending what holding it costs
    /// beyond the file bytes already spent — the key, a symlink's target.
    /// Nothing else bounds how many entries a walk may retain: a directory
    /// of a million empty files costs no file bytes at all.
    fn retain(&mut self, rel: &Utf8Path, key: Utf8PathBuf, node: Entry) -> Result<()> {
        let held = key.as_str().len()
            + match &node {
                Entry::File { .. } => 0,
                Entry::Symlink { target } => target.len(),
                Entry::Block { body, marker, .. } => body.len() + marker.len(),
            };
        self.spend(rel, held)?;
        self.tree.insert(key, node);
        Ok(())
    }

    /// Normalizes one walked path, recording it among the refused and
    /// answering `None` where containment declines it. A refused name is
    /// held to the end of the walk like an admitted one, so it spends the
    /// budget like one.
    fn admit(&mut self, rel: &Utf8Path) -> Result<Option<Utf8PathBuf>> {
        match crate::containment::contained_normalize(rel) {
            Some(normalized) => Ok(Some(normalized)),
            None => {
                self.spend(rel, rel.as_str().len())?;
                self.refused.insert(rel.to_owned());
                Ok(None)
            }
        }
    }

    /// Spends `held` bytes of the walk's one budget, naming `rel` where that
    /// is what runs it out.
    fn spend(&self, rel: &Utf8Path, held: usize) -> Result<()> {
        if self.budget.spend(held as u64) {
            Ok(())
        } else {
            Err(Error::SourceTooLarge {
                path: self.absolute(rel),
                limit: self.budget.limit(),
            })
        }
    }

    /// Where `rel` sits on the invoker's filesystem; the empty `rel` is the
    /// source itself, without a trailing separator.
    fn absolute(&self, rel: &Utf8Path) -> Utf8PathBuf {
        if rel.as_str().is_empty() {
            self.source.to_owned()
        } else {
            self.source.join(rel)
        }
    }
}

/// Opens the directory `name` inside `dir` without following a final
/// symlink.
fn open_dir_nofollow(dir: &Dir, name: &str) -> std::io::Result<Dir> {
    let start = std::fs::File::from(dir.as_fd().try_clone_to_owned()?);
    let opened = cap_primitives::fs::open_dir_nofollow(&start, Path::new(name))?;
    Ok(Dir::from_std_file(opened))
}

/// Opens the file `name` inside `dir` for reading with `O_NOFOLLOW` and
/// `O_NONBLOCK`; the caller reads the kind back off the returned handle.
pub(crate) fn open_file_nofollow(dir: &Dir, name: &str) -> std::io::Result<std::fs::File> {
    let start = std::fs::File::from(dir.as_fd().try_clone_to_owned()?);
    let mut options = OpenOptions::new();
    options.read(true);
    options._cap_fs_ext_follow(FollowSymlinks::No);
    options._cap_fs_ext_nonblock(true);
    cap_primitives::fs::open(&start, Path::new(name), &options)
}

/// Wraps an OS error as [`Error::Io`] at an absolute source-tree path.
fn io_at(path: Utf8PathBuf) -> impl FnOnce(std::io::Error) -> Error {
    move |source| Error::Io {
        role: IoRole::SourceTree,
        path,
        source,
    }
}

/// Whether the source file's owner-executable bit is set.
pub(crate) fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o100 != 0
}

#[cfg(test)]
#[path = "tree_tests.rs"]
mod tests;
