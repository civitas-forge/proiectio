use std::collections::BTreeMap;
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::fs_utf8::Dir;

use super::*;
use crate::test_support::{Tree, assert_tree};
use crate::{DriftPolicy, Entry, PlanOptions, apply, decide};

/// Opens a capability handle at a directory. Ambient authority is the
/// test's to spend; the library itself never opens ambient paths.
fn dir_at(root: &Utf8Path) -> Dir {
    Dir::open_ambient_dir(root, cap_std::ambient_authority()).expect("open fixture root as a Dir")
}

/// The state handle a caller ends up with for `root`: `None` where the
/// directory does not exist, which a caller spells to [`status`] as
/// [`StateDir::Missing`] — "nothing was ever projected here".
fn state_at(root: &Utf8Path) -> Option<Dir> {
    match Dir::open_ambient_dir(root, cap_std::ambient_authority()) {
        Ok(state) => Some(state),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => panic!("open {root}: {e}"),
    }
}

/// One full observe → decide → apply run, so a status test has something
/// recorded to classify.
fn project(dest: &Utf8Path, state: &Utf8Path, owner: &str, desired: &BTreeMap<Utf8PathBuf, Entry>) {
    let dest = dir_at(dest);
    let state = dir_at(state);
    let manifest = crate::load_manifest(&state).expect("load manifest");
    let observations = crate::observe(&dest, &manifest).expect("observe destination");
    let plan = decide(
        owner,
        desired,
        &manifest,
        &observations,
        None,
        PlanOptions {
            drift: DriftPolicy::Refuse,
            ..PlanOptions::default()
        },
    );
    apply(&dest, &state, &manifest, &plan).expect("apply");
}

/// [`status`] over two directories, the state directory outside the
/// destination.
fn status_of(dest: &Utf8Path, state: &Utf8Path) -> Status {
    let state = state_at(state);
    let state = match state.as_ref() {
        Some(dir) => StateDir::Outside(dir),
        None => StateDir::Missing,
    };
    status(&dir_at(dest), state).expect("status")
}

fn states(status: &Status) -> Vec<(&str, PathState)> {
    status
        .paths
        .iter()
        .map(|(path, state)| (path.as_str(), *state))
        .collect()
}

// Definition of done: a destination with no manifest and a missing state
// directory both report emptily rather than failing.

#[test]
fn a_destination_with_no_manifest_reports_nothing() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();

    // The state directory exists and holds no manifest file: nothing has
    // been projected yet.
    assert_eq!(fs::read_dir(state.root()).expect("read state").count(), 0);
    assert_eq!(status_of(dest.root(), state.root()), Status::default());
}

#[test]
fn a_missing_state_directory_reports_nothing() {
    let dest = Tree::new().materialize();
    let elsewhere = Tree::new().materialize();
    let missing = elsewhere.path("never-created");
    assert!(!missing.exists(), "the state directory must not exist");

    let state = state_at(&missing);
    assert!(state.is_none(), "a missing directory opens as no handle");

    let report = status(&dir_at(dest.root()), StateDir::Missing).expect("status");

    assert_eq!(report, Status::default());
}

#[test]
fn without_a_state_directory_everything_on_disk_is_foreign() {
    let dest = Tree::new().file("theirs.txt", "not ours").materialize();
    let elsewhere = Tree::new().materialize();

    assert!(state_at(&elsewhere.path("never-created")).is_none());
    let report = status(&dir_at(dest.root()), StateDir::Missing).expect("status");

    assert_eq!(states(&report), vec![("theirs.txt", PathState::Foreign)]);
}

#[test]
fn reports_one_state_per_path_of_the_union() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let tree = Tree::new()
        .file("clean.txt", "as written")
        .file("drifted.txt", "as written")
        .file("gone.txt", "as written");
    project(dest.root(), state.root(), "own", &tree.entries());

    fs::write(dest.path("drifted.txt"), "edited by hand").expect("edit");
    fs::remove_file(dest.path("gone.txt")).expect("delete");
    fs::write(dest.path("theirs.txt"), "never ours").expect("plant a foreign file");

    assert_eq!(
        states(&status_of(dest.root(), state.root())),
        vec![
            ("clean.txt", PathState::Clean),
            ("drifted.txt", PathState::Drifted),
            ("gone.txt", PathState::Missing),
            ("theirs.txt", PathState::Foreign),
        ]
    );
}

#[test]
fn a_directory_the_projection_created_still_reports_foreign() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let tree = Tree::new().file("a/b/mine.txt", "projected");
    project(dest.root(), state.root(), "own", &tree.entries());

    // The manifest records no directories, so the parents apply created
    // for an owned file are unrecorded like any other directory.
    assert_eq!(
        states(&status_of(dest.root(), state.root())),
        vec![
            ("a", PathState::Foreign),
            ("a/b", PathState::Foreign),
            ("a/b/mine.txt", PathState::Clean),
        ]
    );
}

#[test]
fn status_writes_nothing() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let tree = Tree::new().file("a/b.txt", "alpha");
    project(dest.root(), state.root(), "own", &tree.entries());
    let manifest = fs::read(state.path(crate::MANIFEST_FILE_NAME)).expect("read manifest");

    status_of(dest.root(), state.root());

    assert_tree(dest.root(), &tree);
    assert_tree(
        state.root(),
        &Tree::new().file(crate::MANIFEST_FILE_NAME, manifest),
    );
}

#[test]
fn the_state_subtree_inside_the_destination_never_classifies() {
    let dest = Tree::new().materialize();
    let prefix = Utf8Path::new(".proiectio");
    let state = dest.path(prefix);
    fs::create_dir(&state).expect("create the in-dest state directory");
    let tree = Tree::new().file("a.txt", "alpha");
    project(dest.root(), &state, "own", &tree.entries());

    let opened = state_at(&state).expect("the in-dest state directory opens");
    let report = status(
        &dir_at(dest.root()),
        StateDir::Inside {
            dir: &opened,
            prefix,
        },
    )
    .expect("status");

    assert_eq!(states(&report), vec![("a.txt", PathState::Clean)]);
}
