//! Test scaffolding: declarative trees, `TempDir` fixtures, and directory
//! assertions (`docs/implementation.lex` section 2).
//!
//! A [`Tree`] declares paths, contents, links, and exec bits in one chained
//! expression. The same value serves both sides of a scenario: [`Tree::entries`]
//! produces the desired-tree map `plan` takes, and [`Tree::materialize`] writes
//! the nodes into a fresh [`tempfile::TempDir`], returned as a [`Fixture`].
//! The one exception is `Entry::Block`, which has no [`Node`] counterpart: a
//! block scenario declares its container as an ordinary file and injects the
//! block entry into the map by hand.
//! [`assert_tree`] diffs a directory against an expected tree — contents, exec
//! bit, link targets — and panics with every divergence listed.
//!
//! # The rule of the house
//!
//! Tests never touch the process's current directory. `cargo test` runs tests
//! in parallel threads and `std::env::set_current_dir` is process-global: one
//! test chdir-ing reroutes every relative path in every concurrently running
//! test. So everything here hands out **absolute** paths — [`Fixture::root`]
//! and [`Fixture::path`] — and every helper asserts it was given one.
//! `set_current_dir` is banned crate-wide via `clippy.toml`
//! (`disallowed-methods`), which the CI clippy step turns into a build failure.
//!
//! Teardown is RAII: dropping the [`Fixture`] drops the underlying `TempDir`,
//! which deletes the directory. No shared state, no ordering, no cleanup code
//! in tests.
//!
//! The scaffolding defends its own isolation promise: tree paths admit only
//! normal segments (no `..`, no `.`, no absolute paths, no empty segments),
//! and no node may sit under a declared file or symlink — so a declared tree
//! cannot reach outside the fixture directory, by dot-dot or by writing
//! through a declared link. [`Tree::write_under`] extends the same promise to
//! overlays on an existing root: it refuses to write through an on-disk
//! symlink ancestor, and it unlinks (never follows) an existing leaf whose
//! kind differs from the declared node. This module is Unix-only
//! (`cfg(unix)` at the declaration site): exec bits and symlinks are the
//! behavior under test.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;

use camino::{Utf8Path, Utf8PathBuf};

use crate::Entry;

/// One node of a declared tree.
///
/// Distinct from [`Entry`]: a fixture on disk needs plain directories and
/// carries no `Block` variant — a delimited region only means something once
/// apply defines the marker format, so scenario container files are declared
/// as ordinary [`Node::File`]s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Node {
    /// A regular file.
    File {
        /// The exact bytes.
        contents: Vec<u8>,
        /// Whether the owner-executable bit is set.
        executable: bool,
    },
    /// A symbolic link whose target string is written verbatim.
    Symlink {
        /// The target string, never resolved by the scaffolding.
        target: String,
    },
    /// A directory declared explicitly — needed only for *empty* directories;
    /// parents of other nodes are implied.
    Dir,
}

/// A declarative tree: relative paths mapped to [`Node`]s, built in one
/// chained expression.
///
/// ```ignore
/// let tree = Tree::new()
///     .file("notes/a.txt", "alpha")
///     .executable("bin/run", "#!/bin/sh\n")
///     .symlink("latest", "notes/a.txt")
///     .dir("empty");
/// let desired = tree.entries();      // BTreeMap<Utf8PathBuf, Entry>
/// let fixture = tree.materialize();  // the same shape on disk, RAII-cleaned
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Tree {
    nodes: BTreeMap<Utf8PathBuf, Node>,
}

impl Tree {
    /// An empty tree.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Adds a regular, non-executable file.
    pub(crate) fn file(self, path: impl AsRef<str>, contents: impl Into<Vec<u8>>) -> Self {
        self.insert(
            path,
            Node::File {
                contents: contents.into(),
                executable: false,
            },
        )
    }

    /// Adds a regular file with the executable bit set.
    pub(crate) fn executable(self, path: impl AsRef<str>, contents: impl Into<Vec<u8>>) -> Self {
        self.insert(
            path,
            Node::File {
                contents: contents.into(),
                executable: true,
            },
        )
    }

    /// Adds a symlink; `target` reaches disk verbatim.
    pub(crate) fn symlink(self, path: impl AsRef<str>, target: impl Into<String>) -> Self {
        self.insert(
            path,
            Node::Symlink {
                target: target.into(),
            },
        )
    }

