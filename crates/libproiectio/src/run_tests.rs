use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};

use super::*;
use crate::test_support::{Fixture, Tree, assert_tree, origins_of, state_at};
use crate::{
    Action, ApplyOutcome, Desired, Entry, Error, ExternalTargetPolicy, IoRole, LOCK_FILE_NAME,
    MANIFEST_FILE_NAME, Manifest, ManifestEntry, Origin, PathState, Refusal, RefusalKind,
    RemovalScope, Stopped, save_manifest, sha256_hex,
};

fn projection(dest: &Fixture, state: &Utf8Path) -> Projection {
    Projection::new(dest.root(), Some(state)).expect("a projection")
}

fn desired(tree: &Tree) -> Desired {
    Desired::from_caller(tree.entries())
}

fn owners_of(manifest: &Manifest, path: &str) -> BTreeSet<String> {
    manifest.entries[Utf8Path::new(path)].owners.clone()
}

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
        report
            .report
            .rows
            .iter()
            .map(|(path, row)| (path.clone(), row.verdict))
            .collect::<BTreeMap<Utf8PathBuf, ApplyOutcome>>(),
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
            .rows
            .values()
            .any(|row| row.verdict == PathState::Clean)
    );
}

#[test]
fn a_run_refuses_desired_paths_that_enter_pruned_components() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root())
        .with_pruned_components([".git"])
        .expect("a path component");
    let tree = Tree::new()
        .file(".git/config", "root metadata")
        .file("vendor/project/.git/config", "nested metadata")
        .file(".github/workflows/ci.yml", "jobs: {}")
        .file("src/lib.rs", "pub fn live() {}");

    let mut run = projection.begin().expect("begin");
    let plan = run
        .plan("harness", &desired(&tree), PlanOptions::default())
        .expect("plan");

    for path in [".git/config", "vendor/project/.git/config"] {
        assert!(matches!(
            plan.actions[Utf8Path::new(path)],
            Action::Refuse {
                refusal: Refusal::Containment { through: None }
            }
        ));
    }
    assert!(matches!(
        plan.actions[Utf8Path::new(".github/workflows/ci.yml")],
        Action::Write { .. }
    ));

    let stopped = run.apply().expect_err("a refusal applies nothing");
    assert!(matches!(
        stopped.stopped,
        Stopped::Applying(Error::Refused(_))
    ));
    assert_tree(dest.root(), &Tree::new());
}

#[test]
fn a_pruned_child_keeps_a_drifted_directory_from_looking_empty() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root())
        .with_pruned_components([".git"])
        .expect("a path component");
    let wanted = Tree::new().file("cache", "managed");

    let mut run = projection.begin().expect("begin");
    run.plan("harness", &desired(&wanted), PlanOptions::default())
        .expect("plan initial file");
    run.apply().expect("apply initial file");

    fs::remove_file(dest.path("cache")).expect("remove the managed file");
    fs::create_dir_all(dest.path("cache/.git")).expect("make pruned directory");
    fs::write(dest.path("cache/.git/config"), "metadata").expect("write pruned child");

    let plan = projection
        .plan(
            "harness",
            &desired(&wanted),
            PlanOptions {
                drift: DriftPolicy::Overwrite,
                ..PlanOptions::default()
            },
        )
        .expect("plan drift overwrite");

    assert!(matches!(
        plan.plan.actions[Utf8Path::new("cache")],
        Action::Refuse {
            refusal: Refusal::DirectoryInTheWay { .. }
        }
    ));
    assert_eq!(
        fs::read(dest.path("cache/.git/config")).expect("pruned child remains"),
        b"metadata"
    );
}

