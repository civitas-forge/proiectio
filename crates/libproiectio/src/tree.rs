use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, Metadata, MetadataExt};

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
/// a name swapped for a symlink in the gap between an entry's `lstat` and
/// its open cannot redirect the read out of the source tree — cap-std
/// refuses the escape at open time. A swap to a link pointing back *inside*
/// the tree still resolves, which reads content the caller already asked to
/// project.
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
/// projection may create. Nothing is read for a refused key.
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
///   records such nodes without opening them either;
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
            let meta = entry.metadata().map_err(io_at(self.absolute(&rel)))?;
            let file_type = meta.file_type();
            if file_type.is_dir() {
                // A directory is a container, not an entry: the tree it
                // holds implies it. The handle comes from this directory's
                // own entry, so the descent never re-resolves the name.
                let sub = entry.open_dir().map_err(io_at(self.absolute(&rel)))?;
                self.descend(&sub, &rel)?;
                continue;
            }
            let Some(key) = self.admit(&rel) else {
                // Refused by containment: read nothing for it.
                continue;
            };
            let node = if file_type.is_symlink() {
                // The target string, verbatim — grading it needs the
                // destination and belongs to `decide`.
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
            } else if file_type.is_file() {
                // One handle for bytes and mode, so both describe the same
                // file even if the name is swapped mid-read.
                let mut file = entry.open().map_err(io_at(self.absolute(&rel)))?;
                let executable =
                    is_executable(&file.metadata().map_err(io_at(self.absolute(&rel)))?);
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

/// Wraps an OS error as [`Error::Io`] at an absolute source-tree path.
fn io_at(path: Utf8PathBuf) -> impl FnOnce(std::io::Error) -> Error {
    move |source| Error::Io { path, source }
}

/// Whether the source node's owner-executable bit is set — the one piece of
/// metadata a projected file carries (`docs/cli-tour.lex` section 1).
fn is_executable(meta: &Metadata) -> bool {
    meta.mode() & 0o100 != 0
}

#[cfg(test)]
#[path = "tree_tests.rs"]
mod tests;