    /// Declares a directory explicitly. Only empty directories need this;
    /// parents of files and links are created implicitly.
    pub(crate) fn dir(self, path: impl AsRef<str>) -> Self {
        self.insert(path, Node::Dir)
    }

    fn insert(mut self, path: impl AsRef<str>, node: Node) -> Self {
        let path = Utf8PathBuf::from(path.as_ref());
        assert_normal_relative(&path);
        for ancestor in path.ancestors().skip(1) {
            if ancestor.as_str().is_empty() {
                continue;
            }
            if let Some(existing) = self.nodes.get(ancestor) {
                assert!(
                    matches!(existing, Node::Dir),
                    "{path:?} nests under {ancestor:?}, which is a {} — \
                     nodes may only sit under directories",
                    kind_of(existing)
                );
            }
        }
        if !matches!(node, Node::Dir) {
            if let Some(descendant) = self
                .nodes
                .keys()
                .find(|k| k.as_path() != path && k.starts_with(&path))
            {
                panic!(
                    "{descendant:?} nests under {path:?}, which is a {} — \
                     nodes may only sit under directories",
                    kind_of(&node)
                );
            }
        }
        let previous = self.nodes.insert(path.clone(), node);
        assert!(previous.is_none(), "duplicate tree path {path:?}");
        self
    }

    /// The desired-tree value `plan` takes.
    ///
    /// [`Node::Dir`] entries are omitted: [`Entry`] has no directory variant —
    /// a desired tree implies its directories from the parent components of
    /// its files and links.
    pub(crate) fn entries(&self) -> BTreeMap<Utf8PathBuf, Entry> {
        self.nodes
            .iter()
            .filter_map(|(path, node)| {
                let entry = match node {
                    Node::File {
                        contents,
                        executable,
                    } => Entry::File {
                        contents: contents.clone(),
                        executable: *executable,
                    },
                    Node::Symlink { target } => Entry::Symlink {
                        target: target.clone(),
                    },
                    Node::Dir => return None,
                };
                Some((path.clone(), entry))
            })
            .collect()
    }

    /// Writes the tree into a fresh temporary directory and returns the
    /// [`Fixture`] owning it. The fixture's root is absolute and
    /// canonicalized (on macOS the temp dir sits behind the `/var` symlink).
    // The canonicalize ban guards tree containment, which is lexical; this
    // call resolves the fixture's own trusted temp root (on macOS it sits
    // behind the `/var` symlink), not an untrusted tree path.
    #[allow(clippy::disallowed_methods)]
    pub(crate) fn materialize(&self) -> Fixture {
        let temp = tempfile::TempDir::new().expect("create TempDir");
        let root =
            Utf8PathBuf::from_path_buf(temp.path().canonicalize().expect("canonicalize temp root"))
                .expect("temp root is UTF-8");
        self.write_under(&root);
        Fixture { _temp: temp, root }
    }

    /// Writes the tree's nodes under an existing absolute `root`, in sorted
    /// path order (parents before children), creating implied parent
    /// directories.
    ///
    /// Overlaying on a non-empty root stays inside it: a write whose on-disk
    /// ancestor is a symlink panics instead of following it, and an existing
    /// leaf is unlinked (never followed, never errored on) when its kind
    /// differs from the declared node — so a declared file replaces a stale
    /// symlink rather than writing through it.
    pub(crate) fn write_under(&self, root: &Utf8Path) {
        assert!(root.is_absolute(), "write_under takes an absolute root");
        for (path, node) in &self.nodes {
            let abs = root.join(path);
            let parent = abs.parent().expect("joined path has a parent");
            assert_no_symlink_ancestor(root, parent);
            fs::create_dir_all(parent).unwrap_or_else(|e| panic!("create {parent:?}: {e}"));
            unlink_mismatched_leaf(&abs, node);
            match node {
                Node::File {
                    contents,
                    executable,
                } => {
                    fs::write(&abs, contents).unwrap_or_else(|e| panic!("write {abs:?}: {e}"));
                    // Set the mode unconditionally: `fs::write` over an
                    // existing file keeps its permissions, so a declared
                    // non-executable must also *clear* a stale exec bit.
                    let mode = if *executable { 0o755 } else { 0o644 };
                    fs::set_permissions(&abs, fs::Permissions::from_mode(mode))
                        .unwrap_or_else(|e| panic!("chmod {abs:?}: {e}"));
                }
                Node::Symlink { target } => {
                    std::os::unix::fs::symlink(target, &abs)
                        .unwrap_or_else(|e| panic!("symlink {abs:?} -> {target:?}: {e}"));
                }
                Node::Dir => {
                    fs::create_dir_all(&abs).unwrap_or_else(|e| panic!("create {abs:?}: {e}"));
                }
            }
        }
    }
}