#[test]
fn a_pruned_child_keeps_a_scaffolding_parent_from_being_replaced() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root())
        .with_pruned_components([".git"])
        .expect("a path component");

    let mut run = projection.begin().expect("begin");
    run.plan(
        "harness",
        &desired(&Tree::new().file("cache/old", "managed")),
        PlanOptions::default(),
    )
    .expect("plan initial tree");
    run.apply().expect("apply initial tree");
    fs::create_dir_all(dest.path("cache/.git")).expect("make pruned directory");
    fs::write(dest.path("cache/.git/config"), "metadata").expect("write pruned child");

    let plan = projection
        .plan(
            "harness",
            &desired(&Tree::new().file("cache", "replacement")),
            PlanOptions::default(),
        )
        .expect("plan parent replacement");

    assert!(matches!(
        plan.plan.actions[Utf8Path::new("cache")],
        Action::Refuse {
            refusal: Refusal::DirectoryInTheWay { .. }
        }
    ));
    assert_eq!(
        fs::read(dest.path("cache/.git/config")).expect("pruned child remains"),
        b"metadata"
    );
}

#[test]
fn a_manifest_entry_inside_a_pruned_component_is_an_error() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let unscoped = projection(&dest, state.root());
    let tree = Tree::new().file("vendor/project/.git/config", "metadata");

    let mut run = unscoped.begin().expect("begin unscoped");
    run.plan("harness", &desired(&tree), PlanOptions::default())
        .expect("plan unscoped");
    run.apply().expect("apply unscoped");

    let scoped = projection(&dest, state.root())
        .with_pruned_components([".git"])
        .expect("a path component");
    for error in [
        scoped.manifest().expect_err("manifest is outside scope"),
        scoped.status().expect_err("status cannot classify it"),
        scoped.begin().expect_err("a run cannot load it"),
    ] {
        assert!(matches!(
            error,
            Error::ManifestPathPruned { path }
                if path == Utf8Path::new("vendor/project/.git/config")
        ));
    }
}

#[test]
fn a_removal_that_resolves_through_a_recorded_link_into_a_pruned_component_refuses() {
    let dest = Tree::new()
        .file(".git/config", "metadata")
        .symlink("alias", ".git")
        .materialize();
    let state = Tree::new().materialize();
    let owners = BTreeSet::from(["harness".to_owned()]);
    let manifest = Manifest {
        version: crate::MANIFEST_VERSION,
        entries: BTreeMap::from([
            (
                Utf8PathBuf::from("alias"),
                ManifestEntry {
                    kind: crate::EntryKind::Symlink,
                    hash: sha256_hex(b".git"),
                    executable: false,
                    owners: owners.clone(),
                },
            ),
            (
                Utf8PathBuf::from("alias/config"),
                ManifestEntry {
                    kind: crate::EntryKind::File,
                    hash: sha256_hex(b"metadata"),
                    executable: false,
                    owners,
                },
            ),
        ]),
    };
    save_manifest(&state_at(state.root()), &manifest).expect("seed manifest");
    let projection = projection(&dest, state.root())
        .with_pruned_components([".git"])
        .expect("a path component");

    let mut run = projection.begin().expect("begin");
    let plan = run
        .plan_removal("harness", RemovalScope::Everything, DriftPolicy::Refuse)
        .expect("plan removal");
    let action = &plan.actions[Utf8Path::new("alias/config")];
    assert!(
        matches!(
            action,
            Action::Refuse {
                refusal: Refusal::Containment {
                    through: Some(link)
                }
            } if link == Utf8Path::new("alias")
        ),
        "got {action:?}"
    );

    run.apply().expect_err("a refusal applies nothing");
    assert_eq!(
        fs::read(dest.path(".git/config")).expect("metadata stays"),
        b"metadata"
    );
}

#[test]
fn a_named_removal_path_inside_a_pruned_component_refuses() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root())
        .with_pruned_components([".git"])
        .expect("a path component");
    let requested = BTreeSet::from([Utf8PathBuf::from(".git/config")]);

    let plan = projection
        .plan_removal(
            "harness",
            RemovalScope::Paths(&requested),
            DriftPolicy::Refuse,
        )
        .expect("plan removal");

    assert!(matches!(
        plan.plan.actions[Utf8Path::new(".git/config")],
        Action::Refuse {
            refusal: Refusal::Containment { through: None }
        }
    ));
}

