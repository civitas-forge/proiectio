use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::os::fd::AsFd;
use std::path::Path;

use camino::{Utf8Path, Utf8PathBuf};
use cap_primitives::fs::{FollowSymlinks, OpenOptions};
use cap_std::ambient_authority;
use cap_std::fs::Dir;

use crate::{Entry, Error, Result};

/// Walks a source directory into the desired tree [`decide`](crate::decide)
/// takes — the second desired-tree source beside
/// [`load_mapping`](crate::load_mapping) (`docs/cli-tour.lex` section 1,
/// <https://github.com/civitas-forge/proiectio/blob/main/docs/cli-tour.lex>:
/// a directory tree, verbatim, metadata copied from the source).
///
/// Every regular file under `source` becomes an [`Entry::File`] carrying its
/// bytes and its owner-executable bit; every symlink becomes an
/// [`Entry::Symlink`] carrying the target string verbatim. Keys are paths
/// relative to `source`, so the destination reproduces the source's layout —
/// which is why a relative in-tree target keeps working once projected: the
/// link and what it points at move together. Targets are neither graded nor
/// rewritten here. [`decide`](crate::decide) grades every desired link
/// (`docs/security.lex` section 3), and a target landing outside the
/// destination — an absolute one, or one climbing out of the tree — is
/// refused there unless the caller passes
/// [`ExternalTargetPolicy::Allow`](crate::ExternalTargetPolicy::Allow).
///
/// Directories carry no entry of their own: [`Entry`] has no directory
/// variant, and a desired tree implies its directories from its files'
/// parent components. A directory the walk finds empty therefore projects
/// nothing, and no such directory appears at the destination — a source tree
/// whose only content is an empty directory produces an empty desired tree.
///
/// An archive met *inside* the tree is a file like any other and is copied
/// byte-for-byte. Extraction happens only where it is asked for
/// (`docs/cli-tour.lex` section 5).
///
/// A desired tree carries every file's bytes, so loading one costs the
/// source's total size in memory — the shape [`Entry`] takes, and what a
/// caller computing a tree of its own already pays.
///
/// # Trust
///
/// The trust split of `docs/security.lex` section 1,
/// <https://github.com/civitas-forge/proiectio/blob/main/docs/security.lex>:
/// `source` is the invoker's and is trusted — it may point anywhere the
/// invoker can read and passes no containment check — while everything
/// under it is content the invoker did not necessarily author.
///
/// So the walk never dereferences what it did not choose to descend. Every
/// entry is judged by its own `lstat`: a symlink is read as a pointer and
/// carried as one, never opened, so a link inside the source pointing at
/// `/etc` reaches the desired tree as a link and never as copied content.
/// Only real directories are descended. `source` is opened once with
/// ambient authority and every read below rides that capability handle, so
/// nothing outside the source tree is ever opened.
///
/// An entry's `lstat` and its open are two lookups of one name, and a
/// source tree somebody else can write may change that name in between. So
/// neither open follows a final symlink: a name that has become a link is
/// refused at open rather than descended or read through, which is what
/// keeps a link aimed at an ancestor from recursing without end. The file
/// open also waits for nothing (`O_NONBLOCK`), so a name that has become a
/// FIFO cannot park the walk until a writer appears, and the kind is read
/// back off the opened handle — a name that changed kind under the walk
/// fails the load instead of being read as whatever it now is.
///
/// # Refusals and errors
///
/// A key the containment gateway refuses — [`contained_join`]'s lexical
/// contract, applied to every walked path exactly as it is applied to every
/// mapping key — is aggregated, and the whole walk reports its offenders in
/// one [`Error::Containment`] naming each key verbatim, relative to
/// `source`. A filesystem name is never `.`, `..`, empty, or absolute, so
/// what a walked key can offend on is the rest of the contract: a backslash
/// in a name, a colon, a trailing dot or space, and the Windows reserved
/// device names — all ordinary names on Unix, none of them a path the
/// projection may create. Nothing is read for a refused key, and a refused
/// *directory* is refused whole: every key under it would carry the refused
/// name as a component, so the walk names the directory and never opens it.
/// That holds however little it contains — a directory the gateway refuses
/// fails the load even when it is empty, where an empty directory bearing
/// an ordinary name simply projects nothing.
///
/// The two collision refusals a desired tree can otherwise carry cannot
/// arise here. Two walked keys never name one location: names within a
/// directory are distinct and each directory contributes its own prefix.
/// And no key lies beneath another: every key's ancestors are the
/// directories that produced it, and a directory carries no entry to be
/// nested under.
///
/// The rest are errors, not refusals — a source tree carrying something the
/// projection cannot express fails the load rather than declining a
/// destination path:
///
/// - a name that is not UTF-8 — [`Error::TreeNameNotUtf8`]. Observation
///   *skips* such a name, because it can never match a desired or recorded
///   path ([`observe`](crate::observe)); here it is content the caller asked
///   to project, and skipping it would drop that content silently;
/// - a symlink whose target is not UTF-8 — [`Error::TreeTargetNotUtf8`]:
///   [`Entry::Symlink`] carries a `String`, so such a pointer has no
///   representation in a desired tree;
/// - a node of a kind the projection never writes — a FIFO, a socket, or a
///   device node — [`Error::TreeNodeKind`] naming it. It is never opened:
///   reading a FIFO with no writer blocks forever, which is why observation
///   records such nodes without opening them either. The same error names a
///   file whose kind changed between its `lstat` and its open;
/// - anything the filesystem refuses — [`Error::Io`], carrying the absolute
///   path of what could not be read.
///
/// [`contained_join`]: crate::contained_join
///
/// # Panics
///
/// Panics if `source` is relative: the crate never consults the current
/// directory, so a relative path here has no meaning it could honor.
pub fn load_tree(source: &Utf8Path) -> Result<BTreeMap<Utf8PathBuf, Entry>> {
    assert!(
        source.is_absolute(),
        "tree source path must be absolute, got {source}"
    );
    let root = Dir::open_ambient_dir(source, ambient_authority()).map_err(|e| Error::Io {
        path: source.to_owned(),
        source: e,
    })?;
    let mut walk = Walk {
        source,
        tree: BTreeMap::new(),
        refused: BTreeSet::new(),
    };
    walk.descend(&root, Utf8Path::new(""))?;
    if !walk.refused.is_empty() {
        return Err(Error::Containment {
            paths: walk.refused,
        });
    }
    Ok(walk.tree)
}