/// A materialized tree in a temporary directory.
///
/// Owns the `TempDir`: dropping the fixture deletes the directory (RAII
/// teardown). All paths it hands out are absolute.
#[derive(Debug)]
pub(crate) struct Fixture {
    /// Held only for its `Drop`; the directory lives as long as the fixture.
    _temp: tempfile::TempDir,
    root: Utf8PathBuf,
}

impl Fixture {
    /// The absolute, canonicalized root of the fixture directory.
    pub(crate) fn root(&self) -> &Utf8Path {
        &self.root
    }

    /// The absolute path of `rel` inside the fixture.
    pub(crate) fn path(&self, rel: impl AsRef<Utf8Path>) -> Utf8PathBuf {
        let rel = rel.as_ref();
        assert_normal_relative(rel);
        self.root.join(rel)
    }
}

/// Asserts `path` is relative and made only of normal segments — no `..`,
/// no `.`, no leading `/`, no empty segments (so no `a//b` and no trailing
/// slash) — so joining it under a fixture root can never resolve outside the
/// root. Checks the raw string, because `Utf8Path::components()` silently
/// normalizes `.` and empty segments away.
fn assert_normal_relative(path: &Utf8Path) {
    let raw = path.as_str();
    assert!(
        !raw.is_empty()
            && !raw.starts_with('/')
            && raw
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != ".."),
        "tree paths are non-empty, relative, and free of `.`/`..`/empty segments, got {path:?}"
    );
}

/// Panics if any on-disk component strictly below `root`, down to `parent`,
/// exists and is a symlink: `create_dir_all` and `fs::write` would follow it,
/// and the write could land outside the fixture.
fn assert_no_symlink_ancestor(root: &Utf8Path, parent: &Utf8Path) {
    let rel = parent
        .strip_prefix(root)
        .expect("write_under paths stay under the root");
    let mut cur = root.to_owned();
    for component in rel.components() {
        cur.push(component);
        if let Ok(meta) = fs::symlink_metadata(&cur) {
            assert!(
                !meta.file_type().is_symlink(),
                "on-disk ancestor {cur:?} is a symlink — refusing to write through it"
            );
        }
    }
}

/// Unlinks an existing path at `abs` before `node` is written, so the write
/// lands on a fresh inode and never reaches anything else: a declared file
/// over an existing symlink must replace the link, not write through it; over
/// an existing regular file it must not truncate in place, because a
/// hard-linked inode is shared and truncation would mutate every other name
/// for it; and a declared symlink or directory over a different kind would
/// otherwise fail with `EEXIST`/`ENOTDIR`. Only a directory over a directory
/// is left in place. Never follows the existing path.
fn unlink_mismatched_leaf(abs: &Utf8Path, node: &Node) {
    let Ok(meta) = fs::symlink_metadata(abs) else {
        return;
    };
    let file_type = meta.file_type();
    let overwritable_in_place = match node {
        // `fs::write` would truncate the existing inode; unlink so a
        // hard-linked sibling keeps its bytes.
        Node::File { .. } => false,
        // `symlink(2)` refuses any existing path, same-kind included.
        Node::Symlink { .. } => false,
        Node::Dir => file_type.is_dir(),
    };
    if overwritable_in_place {
        return;
    }
    if file_type.is_dir() {
        fs::remove_dir_all(abs).unwrap_or_else(|e| panic!("remove {abs:?}: {e}"));
    } else {
        fs::remove_file(abs).unwrap_or_else(|e| panic!("remove {abs:?}: {e}"));
    }
}