#[test]
fn a_link_target_inside_a_pruned_component_needs_the_external_target_permission() {
    let dest = Tree::new().file(".git/config", "metadata").materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root())
        .with_pruned_components([".git"])
        .expect("a path component");
    let tree = Tree::new().symlink("git-config", ".git/config");

    let refused = projection
        .plan("harness", &desired(&tree), PlanOptions::default())
        .expect("plan");
    assert!(matches!(
        refused.plan.actions[Utf8Path::new("git-config")],
        Action::Refuse {
            refusal: Refusal::ExternalTarget { .. }
        }
    ));

    let mut run = projection.begin().expect("begin");
    run.plan(
        "harness",
        &desired(&tree),
        PlanOptions {
            external_targets: ExternalTargetPolicy::Allow,
            ..PlanOptions::default()
        },
    )
    .expect("plan with permission");
    run.apply().expect("write the pointer");
    assert_eq!(
        fs::read_link(dest.path("git-config")).expect("read link target"),
        std::path::Path::new(".git/config")
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

    match &error {
        Error::LockHeld { path } => assert_eq!(*path, state.path(LOCK_FILE_NAME)),
        other => panic!("expected LockHeld, got {other:?}"),
    }
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

    assert!(report.report.is_empty());
    assert!(report.manifest.entries.is_empty());
    assert_tree(dest.root(), &Tree::new().file("theirs.txt", "not ours"));
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
    assert_eq!(
        projection
            .status()
            .expect("status")
            .iter()
            .map(|(path, row)| (path.to_string(), row.verdict))
            .collect::<Vec<_>>(),
        vec![("a.txt".to_owned(), PathState::Clean)]
    );
}

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
    assert!(
        matches!(
            error,
            Error::Io {
                role: IoRole::StateDirectory,
                ..
            }
        ),
        "got {error:?}"
    );

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
    run.plan_removal("harness", RemovalScope::Everything, DriftPolicy::Refuse)
        .expect("plan the removal");
    run.apply().expect("apply the removal");

    assert_tree(dest.root(), &Tree::new());
    assert!(projection.manifest().expect("manifest").entries.is_empty());
}

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
    assert!(report.report.is_empty());
    assert!(!dest.path("first.txt").exists(), "nothing was projected");
}

#[test]
fn a_removal_plan_names_no_source() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root());

    let planned = projection
        .plan_removal("harness", RemovalScope::Everything, DriftPolicy::Refuse)
        .expect("plan the removal");

    assert!(planned.plan.origins.is_empty());
}

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
            refusal: Refusal::Containment { through: None }
        })
    );

    let stopped = run
        .apply()
        .expect_err("a plan carrying a refusal applies nothing");

    match stopped.stopped.error() {
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
    assert!(!stopped.applied_anything());
    assert_eq!(
        stopped.to_string(),
        "refusing paths that violate containment: \
         ../escape (from mapping /etc/harness/skills.toml)"
    );
    assert_tree(dest.root(), &Tree::new());
}

#[test]
fn the_plans_refused_is_the_error_applying_it_raises() {
    let dest = Tree::new().file("theirs.txt", "not ours").materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root());
    let tree = Tree::new()
        .file("theirs.txt", "ours now")
        .file("notes/a.txt", "alpha");

    let mut run = projection.begin().expect("begin");
    let refused = run
        .plan("harness", &desired(&tree), PlanOptions::default())
        .expect("plan")
        .refused()
        .expect("the foreign path is refused");

    let stopped = run
        .apply()
        .expect_err("a plan carrying a refusal applies nothing");

    assert!(!stopped.applied_anything());
    match stopped.stopped {
        Stopped::Applying(Error::Refused(raised)) => assert_eq!(raised, refused),
        other => panic!("expected a refusal met while applying, got {other:?}"),
    }
    assert_eq!(refused.kind(), RefusalKind::Foreign);
    assert_eq!(
        refused.keys().collect::<Vec<_>>(),
        vec![Utf8Path::new("theirs.txt")]
    );
}