/// One [`load_tree`] walk: the tree built so far and the keys containment
/// refused, which accumulate so a source tree is reported whole rather than
/// one offending name at a time.
struct Walk<'a> {
    /// The absolute source root, used to name paths in errors — the invoker
    /// chose a location that may sit anywhere, so a relative name would not
    /// say which tree failed.
    source: &'a Utf8Path,
    tree: BTreeMap<Utf8PathBuf, Entry>,
    refused: BTreeSet<Utf8PathBuf>,
}

impl Walk<'_> {
    /// Walks every entry of `dir` — the source subdirectory at `prefix` —
    /// into the tree, recursing into real subdirectories through handles
    /// opened from `dir`, so every open stays anchored to the source handle.
    fn descend(&mut self, dir: &Dir, prefix: &Utf8Path) -> Result<()> {
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
            // `metadata` and not `file_type`: cap-std reads the latter from
            // the directory stream, and where the filesystem leaves that
            // field unset — XFS without `ftype`, some network filesystems —
            // it answers "unknown" rather than falling back to a stat, which
            // this walk would have to read as a kind it refuses. `metadata`
            // is the `lstat` `observe` takes for the same reason, and a
            // symlink is described by itself, never by what it points at.
            let meta = entry.metadata().map_err(io_at(self.absolute(&rel)))?;
            let file_type = meta.file_type();
            let Some(key) = self.admit(&rel) else {
                // Refused by containment. A directory is refused here too,
                // and refusing it settles its whole subtree: every key below
                // carries the refused name as a component, so none of them
                // could be projected either. The directory is never opened
                // and nothing under it is read — the refusal names it rather
                // than the descendants it would have produced.
                continue;
            };
            let node = if file_type.is_symlink() {
                // The target string, verbatim — grading it needs the
                // destination and belongs to `decide`. `read_link_contents`
                // never resolves the name it is given, so a link is read as
                // a pointer whatever it points at.
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
                // A directory is a container, not an entry: the tree it
                // holds implies it. Judged after the symlink arm, as
                // `observe`'s walk judges it, so a link to a directory is
                // carried as a pointer without that resting on the `lstat`
                // above being a no-follow one.
                let sub = open_dir_nofollow(dir, &name).map_err(io_at(self.absolute(&rel)))?;
                self.descend(&sub, &rel)?;
                continue;
            } else if file_type.is_file() {
                // One handle for bytes and mode, so both describe the same
                // file even if the name is swapped mid-read, and the handle
                // itself says which kind was opened — the `lstat` above was a
                // separate lookup and by now may describe a name that has
                // been replaced.
                let mut file =
                    open_file_nofollow(dir, &name).map_err(io_at(self.absolute(&rel)))?;
                let meta = file.metadata().map_err(io_at(self.absolute(&rel)))?;
                if !meta.is_file() {
                    return Err(Error::TreeNodeKind {
                        path: self.absolute(&rel),
                    });
                }
                let executable = is_executable(&meta);
                let mut contents = Vec::new();
                file.read_to_end(&mut contents)
                    .map_err(io_at(self.absolute(&rel)))?;
                Entry::File {
                    contents,
                    executable,
                }
            } else {
                // A FIFO, socket, or device node: the projection writes no
                // such thing, and opening one to find out what it holds can
                // block forever.
                return Err(Error::TreeNodeKind {
                    path: self.absolute(&rel),
                });
            };
            self.tree.insert(key, node);
        }
        Ok(())
    }

    /// Runs one walked path through the containment gateway's normalize-only
    /// half; `None` is a refusal, recorded for the aggregated
    /// [`Error::Containment`] and named verbatim there.
    fn admit(&mut self, rel: &Utf8Path) -> Option<Utf8PathBuf> {
        match crate::containment::contained_normalize(rel) {
            Ok(normalized) => Some(normalized),
            Err(_) => {
                self.refused.insert(rel.to_owned());
                None
            }
        }
    }

    /// Where `rel` sits on the invoker's filesystem. Errors name absolute
    /// paths because a source tree may live anywhere the invoker can read,
    /// and a path relative to a root the message does not carry would not
    /// locate the offending node. The empty `rel` — the walk's own root —
    /// is the source itself, spelled without the trailing separator a join
    /// would leave.
    fn absolute(&self, rel: &Utf8Path) -> Utf8PathBuf {
        if rel.as_str().is_empty() {
            self.source.to_owned()
        } else {
            self.source.join(rel)
        }
    }
}