/// Diffs the directory at `root` against `expected`, returning one line per
/// divergence: paths missing from disk, paths on disk the tree does not
/// declare, and per-path mismatches of kind, contents, exec bit, or link
/// target. Empty means the directory matches.
///
/// Ancestor directories of declared nodes are implied and never reported.
pub(crate) fn tree_diff(root: &Utf8Path, expected: &Tree) -> Vec<String> {
    assert!(root.is_absolute(), "tree_diff takes an absolute root");

    let mut want = expected.nodes.clone();
    for path in expected.nodes.keys() {
        for ancestor in path.ancestors().skip(1) {
            if !ancestor.as_str().is_empty() {
                want.entry(ancestor.to_owned()).or_insert(Node::Dir);
            }
        }
    }

    let mut got = BTreeMap::new();
    observe(root, root, &mut got);

    let mut diff = Vec::new();
    for (path, want_node) in &want {
        match got.get(path) {
            None => diff.push(format!("missing: {path} (expected {})", kind_of(want_node))),
            Some(got_node) => describe_mismatch(path, want_node, got_node, &mut diff),
        }
    }
    for (path, got_node) in &got {
        if !want.contains_key(path) {
            diff.push(format!("unexpected: {path} ({})", kind_of(got_node)));
        }
    }
    diff
}

/// Asserts the directory at `root` matches `expected`, panicking with every
/// divergence [`tree_diff`] found.
pub(crate) fn assert_tree(root: &Utf8Path, expected: &Tree) {
    let diff = tree_diff(root, expected);
    assert!(
        diff.is_empty(),
        "directory {root} diverges from the expected tree:\n  {}",
        diff.join("\n  ")
    );
}

fn observe(root: &Utf8Path, dir: &Utf8Path, into: &mut BTreeMap<Utf8PathBuf, Node>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {dir:?}: {e}"));
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("read_dir {dir:?}: {e}"));
        let abs = Utf8PathBuf::from_path_buf(entry.path()).expect("fixture paths are UTF-8");
        let rel = abs
            .strip_prefix(root)
            .expect("walked path is under the root")
            .to_owned();
        let meta = fs::symlink_metadata(&abs).unwrap_or_else(|e| panic!("stat {abs:?}: {e}"));
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&abs).unwrap_or_else(|e| panic!("read_link {abs:?}: {e}"));
            let target = Utf8PathBuf::from_path_buf(target).expect("link targets are UTF-8");
            into.insert(
                rel,
                Node::Symlink {
                    target: target.into_string(),
                },
            );
        } else if meta.is_dir() {
            into.insert(rel, Node::Dir);
            observe(root, &abs, into);
        } else if meta.is_file() {
            let contents = fs::read(&abs).unwrap_or_else(|e| panic!("read {abs:?}: {e}"));
            into.insert(
                rel,
                Node::File {
                    contents,
                    executable: meta.permissions().mode() & 0o100 != 0,
                },
            );
        } else {
            // A FIFO or device node: `fs::read` on a writerless FIFO blocks
            // forever, so report the kind instead of opening it.
            panic!(
                "unsupported node kind at {abs:?}: {:?} — fixtures hold only \
                 files, directories, and symlinks",
                meta.file_type()
            );
        }
    }
}

fn describe_mismatch(path: &Utf8Path, want: &Node, got: &Node, diff: &mut Vec<String>) {
    match (want, got) {
        (
            Node::File {
                contents: want_contents,
                executable: want_exec,
            },
            Node::File {
                contents: got_contents,
                executable: got_exec,
            },
        ) => {
            if want_contents != got_contents {
                diff.push(format!(
                    "contents differ: {path} (expected {:?}, found {:?})",
                    String::from_utf8_lossy(want_contents),
                    String::from_utf8_lossy(got_contents),
                ));
            }
            if want_exec != got_exec {
                diff.push(format!(
                    "exec bit differs: {path} (expected {want_exec}, found {got_exec})"
                ));
            }
        }
        (
            Node::Symlink {
                target: want_target,
            },
            Node::Symlink { target: got_target },
        ) => {
            if want_target != got_target {
                diff.push(format!(
                    "link target differs: {path} (expected {want_target:?}, found {got_target:?})"
                ));
            }
        }
        (Node::Dir, Node::Dir) => {}
        (want, got) => diff.push(format!(
            "kind differs: {path} (expected {}, found {})",
            kind_of(want),
            kind_of(got)
        )),
    }
}

fn kind_of(node: &Node) -> &'static str {
    match node {
        Node::File { .. } => "file",
        Node::Symlink { .. } => "symlink",
        Node::Dir => "directory",
    }
}

#[path = "test_support_tests.rs"]
mod tests;