#[test]
fn a_plan_from_a_read_takes_no_lock() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root());
    let tree = Tree::new().file("notes/a.txt", "alpha");

    let planned = projection
        .plan("harness", &desired(&tree), PlanOptions::default())
        .expect("plan");

    assert_eq!(
        planned
            .plan
            .actions
            .keys()
            .map(|path| path.as_str())
            .collect::<Vec<_>>(),
        vec!["notes/a.txt"]
    );
    assert_eq!(fs::read_dir(state.root()).expect("read state").count(), 0);
    projection.begin().expect("a read left no guard behind");
}

#[test]
fn a_read_returns_the_manifest_its_plan_was_decided_against() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root());
    let tree = Tree::new().file("conf", "one");

    let mut run = projection.begin().expect("begin");
    run.plan("x", &desired(&tree), PlanOptions::default())
        .expect("plan");
    run.apply().expect("apply");

    let planned = projection
        .plan("x", &desired(&tree), PlanOptions::default())
        .expect("plan");

    let mut run = projection.begin().expect("begin");
    run.plan("y", &desired(&tree), PlanOptions::default())
        .expect("plan");
    run.apply().expect("apply");

    let only_x = BTreeSet::from(["x".to_owned()]);
    assert_eq!(owners_of(&planned.manifest, "conf"), only_x);
    assert_eq!(
        planned
            .report()
            .rows
            .get(Utf8Path::new("conf"))
            .expect("a row")
            .facts
            .as_ref()
            .expect("facts")
            .owners,
        only_x
    );
    assert_eq!(
        owners_of(&projection.manifest().expect("manifest"), "conf"),
        BTreeSet::from(["x".to_owned(), "y".to_owned()])
    );
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

#[test]
fn no_entry_point_that_decides_a_plan_takes_a_name_that_is_not_an_owner() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root());
    let tree = Tree::new().file("notes/a.txt", "alpha");

    for name in ["", " ", "\t", "\n  "] {
        let refused = |error: Error| {
            assert!(
                matches!(&error, Error::OwnerNotNamed { owner } if owner == name),
                "{name:?}: {error}"
            );
        };

        refused(
            projection
                .plan(name, &desired(&tree), PlanOptions::default())
                .expect_err("a refused owner"),
        );
        refused(
            projection
                .plan_removal(name, RemovalScope::Everything, DriftPolicy::Refuse)
                .expect_err("a refused owner"),
        );

        let mut run = projection.begin().expect("begin");
        refused(
            run.plan(name, &desired(&tree), PlanOptions::default())
                .expect_err("a refused owner"),
        );
        refused(
            run.plan_removal(name, RemovalScope::Everything, DriftPolicy::Refuse)
                .expect_err("a refused owner"),
        );
    }

    assert_eq!(projection.manifest().expect("manifest"), Manifest::new());
}

#[test]
fn a_name_that_is_not_an_owner_leaves_the_run_with_no_plan() {
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection = projection(&dest, state.root());
    let tree = Tree::new().file("notes/a.txt", "alpha");

    let mut run = projection.begin().expect("begin");
    run.plan("harness", &desired(&tree), PlanOptions::default())
        .expect("plan");
    run.plan("  ", &desired(&tree), PlanOptions::default())
        .expect_err("a refused owner");

    assert!(run.planned().is_none());
}

#[test]
fn the_name_is_refused_before_the_destination_is_opened() {
    let dest = Tree::new().materialize();
    let missing = dest.root().join("no-such-directory");
    let projection = Projection::new(&missing, Some(dest.root())).expect("a projection");

    assert!(matches!(
        projection
            .plan("", &Desired::new(), PlanOptions::default())
            .expect_err("a refused owner"),
        Error::OwnerNotNamed { owner } if owner.is_empty()
    ));
}