/// Opens the directory `name` inside `dir` without following a final
/// symlink, through cap-primitives' `open_dir_nofollow` — the same door
/// apply's no-follow walk opens its directories with, public there and not
/// on `Dir` itself.
///
/// The `lstat` that said "directory" and this open are two lookups of one
/// name, and refusing the follow is what keeps the gap between them closed:
/// a link swapped in meanwhile is refused at open rather than descended.
/// Following it would walk a subtree the caller never named, and following
/// one aimed at an ancestor would recurse until the stack ran out.
fn open_dir_nofollow(dir: &Dir, name: &str) -> std::io::Result<Dir> {
    let start = std::fs::File::from(dir.as_fd().try_clone_to_owned()?);
    let opened = cap_primitives::fs::open_dir_nofollow(&start, Path::new(name))?;
    Ok(Dir::from_std_file(opened))
}

/// Opens the regular file `name` inside `dir` for reading, following no
/// final symlink and waiting for nothing.
///
/// Same two-lookup gap as [`open_dir_nofollow`], and the same two
/// substitutions to refuse: a symlink swapped in is refused at open
/// (`O_NOFOLLOW`) instead of read through, and a FIFO swapped in opens
/// (`O_NONBLOCK`) instead of parking the walk until some writer appears. The
/// caller reads the kind back off the returned handle, so what it reads is
/// what it opened.
///
/// The two `_cap_fs_ext_` methods are cap-primitives' public spelling of
/// both flags; the `cap_fs_ext` traits that wrap them live in a crate this
/// one does not depend on.
fn open_file_nofollow(dir: &Dir, name: &str) -> std::io::Result<std::fs::File> {
    let start = std::fs::File::from(dir.as_fd().try_clone_to_owned()?);
    let mut options = OpenOptions::new();
    options.read(true);
    options._cap_fs_ext_follow(FollowSymlinks::No);
    options._cap_fs_ext_nonblock(true);
    cap_primitives::fs::open(&start, Path::new(name), &options)
}

/// Wraps an OS error as [`Error::Io`] at an absolute source-tree path.
fn io_at(path: Utf8PathBuf) -> impl FnOnce(std::io::Error) -> Error {
    move |source| Error::Io { path, source }
}

/// Whether the source file's owner-executable bit is set — the one piece of
/// metadata a projected file carries (`docs/cli-tour.lex` section 1). Read
/// from the opened handle, as `load_mapping` reads a `source` entry's.
fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o100 != 0
}

#[cfg(test)]
#[path = "tree_tests.rs"]
mod tests;
