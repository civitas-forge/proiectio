use std::collections::BTreeMap;
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};

use super::*;
use crate::test_support::{Tree, assert_tree};
use crate::{Entry, Origin, PlanOptions, Projection};

fn projection(dest: &Utf8Path, state: &Utf8Path) -> Projection {
    Projection::new(dest.to_owned(), state.to_owned())
}

/// One full plan → apply pass, so a status test has something recorded to
/// classify.
fn project(projection: &Projection, owner: &str, desired: &BTreeMap<Utf8PathBuf, Entry>) {
    let mut run = projection.begin().expect("begin");
    run.plan(owner, desired, Origin::Caller, PlanOptions::default())
        .expect("plan");
    run.apply().expect("apply");
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
    assert_eq!(
        projection(dest.root(), state.root())
            .status()
            .expect("status"),
        Status::default()
    );
}

#[test]
fn a_missing_state_directory_reports_nothing() {
    let dest = Tree::new().materialize();
    let elsewhere = Tree::new().materialize();
    let missing = elsewhere.path("never-created");
    assert!(!missing.exists(), "the state directory must not exist");

    let report = projection(dest.root(), &missing).status().expect("status");

    assert_eq!(report, Status::default());
    assert!(
        !missing.exists(),
        "a read never creates the state directory"
    );
}

#[test]
fn without_a_state_directory_everything_on_disk_is_foreign() {
    let dest = Tree::new().file("theirs.txt", "not ours").materialize();
    let elsewhere = Tree::new().materialize();

    let report = projection(dest.root(), &elsewhere.path("never-created"))
        .status()
        .expect("status");

    assert_eq!(states(&report), vec![("theirs.txt", PathState::Foreign)]);
}

#[test]
fn reports_one_state_per_path_of_the_union() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(dest.root(), state.root());
    let tree = Tree::new()
        .file("clean.txt", "as written")
        .file("drifted.txt", "as written")
        .file("gone.txt", "as written");
    project(&projection, "own", &tree.entries());

    fs::write(dest.path("drifted.txt"), "edited by hand").expect("edit");
    fs::remove_file(dest.path("gone.txt")).expect("delete");
    fs::write(dest.path("theirs.txt"), "never ours").expect("plant a foreign file");

    assert_eq!(
        states(&projection.status().expect("status")),
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
    let projection = projection(dest.root(), state.root());
    let tree = Tree::new().file("a/b/mine.txt", "projected");
    project(&projection, "own", &tree.entries());

    // The manifest records no directories, so the parents apply created
    // for an owned file are unrecorded like any other directory.
    assert_eq!(
        states(&projection.status().expect("status")),
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
    let projection = projection(dest.root(), state.root());
    let tree = Tree::new().file("a/b.txt", "alpha");
    project(&projection, "own", &tree.entries());
    let manifest = fs::read(state.path(crate::MANIFEST_FILE_NAME)).expect("read manifest");

    projection.status().expect("status");

    assert_tree(dest.root(), &tree);
    assert_tree(
        state.root(),
        &Tree::new()
            .file(crate::MANIFEST_FILE_NAME, manifest)
            .file(crate::LOCK_FILE_NAME, ""),
    );
}

#[test]
fn the_state_subtree_inside_the_destination_never_classifies() {
    let dest = Tree::new().materialize();
    let projection = projection(dest.root(), &dest.path(".proiectio"));
    let tree = Tree::new().file("a.txt", "alpha");
    project(&projection, "own", &tree.entries());

    let report = projection.status().expect("status");

    assert_eq!(states(&report), vec![("a.txt", PathState::Clean)]);
}
