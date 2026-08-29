use std::collections::BTreeMap;
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};

use super::*;
use crate::test_support::{Fixture, Tree, assert_tree, origins_of};
use crate::{
    Action, ApplyOutcome, Desired, Entry, Error, LOCK_FILE_NAME, MANIFEST_FILE_NAME, Origin,
    PathState, Refusal, RemovalScope,
};

// A projection over two fixture directories, the state directory outside
// the destination.
fn projection(dest: &Fixture, state: &Utf8Path) -> Projection {
    Projection::new(dest.root(), Some(state)).expect("a projection")
}

fn desired(tree: &Tree) -> Desired {
    Desired::from_caller(tree.entries())
}

// The definition of done: a full begin → plan → apply cycle, with the caller
// opening nothing.

#[test]
fn a_run_projects_a_tree_and_records_it() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root());
    let tree = Tree::new()
        .file("notes/a.txt", "alpha")
        .executable("bin/run", "#!/bin/sh\n");

    let mut run = projection.begin().expect("begin");
    run.plan("harness", &desired(&tree), PlanOptions::default())
        .expect("plan");
    let report = run.apply().expect("apply");

    assert_eq!(
        report.outcomes,
        BTreeMap::from([
            ("bin/run".into(), ApplyOutcome::Written),
            ("notes/a.txt".into(), ApplyOutcome::Written),
        ])
    );
    assert_tree(dest.root(), &tree);
    assert_eq!(
        projection
            .manifest()
            .expect("manifest")
            .entries
            .keys()
            .len(),
        2
    );
    assert!(
        projection
            .status()
            .expect("status")
            .paths
            .values()
            .any(|state| *state == PathState::Clean)
    );
}

#[test]
fn beginning_creates_the_state_directory_a_first_run_has_not_got() {
    let dest = Tree::new().materialize();
    let elsewhere = Tree::new().materialize();
    let state = elsewhere.path("state/proiectio");
    assert!(!state.exists());

    let run = projection(&dest, &state).begin().expect("begin");

    assert!(state.is_dir(), "begin creates the state directory");
    assert!(state.join(LOCK_FILE_NAME).is_file());
    assert!(run.manifest().entries.is_empty());
}

// The guard covers the whole run, load included: while one lives, no other
// writer starts.
#[test]
fn a_second_run_meets_lock_held_while_the_first_lives() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root());
    let first = projection.begin().expect("first begin");

    let contender = projection.clone();
    let error = std::thread::spawn(move || contender.begin().map(|_| ()))
        .join()
        .expect("contender thread")
        .expect_err("the lock is held");

    assert!(matches!(error, Error::LockHeld { .. }));
    assert!(!error.is_refusal(), "a contended lock is exit-1 territory");

    drop(first);
    projection.begin().expect("begin once the first run ended");
}

#[test]
fn a_run_that_decided_no_plan_writes_nothing() {
    let dest = Tree::new().file("theirs.txt", "not ours").materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root());

    let run = projection.begin().expect("begin");
    assert!(run.planned().is_none());
    let report = run.apply().expect("apply");

    assert!(report.outcomes.is_empty());
    assert!(report.manifest.entries.is_empty());
    assert_tree(dest.root(), &Tree::new().file("theirs.txt", "not ours"));
    // `begin` created the lock file; nothing else was written.
    assert_tree(state.root(), &Tree::new().file(LOCK_FILE_NAME, ""));
}

#[test]
fn deciding_again_replaces_the_kept_plan() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root());

    let mut run = projection.begin().expect("begin");
    run.plan(
        "harness",
        &desired(&Tree::new().file("first.txt", "one")),
        PlanOptions::default(),
    )
    .expect("first plan");
    run.plan(
        "harness",
        &desired(&Tree::new().file("second.txt", "two")),
        PlanOptions::default(),
    )
    .expect("second plan");

    assert_eq!(
        run.planned()
            .expect("a plan")
            .actions
            .keys()
            .map(|path| path.as_str())
            .collect::<Vec<_>>(),
        vec!["second.txt"]
    );
    run.apply().expect("apply");
    assert_tree(dest.root(), &Tree::new().file("second.txt", "two"));
}

// The in-dest state directory is created through the destination handle,
// which has to reach a prefix with directories above it as the ambient
// create did.
#[test]
fn beginning_creates_a_nested_in_dest_state_directory() {
    let dest = Tree::new().materialize();
    let state = dest.path(".local/state/proiectio");
    let projection = projection(&dest, &state);
    let tree = Tree::new().file("a.txt", "alpha");

    let mut run = projection.begin().expect("begin");
    run.plan("harness", &desired(&tree), PlanOptions::default())
        .expect("plan");
    run.apply().expect("apply");

    assert!(state.join(MANIFEST_FILE_NAME).is_file());
    // The state subtree itself never classifies. The directories above it
    // are unrecorded directories like any other and read Foreign, which is
    // what keeps the manifest and the lock file out of the report while
    // nothing pretends the projection owns `.local`.
    assert_eq!(
        projection
            .status()
            .expect("status")
            .paths
            .into_iter()
            .map(|(path, state)| (path.to_string(), state))
            .collect::<Vec<_>>(),
        vec![
            (".local".to_owned(), PathState::Foreign),
            (".local/state".to_owned(), PathState::Foreign),
            ("a.txt".to_owned(), PathState::Clean),
        ]
    );
}

