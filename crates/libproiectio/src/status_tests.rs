use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};

use super::*;
use crate::test_support::{Tree, assert_tree};
use crate::{Desired, Entry, PathFacts, PathShape, PlanOptions, Projection};

fn projection(dest: &Utf8Path, state: &Utf8Path) -> Projection {
    Projection::new(dest, Some(state)).expect("a projection")
}

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
    let tree = Tree::new()
        .executable("bin/tool", "#!/bin/sh\n")
        .symlink("current", "bin/tool");
    project(&projection, "site", tree.entries());
    project(&projection, "harness", tree.entries());
    fs::write(dest.path("theirs.txt"), "never ours").expect("plant a foreign file");

    let report = projection.status().expect("status");

    assert_eq!(
        facts(&report, "bin/tool"),
        &Some(PathFacts {
            shape: Some(PathShape::File { executable: true }),
            owners: BTreeSet::from(["harness".to_owned(), "site".to_owned()]),
            origin: None,
        })
    );
    assert_eq!(
        facts(&report, "current"),
        &Some(PathFacts {
            shape: Some(PathShape::Symlink {
                target: Some("bin/tool".to_owned()),
            }),
            owners: BTreeSet::from(["harness".to_owned(), "site".to_owned()]),
            origin: None,
        })
    );
    assert_eq!(facts(&report, "theirs.txt"), &None);
}

#[test]
fn a_drifted_link_states_the_target_it_now_carries() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(dest.root(), state.root());
    project(
        &projection,
        "site",
        Tree::new()
            .file("bin/tool", "#!/bin/sh\n")
            .symlink("current", "bin/tool")
            .entries(),
    );
    let link = dest.path("current");
    fs::remove_file(&link).expect("drop the projected link");
    std::os::unix::fs::symlink("bin/other", &link).expect("plant a link elsewhere");

    let report = projection.status().expect("status");

    assert_eq!(
        report.rows[Utf8Path::new("current")].verdict,
        PathState::Drifted
    );
    assert_eq!(
        facts(&report, "current"),
        &Some(PathFacts {
            shape: Some(PathShape::Symlink {
                target: Some("bin/other".to_owned()),
            }),
            owners: BTreeSet::from(["site".to_owned()]),
            origin: None,
        })
    );
}

#[test]
fn a_link_no_longer_on_disk_names_no_target() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(dest.root(), state.root());
    project(
        &projection,
        "site",
        Tree::new()
            .file("bin/tool", "#!/bin/sh\n")
            .symlink("gone", "bin/tool")
            .symlink("clobbered", "bin/tool")
            .entries(),
    );
    fs::remove_file(dest.path("gone")).expect("delete the link");
    fs::remove_file(dest.path("clobbered")).expect("delete the link");
    fs::write(dest.path("clobbered"), "a file now").expect("plant a file in its place");

    let report = projection.status().expect("status");

    for path in ["gone", "clobbered"] {
        assert_eq!(
            facts(&report, path),
            &Some(PathFacts {
                shape: Some(PathShape::Symlink { target: None }),
                owners: BTreeSet::from(["site".to_owned()]),
                origin: None,
            }),
            "{path}"
        );
    }
}

#[test]
fn a_link_whose_on_disk_target_is_not_utf8_names_no_target() {
    use std::os::unix::ffi::OsStrExt;

    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(dest.root(), state.root());
    project(
        &projection,
        "site",
        Tree::new()
            .file("bin/tool", "#!/bin/sh\n")
            .symlink("current", "bin/tool")
            .entries(),
    );
    let link = dest.path("current");
    fs::remove_file(&link).expect("drop the projected link");
    std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(b"bin/to\xffol"), &link)
        .expect("plant a link with a non-UTF-8 target");

    let report = projection.status().expect("status");

    assert_eq!(
        facts(&report, "current"),
        &Some(PathFacts {
            shape: Some(PathShape::Symlink { target: None }),
            owners: BTreeSet::from(["site".to_owned()]),
            origin: None,
        })
    );
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
            "rows": [
                {
                    "path": "clean.txt",
                    "verdict": "Clean",
                    "facts": {
                        "shape": { "File": { "executable": false } },
                        "owners": ["site"],
                        "origin": null,
                    },
                },
                {
                    "path": "gone.txt",
                    "verdict": "Missing",
                    "facts": {
                        "shape": { "File": { "executable": false } },
                        "owners": ["site"],
                        "origin": null,
                    },
                },
                {
                    "path": "theirs.txt",
                    "verdict": "Foreign",
                    "facts": null,
                },
            ]
        })
    );
}

#[test]
fn a_directory_the_projection_created_reports_no_row() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(dest.root(), state.root());
    let tree = Tree::new().file("a/b/mine.txt", "projected");
    project(&projection, "own", tree.entries());

    assert_eq!(
        states(&projection.status().expect("status")),
        vec![("a/b/mine.txt", PathState::Clean)]
    );
}

#[test]
fn an_empty_unrecorded_directory_reports_no_row() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(dest.root(), state.root());
    let tree = Tree::new().file("mine.txt", "projected");
    project(&projection, "own", tree.entries());
    fs::create_dir(dest.path("stray")).expect("an empty stray directory");

    assert_eq!(
        states(&projection.status().expect("status")),
        vec![("mine.txt", PathState::Clean)]
    );
}

#[test]
fn a_projected_file_replaced_by_a_directory_reports_drifted() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(dest.root(), state.root());
    let tree = Tree::new().file("mine.txt", "projected");
    project(&projection, "own", tree.entries());
    fs::remove_file(dest.path("mine.txt")).expect("delete the projected file");
    fs::create_dir(dest.path("mine.txt")).expect("a directory over the projected path");

    assert_eq!(
        states(&projection.status().expect("status")),
        vec![("mine.txt", PathState::Drifted)]
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

/// One row of the cleanliness contract: what to call the case, and what to do
/// to the destination after it is projected. Nothing done leaves it clean.
type Difference = (&'static str, Option<fn(&Utf8Path)>);

/// What "clean" means over a whole report: every recorded path as written and
/// nothing on disk the manifest never wrote. One path in any other state is
/// enough to say no.
#[test]
fn a_report_is_clean_only_where_every_row_is() {
    let tree = Tree::new()
        .file("clean.txt", "as written")
        .file("drifted.txt", "as written")
        .file("gone.txt", "as written");
    let cases: [Difference; 4] = [
        ("nothing touched", None),
        (
            "a drifted path",
            Some(|dest| {
                fs::write(dest.join("drifted.txt"), "edited by hand").expect("edit");
            }),
        ),
        (
            "a missing path",
            Some(|dest| {
                fs::remove_file(dest.join("gone.txt")).expect("delete");
            }),
        ),
        (
            "a foreign path",
            Some(|dest| {
                fs::write(dest.join("theirs.txt"), "never ours").expect("plant");
            }),
        ),
    ];

    for (case, differ) in cases {
        let dest = Tree::new().materialize();
        let state = Tree::new().materialize();
        let classifying = projection(dest.root(), state.root());
        project(&classifying, "own", tree.entries());
        if let Some(differ) = differ {
            differ(dest.root());
        }

        assert_eq!(
            classifying.status().expect("status").is_clean(),
            differ.is_none(),
            "with {case}"
        );
    }
}

#[test]
fn a_report_of_no_rows_is_clean() {
    assert!(Status::default().is_clean());
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
