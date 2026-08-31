// Test scaffolding: declarative trees, `TempDir` fixtures, and directory
// assertions. Every path handed out is absolute and every helper asserts it
// was given one — tests never chdir, which `clippy.toml` bans crate-wide,
// since `set_current_dir` is process-global and `cargo test` runs in
// parallel threads. `Entry::Block` has no [`Node`] counterpart: a block
// scenario declares its container as an ordinary file and injects the block
// entry into the map by hand.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;

use camino::{Utf8Path, Utf8PathBuf};

use crate::{Entry, Origin, Refusal, Refused, StateDir};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Node {
    File { contents: Vec<u8>, executable: bool },
    // The target string, written verbatim and never resolved.
    Symlink { target: String },
    // Needed only for *empty* directories; parents of other nodes are
    // implied.
    Dir,
}

// A declarative tree: relative paths mapped to [`Node`]s, built in one
// chained expression.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Tree {
    nodes: BTreeMap<Utf8PathBuf, Node>,
}

impl Tree {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn file(self, path: impl AsRef<str>, contents: impl Into<Vec<u8>>) -> Self {
        self.insert(
            path,
            Node::File {
                contents: contents.into(),
                executable: false,
            },
        )
    }

    pub(crate) fn executable(self, path: impl AsRef<str>, contents: impl Into<Vec<u8>>) -> Self {
        self.insert(
            path,
            Node::File {
                contents: contents.into(),
                executable: true,
            },
        )
    }

    pub(crate) fn symlink(self, path: impl AsRef<str>, target: impl Into<String>) -> Self {
        self.insert(
            path,
            Node::Symlink {
                target: target.into(),
            },
        )
    }

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

    // The desired-tree value `plan` takes; [`Node::Dir`] entries are omitted.
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

    // Writes the tree into a fresh temporary directory. The fixture's root
    // is absolute and canonicalized.
    // The canonicalize ban guards tree containment; this call resolves the
    // fixture's own temp root (on macOS it sits behind the `/var` symlink).
    #[allow(clippy::disallowed_methods)]
    pub(crate) fn materialize(&self) -> Fixture {
        let temp = tempfile::TempDir::new().expect("create TempDir");
        let root =
            Utf8PathBuf::from_path_buf(temp.path().canonicalize().expect("canonicalize temp root"))
                .expect("temp root is UTF-8");
        self.write_under(&root);
        Fixture { _temp: temp, root }
    }

    // Writes the tree's nodes under an existing absolute `root`, creating
    // implied parent directories. A write whose on-disk ancestor is a
    // symlink panics instead of following it.
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
                    // `fs::write` over an existing file keeps its permissions,
                    // so set the mode unconditionally.
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

// A materialized tree in a temporary directory; dropping the fixture deletes
// the directory. All paths it hands out are absolute.
#[derive(Debug)]
pub(crate) struct Fixture {
    _temp: tempfile::TempDir,
    root: Utf8PathBuf,
}

impl Fixture {
    pub(crate) fn root(&self) -> &Utf8Path {
        &self.root
    }

    pub(crate) fn path(&self, rel: impl AsRef<Utf8Path>) -> Utf8PathBuf {
        let rel = rel.as_ref();
        assert_normal_relative(rel);
        self.root.join(rel)
    }
}

// The fixture root as the state directory the library reads and writes.
pub(crate) fn state_at(root: &Utf8Path) -> StateDir {
    assert!(root.is_absolute(), "state_at takes an absolute root");
    StateDir::open(root)
        .expect("the fixture root is there")
        .expect("open the fixture root as a state directory")
}

#[derive(Debug)]
pub(crate) struct MissingName {
    _temp: tempfile::TempDir,
    relative: Utf8PathBuf,
    absolute: Utf8PathBuf,
}

impl MissingName {
    // A random basename, held by a temporary directory elsewhere so no
    // current directory carries it, and the absolute path cwd resolves it to.
    pub(crate) fn with_suffix(suffix: &str) -> MissingName {
        let temp = tempfile::Builder::new()
            .prefix("proiectio-absent-")
            .suffix(suffix)
            .tempdir()
            .expect("a temporary directory");
        let relative = Utf8PathBuf::from(
            temp.path()
                .file_name()
                .and_then(|name| name.to_str())
                .expect("a UTF-8 temporary directory name"),
        );
        let cwd = std::env::current_dir().expect("a readable current directory");
        let absolute = Utf8PathBuf::from_path_buf(cwd)
            .expect("a UTF-8 current directory")
            .join(&relative);
        MissingName {
            _temp: temp,
            relative,
            absolute,
        }
    }

    pub(crate) fn relative(&self) -> &Utf8Path {
        &self.relative
    }

    pub(crate) fn absolute(&self) -> &Utf8Path {
        &self.absolute
    }
}

// Asserts `path` is relative and made only of normal segments. Checks the
// raw string: `Utf8Path::components()` normalizes `.` and empty segments away.
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

// Panics if any on-disk component strictly below `root`, down to `parent`,
// exists and is a symlink.
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

// Unlinks an existing path at `abs` before `node` is written, never
// following it. Only a directory over a directory is left in place.
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

// Diffs the directory at `root` against `expected`, one line per divergence;
// empty means it matches. Implied ancestor directories are never reported.
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
            // `fs::read` on a writerless FIFO blocks forever, so report the
            // kind instead of opening it.
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

// Writes a file at `path`, whose name is not UTF-8, and says whether it is
// there. Some hosts refuse such a name outright — APFS on macOS enforces
// UTF-8 — and a test that turns on one has nothing to do there. Linux takes
// it, and is where those tests do their work, so a failure to plant it there
// is a failure rather than a silent skip that would pass on nothing.
pub(crate) fn plant(path: &std::path::Path) -> bool {
    match fs::write(path, b"unnameable") {
        Ok(()) => true,
        Err(_) if !cfg!(target_os = "linux") => false,
        Err(e) => panic!("planting a name that is not UTF-8 at {path:?}: {e}"),
    }
}

pub(crate) fn paths_of(refused: &Refused) -> BTreeSet<Utf8PathBuf> {
    refused.paths().keys().cloned().collect()
}

pub(crate) fn origins_of(refused: &Refused) -> BTreeMap<Utf8PathBuf, Origin> {
    refused
        .paths()
        .iter()
        .map(|(path, refused)| (path.clone(), refused.origin.clone()))
        .collect()
}

pub(crate) fn refusals_of(refused: &Refused) -> BTreeMap<Utf8PathBuf, Refusal> {
    refused
        .paths()
        .iter()
        .map(|(path, refused)| (path.clone(), refused.refusal.clone()))
        .collect()
}

pub(crate) fn sourced_of(refused: &Refused) -> BTreeMap<Utf8PathBuf, (Refusal, Origin)> {
    refused
        .paths()
        .iter()
        .map(|(path, refused)| {
            (
                path.clone(),
                (refused.refusal.clone(), refused.origin.clone()),
            )
        })
        .collect()
}