// A state directory inside the destination is reached through the
// destination handle, so a prefix component that is a symlink out of the
// target is refused rather than followed — the handle and the prefix
// `state_prefix` excludes from classification name one directory or there
// is no run.
#[test]
fn an_in_dest_state_directory_symlinked_out_of_the_target_refuses() {
    let elsewhere = Tree::new().materialize();
    let dest = Tree::new()
        .symlink(".proiectio", elsewhere.root().as_str())
        .materialize();
    let projection = projection(&dest, &dest.path(".proiectio"));

    let error = projection
        .begin()
        .expect_err("the state prefix leaves the target");
    assert!(matches!(error, Error::Io { .. }), "got {error:?}");

    // Nothing was written through the link, and the read agrees.
    assert_tree(elsewhere.root(), &Tree::new());
    assert!(
        projection.status().is_err(),
        "the read refuses the same escape"
    );
}

#[test]
fn a_removal_run_clears_what_the_owner_holds() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root());
    let tree = Tree::new().file("notes/a.txt", "alpha");

    let mut run = projection.begin().expect("begin");
    run.plan("harness", &desired(&tree), PlanOptions::default())
        .expect("plan");
    run.apply().expect("apply");

    let mut run = projection.begin().expect("begin the removal");
    run.plan_removal("harness", RemovalScope::Everything, PlanOptions::default())
        .expect("plan the removal");
    run.apply().expect("apply the removal");

    assert_tree(dest.root(), &Tree::new());
    assert!(projection.manifest().expect("manifest").entries.is_empty());
}

// Deciding discards the kept plan before it decides, so a decision that
// fails leaves the run with nothing to apply rather than with the plan the
// caller was replacing — which `apply` would otherwise execute in place of
// the decision that never happened.
#[test]
fn a_decision_that_fails_leaves_no_plan_behind() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root());

    let mut run = projection.begin().expect("begin");
    run.plan(
        "harness",
        &desired(&Tree::new().file("first.txt", "one")),
        PlanOptions::default(),
    )
    .expect("first plan");

    // Nesting the destination past the walk's limit fails the observation
    // every later decision starts from.
    fs::create_dir_all(dest.path(["d"; crate::MAX_WALK_DEPTH + 1].join("/"))).expect("nest");
    let error = run
        .plan(
            "harness",
            &desired(&Tree::new().file("second.txt", "two")),
            PlanOptions::default(),
        )
        .expect_err("the observation fails");
    assert!(matches!(error, Error::DestinationTooDeep { .. }), "{error}");

    assert!(run.planned().is_none(), "the replaced plan is gone");
    let report = run.apply().expect("apply");
    assert!(report.outcomes.is_empty());
    assert!(!dest.path("first.txt").exists(), "nothing was projected");
}

// A removal is decided from the manifest, so its plan has no source tree to
// name.
#[test]
fn a_removal_plan_names_no_source() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root());

    let plan = projection
        .plan_removal("harness", RemovalScope::Everything, PlanOptions::default())
        .expect("plan the removal");

    assert!(plan.origins.is_empty());
}

// The refusals apply raises come from deep inside the walk, so this is what
// proves the plan's origin reaches them.
#[test]
fn a_refusal_raised_by_applying_names_the_plans_origin() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root());
    let mapping = Utf8PathBuf::from("/etc/harness/skills.toml");
    let desired = Desired::from_source(
        BTreeMap::from([(
            Utf8PathBuf::from("../escape"),
            Entry::File {
                contents: b"out".to_vec(),
                executable: false,
            },
        )]),
        Origin::Mapping {
            path: mapping.clone(),
        },
    );

    let mut run = projection.begin().expect("begin");
    let plan = run
        .plan("harness", &desired, PlanOptions::default())
        .expect("plan");
    assert_eq!(
        plan.actions.get(Utf8Path::new("../escape")),
        Some(&Action::Refuse {
            refusal: Refusal::Containment
        })
    );

    let error = run
        .apply()
        .expect_err("a plan carrying a refusal applies nothing");

    match &error {
        Error::Refused(refused) => {
            assert_eq!(
                origins_of(refused),
                BTreeMap::from([(
                    Utf8PathBuf::from("../escape"),
                    Origin::Mapping { path: mapping },
                )])
            );
        }
        other => panic!("expected Containment, got {other:?}"),
    }
    assert_eq!(
        error.to_string(),
        "refusing paths that violate containment: \
         ../escape (from mapping /etc/harness/skills.toml)"
    );
    assert_tree(dest.root(), &Tree::new());
}

// The plan a read returns is a report: it says what applying would do and
// carries no lock, so a run can start while a caller still holds one.
#[test]
fn a_plan_from_a_read_takes_no_lock() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root());
    let tree = Tree::new().file("notes/a.txt", "alpha");

    let plan = projection
        .plan("harness", &desired(&tree), PlanOptions::default())
        .expect("plan");

    assert_eq!(
        plan.actions
            .keys()
            .map(|path| path.as_str())
            .collect::<Vec<_>>(),
        vec!["notes/a.txt"]
    );
    // No lock file, no state directory: a read creates neither.
    assert_eq!(fs::read_dir(state.root()).expect("read state").count(), 0);
    projection.begin().expect("a read left no guard behind");
}

#[test]
fn applying_persists_the_manifest_the_next_run_loads() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root());
    let tree = Tree::new().file("notes/a.txt", "alpha");

    let mut run = projection.begin().expect("begin");
    run.plan("harness", &desired(&tree), PlanOptions::default())
        .expect("plan");
    let report = run.apply().expect("apply");

    assert!(state.path(MANIFEST_FILE_NAME).is_file());
    let next = projection.begin().expect("begin the second run");
    assert_eq!(*next.manifest(), report.manifest);
    assert_eq!(next.projection(), &projection);
}
