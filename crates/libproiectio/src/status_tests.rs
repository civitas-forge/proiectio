use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};

use super::*;
use crate::test_support::{Tree, assert_tree};
use crate::{Desired, Entry, EntryKind, Origin, PathFacts, PlanOptions, Projection};

fn projection(dest: &Utf8Path, state: &Utf8Path) -> Projection {
    Projection::new(dest.to_owned(), state.to_owned())
}

// One full plan → apply pass, so a status test has something recorded to
// classify.
fn project(projection: &Projection, owner: &str, desired: BTreeMap<Utf8PathBuf, Entry>) {
    let mut run = projection.begin().expect("begin");
    run.plan(
        owner,
        &Desired::from_caller(desired),
        PlanOptions::default(),
    )
    .expect("plan");
    run.apply().expect("apply");
}

fn states(status: &Status) -> Vec<(&str, PathState)> {
    status
        .iter()
        .map(|(path, row)| (path.as_str(), row.verdict))
        .collect()
}

fn facts<'status>(status: &'status Status, path: &str) -> &'status Option<PathFacts> {
    &status.rows[Utf8Path::new(path)].facts
}

#[test]
fn a_destination_with_no_manifest_reports_nothing() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();

    // The state directory exists and holds no manifest file: nothing has
    // been projected yet.
    assert_eq!(fs::read_dir(state.root()).expect("read state").count(), 0);
    assert!(
        projection(dest.root(), state.root())
            .status()
            .expect("status")
            .is_empty()
    );
}

#[test]
fn a_missing_state_directory_reports_nothing() {
    let dest = Tree::new().materialize();
    let elsewhere = Tree::new().materialize();
    let missing = elsewhere.path("never-created");
    assert!(!missing.exists(), "the state directory must not exist");

    let report = projection(dest.root(), &missing).status().expect("status");

    assert!(report.is_empty());
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
    project(&projection, "own", tree.entries());

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
fn a_recorded_row_carries_the_manifest_entry_and_a_foreign_one_carries_nothing() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(dest.root(), state.root());
    let tree = Tree::new().executable("bin/tool", "#!/bin/sh\n");
    project(&projection, "site", tree.entries());
    project(&projection, "harness", tree.entries());
    fs::write(dest.path("theirs.txt"), "never ours").expect("plant a foreign file");

    let report = projection.status().expect("status");

    assert_eq!(
        facts(&report, "bin/tool"),
        &Some(PathFacts {
            kind: EntryKind::File,
            executable: true,
            // A link's target the manifest records only as a hash, so no
            // status row carries one.
            target: None,
            owners: BTreeSet::from(["harness".to_owned(), "site".to_owned()]),
            origin: Origin::Caller,
        })
    );
    assert_eq!(facts(&report, "theirs.txt"), &None);
}

#[test]
fn a_status_serializes_one_row_per_path() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(dest.root(), state.root());
    let tree = Tree::new()
        .file("clean.txt", "as written")
        .file("gone.txt", "as written");
    project(&projection, "site", tree.entries());
    fs::remove_file(dest.path("gone.txt")).expect("delete");
    fs::write(dest.path("theirs.txt"), "never ours").expect("plant a foreign file");

    let json = serde_json::to_value(projection.status().expect("status")).expect("serialize");

    assert_eq!(
        json,
        serde_json::json!({
            "rows": {
                "clean.txt": {
                    "facts": {
                        "kind": "File",
                        "executable": false,
                        "target": null,
                        "owners": ["site"],
                        "origin": "Caller",
                    },
                    "verdict": "Clean",
                },
                "gone.txt": {
                    "facts": {
                        "kind": "File",
                        "executable": false,
                        "target": null,
                        "owners": ["site"],
                        "origin": "Caller",
                    },
                    "verdict": "Missing",
                },
                "theirs.txt": {
                    "facts": null,
                    "verdict": "Foreign",
                },
            }
        })
    );
}

#[test]
fn a_directory_the_projection_created_still_reports_foreign() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(dest.root(), state.root());
    let tree = Tree::new().file("a/b/mine.txt", "projected");
    project(&projection, "own", tree.entries());

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
    project(&projection, "own", tree.entries());
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
    project(&projection, "own", tree.entries());

    let report = projection.status().expect("status");

    assert_eq!(states(&report), vec![("a.txt", PathState::Clean)]);
}
