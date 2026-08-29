use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::fs_utf8::Dir;

use super::*;
use crate::test_support::{Fixture, Tree, assert_tree};
use crate::{
    DriftPolicy, EntryKind, ExternalTargetPolicy, PlanOptions, RemovalScope, decide,
    decide_removal, observe,
};

/// Opens a capability handle at a fixture root. Ambient authority is the
/// test's to spend; the library itself never opens ambient paths.
fn dir_at(root: &Utf8Path) -> Dir {
    Dir::open_ambient_dir(root, cap_std::ambient_authority()).expect("open fixture root as a Dir")
}

/// A fresh destination and a fresh state directory, both empty.
fn fixtures() -> (Fixture, Fixture) {
    (Tree::new().materialize(), Tree::new().materialize())
}

/// The observe → decide half of a run: the manifest as loaded from `state`
/// and the plan decided against it — split out so tests can mutate the disk
/// in the plan-to-apply gap. `policy` rides the default (refusing)
/// external-target policy; [`plan_for_with`] takes the options whole.
fn plan_for(
    dest: &Fixture,
    state: &Fixture,
    owner: &str,
    desired: &BTreeMap<Utf8PathBuf, Entry>,
    policy: DriftPolicy,
) -> (Manifest, Plan) {
    plan_for_with(
        dest,
        state,
        owner,
        desired,
        PlanOptions {
            drift: policy,
            ..PlanOptions::default()
        },
    )
}

/// [`plan_for`] under options the test chooses whole.
fn plan_for_with(
    dest: &Fixture,
    state: &Fixture,
    owner: &str,
    desired: &BTreeMap<Utf8PathBuf, Entry>,
    options: PlanOptions,
) -> (Manifest, Plan) {
    let dest = dir_at(dest.root());
    let state = dir_at(state.root());
    let manifest = load_manifest(&state).expect("load manifest");
    let observations = observe(&dest, &manifest).expect("observe destination");
    let plan = decide(owner, desired, &manifest, &observations, None, options);
    (manifest, plan)
}

/// Applies a plan against the fixtures.
fn apply_at(
    dest: &Fixture,
    state: &Fixture,
    manifest: &Manifest,
    plan: &Plan,
) -> Result<ApplyReport> {
    apply(&dir_at(dest.root()), &dir_at(state.root()), manifest, plan)
}

/// One full observe → decide → apply run.
fn pipeline(
    dest: &Fixture,
    state: &Fixture,
    owner: &str,
    desired: &BTreeMap<Utf8PathBuf, Entry>,
    policy: DriftPolicy,
) -> Result<ApplyReport> {
    let (manifest, plan) = plan_for(dest, state, owner, desired, policy);
    apply_at(dest, state, &manifest, &plan)
}

/// [`pipeline`] under options the test chooses whole.
fn pipeline_with(
    dest: &Fixture,
    state: &Fixture,
    owner: &str,
    desired: &BTreeMap<Utf8PathBuf, Entry>,
    options: PlanOptions,
) -> Result<ApplyReport> {
    let (manifest, plan) = plan_for_with(dest, state, owner, desired, options);
    apply_at(dest, state, &manifest, &plan)
}

/// The manifest as persisted in the state fixture.
fn persisted(state: &Fixture) -> Manifest {
    load_manifest(&dir_at(state.root())).expect("load persisted manifest")
}

/// A hand-built manifest entry under the given owners.
fn recorded(kind: EntryKind, hash: String, owners: &[&str]) -> ManifestEntry {
    ManifestEntry {
        kind,
        hash,
        executable: false,
        owners: owners.iter().map(|owner| (*owner).to_owned()).collect(),
    }
}

/// The names in a fixture directory, sorted.
fn names_in(fixture: &Fixture) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(fixture.root())
        .expect("read fixture dir")
        .map(|entry| entry.expect("dir entry").file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn projects_a_fresh_tree_and_persists_the_manifest() {
    let (dest, state) = fixtures();
    let tree = Tree::new()
        .file("notes/a.txt", "alpha")
        .executable("bin/run", "#!/bin/sh\n");

    let report = pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse)
        .expect("apply succeeds");

    assert_tree(dest.root(), &tree);
    assert_eq!(
        report.outcomes,
        BTreeMap::from([
            ("notes/a.txt".into(), ApplyOutcome::Written),
            ("bin/run".into(), ApplyOutcome::Written),
        ])
    );
    // The report's manifest is the persisted one, atomically written with
    // no tempfile left beside it.
    assert_eq!(persisted(&state), report.manifest);
    assert_eq!(names_in(&state), vec![MANIFEST_FILE_NAME.to_owned()]);
    let entry = &report.manifest.entries[Utf8Path::new("bin/run")];
    assert_eq!(entry.kind, EntryKind::File);
    assert!(entry.executable);
    assert_eq!(entry.hash, sha256_hex(b"#!/bin/sh\n"));
    assert_eq!(entry.owners, BTreeSet::from(["own".to_owned()]));
}

#[test]
fn reapplying_an_unchanged_tree_skips_everything() {
    let (dest, state) = fixtures();
    let tree = Tree::new().file("a.txt", "alpha").file("b/c.txt", "gamma");
    pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse).expect("first apply");

    let report =
        pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse).expect("second apply");

    assert!(
        report
            .outcomes
            .values()
            .all(|outcome| *outcome == ApplyOutcome::Skipped),
        "expected every outcome skipped, got {:?}",
        report.outcomes
    );
    assert_tree(dest.root(), &tree);
}

#[test]
fn overwrites_changed_content_and_updates_the_exec_bit() {
    let (dest, state) = fixtures();
    let v1 = Tree::new().file("tool", "version one\n");
    pipeline(&dest, &state, "own", &v1.entries(), DriftPolicy::Refuse).expect("first apply");

    let v2 = Tree::new().executable("tool", "version two\n");
    let report =
        pipeline(&dest, &state, "own", &v2.entries(), DriftPolicy::Refuse).expect("second apply");

    assert_eq!(
        report.outcomes,
        BTreeMap::from([("tool".into(), ApplyOutcome::Overwritten)])
    );
    assert_tree(dest.root(), &v2);
    assert!(report.manifest.entries[Utf8Path::new("tool")].executable);
}

// Definition of done: error mid-plan leaves a state a re-run completes
// cleanly — the manifest persisted on failure records what was actually
// applied, so the partial run heals instead of wedging behind Foreign.
#[test]
fn a_mid_run_failure_persists_the_applied_entries_and_a_rerun_heals() {
    let (dest, state) = fixtures();
    Tree::new().dir("ro").write_under(dest.root());
    let ro = dest.path("ro");
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o555)).expect("make ro read-only");

    let tree = Tree::new()
        .file("a.txt", "alpha")
        .file("ro/x.txt", "blocked")
        .file("z.txt", "zeta");
    let error = pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse)
        .expect_err("the read-only directory fails the middle write");

    // An OS failure, not a refusal — and the destination holds exactly the
    // one file applied before it: no z.txt, no tempfile litter.
    assert!(!error.is_refusal(), "expected an I/O error, got {error:?}");
    assert_tree(dest.root(), &Tree::new().file("a.txt", "alpha").dir("ro"));
    // The manifest records reality: a.txt applied, nothing else.
    let manifest = persisted(&state);
    assert_eq!(
        manifest.entries.keys().collect::<Vec<_>>(),
        vec![Utf8Path::new("a.txt")]
    );

    fs::set_permissions(&ro, fs::Permissions::from_mode(0o755)).expect("heal the directory");
    let report = pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse)
        .expect("the re-run completes cleanly");

    assert_eq!(
        report.outcomes,
        BTreeMap::from([
            ("a.txt".into(), ApplyOutcome::Skipped),
            ("ro/x.txt".into(), ApplyOutcome::Written),
            ("z.txt".into(), ApplyOutcome::Written),
        ])
    );
    assert_tree(dest.root(), &tree);
}

// Definition of done: file mutated between plan and apply → refusal
// carrying the path.
#[test]
fn drift_in_the_plan_to_apply_gap_is_refused_with_the_path() {
    let (dest, state) = fixtures();
    let v1 = Tree::new().file("m.txt", "old");
    pipeline(&dest, &state, "own", &v1.entries(), DriftPolicy::Refuse).expect("first apply");

    let v2 = Tree::new().file("m.txt", "new");
    let (manifest, plan) = plan_for(&dest, &state, "own", &v2.entries(), DriftPolicy::Refuse);
    // The gap: the user edits the file after the plan, before the apply.
    fs::write(dest.path("m.txt"), "tampered").expect("tamper in the gap");

    let error = apply_at(&dest, &state, &manifest, &plan).expect_err("the re-check refuses");

    match error {
        Error::Drift { paths } => assert_eq!(paths, BTreeSet::from(["m.txt".into()])),
        other => panic!("expected Drift, got {other:?}"),
    }
    // The edit survives, and nothing else was written or littered.
    assert_tree(dest.root(), &Tree::new().file("m.txt", "tampered"));
    assert_eq!(persisted(&state), manifest);
}

// Definition of done: a planted symlinked ancestor is refused — for links
// the projection does not own, wherever they point.
#[test]
fn an_unrecorded_symlinked_ancestor_is_refused() {
    for target in ["real", "../outside", "/etc"] {
        let (dest, state) = fixtures();
        Tree::new()
            .dir("real")
            .symlink("logs", target)
            .write_under(dest.root());

        let desired = Tree::new().file("logs/x.txt", "smuggled").entries();
        let error = pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse)
            .expect_err("no write through an unowned link");

        match error {
            Error::Containment { paths } => {
                assert_eq!(paths, BTreeSet::from(["logs/x.txt".into()]))
            }
            other => panic!("expected Containment for target {target}, got {other:?}"),
        }
        assert!(
            !dest.path("real").join("x.txt").exists(),
            "nothing may land through the link"
        );
    }
}

// Definition of done: foreign files are never touched, and failures leave
// no tempfile behind (assert_tree reports any unexpected entry).
#[test]
fn foreign_files_are_never_touched() {
    let (dest, state) = fixtures();
    let foreign = Tree::new()
        .file("keep.txt", "precious")
        .file("data/nested.txt", "also precious");
    foreign.write_under(dest.root());

    let desired = Tree::new().file("data/mine.txt", "projected");
    pipeline(
        &dest,
        &state,
        "own",
        &desired.entries(),
        DriftPolicy::Refuse,
    )
    .expect("apply");

    // The union, exactly: foreign files byte-identical, ours placed, no
    // litter anywhere.
    assert_tree(
        dest.root(),
        &Tree::new()
            .file("keep.txt", "precious")
            .file("data/nested.txt", "also precious")
            .file("data/mine.txt", "projected"),
    );

    // Removing the projection leaves the foreign files and their
    // directories alone.
    pipeline(&dest, &state, "own", &BTreeMap::new(), DriftPolicy::Refuse).expect("removal");
    assert_tree(dest.root(), &foreign);
}

#[test]
fn removal_removes_owned_paths_in_reverse_and_prunes_emptied_dirs() {
    let (dest, state) = fixtures();
    let tree = Tree::new()
        .file("a/b/c.txt", "one")
        .file("a/b/d.txt", "two")
        .file("top.txt", "three");
    pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse).expect("project");

    let report =
        pipeline(&dest, &state, "own", &BTreeMap::new(), DriftPolicy::Refuse).expect("removal");

    assert!(
        report
            .outcomes
            .values()
            .all(|outcome| *outcome == ApplyOutcome::Removed)
    );
    assert!(report.manifest.entries.is_empty());
    assert_tree(dest.root(), &Tree::new());
}

#[test]
fn pruning_keeps_directories_still_holding_anything() {
    let (dest, state) = fixtures();
    let tree = Tree::new().file("a/b/mine.txt", "projected");
    pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse).expect("project");
    fs::write(dest.path("a/b/theirs.txt"), "foreign").expect("plant a foreign file");

    pipeline(&dest, &state, "own", &BTreeMap::new(), DriftPolicy::Refuse).expect("removal");

    assert_tree(dest.root(), &Tree::new().file("a/b/theirs.txt", "foreign"));
}

/// One observe → [`decide_removal`] → apply run over `scope`: the whole
/// owner, or the paths named.
fn removal_pipeline(
    dest: &Fixture,
    state: &Fixture,
    owner: &str,
    scope: RemovalScope<'_>,
    policy: DriftPolicy,
) -> Result<ApplyReport> {
    let (manifest, plan) = {
        let dest = dir_at(dest.root());
        let state = dir_at(state.root());
        let manifest = load_manifest(&state).expect("load manifest");
        let observations = observe(&dest, &manifest).expect("observe destination");
        let plan = decide_removal(
            owner,
            scope,
            &manifest,
            &observations,
            None,
            PlanOptions {
                drift: policy,
                ..PlanOptions::default()
            },
        );
        (manifest, plan)
    };
    apply_at(dest, state, &manifest, &plan)
}

fn requested(paths: &[&str]) -> BTreeSet<Utf8PathBuf> {
    paths.iter().map(Utf8PathBuf::from).collect()
}

// Definition of done: removing a drifted file refuses and names it.
#[test]
fn removing_a_drifted_file_refuses_with_the_path() {
    let (dest, state) = fixtures();
    let tree = Tree::new()
        .file("a/b.txt", "as written")
        .file("c.txt", "kept");
    pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse).expect("project");
    let edited = Tree::new()
        .file("a/b.txt", "edited by hand")
        .file("c.txt", "kept");
    fs::write(dest.path("a/b.txt"), "edited by hand").expect("edit the file");

    let error = removal_pipeline(
        &dest,
        &state,
        "own",
        RemovalScope::Everything,
        DriftPolicy::Refuse,
    )
    .expect_err("the drifted file refuses");

    assert!(matches!(
        &error,
        Error::Drift { paths } if paths == &BTreeSet::from([Utf8PathBuf::from("a/b.txt")])
    ));
    // The refusal is up front: nothing was removed, the manifest is whole.
    assert_tree(dest.root(), &edited);
    assert_eq!(
        persisted(&state).entries.keys().collect::<Vec<_>>(),
        vec![Utf8Path::new("a/b.txt"), Utf8Path::new("c.txt")]
    );

    // The same removal under --force takes the edited file with it.
    removal_pipeline(
        &dest,
        &state,
        "own",
        RemovalScope::Everything,
        DriftPolicy::Overwrite,
    )
    .expect("forced removal");
    assert_tree(dest.root(), &Tree::new());
}

// Definition of done: pruning empties what removal emptied and keeps a
// directory still holding a file the projection does not own.
#[test]
fn removal_prunes_emptied_dirs_and_keeps_one_holding_a_foreign_file() {
    let (dest, state) = fixtures();
    let tree = Tree::new()
        .file("shared/mine.txt", "projected")
        .file("solely/deep/mine.txt", "projected");
    pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse).expect("project");
    fs::write(dest.path("shared/theirs.txt"), "foreign").expect("plant a foreign file");

    removal_pipeline(
        &dest,
        &state,
        "own",
        RemovalScope::Everything,
        DriftPolicy::Refuse,
    )
    .expect("removal");

    assert_tree(
        dest.root(),
        &Tree::new().file("shared/theirs.txt", "foreign"),
    );
    assert!(persisted(&state).entries.is_empty());
}

#[test]
fn a_subset_removal_clears_the_named_paths_and_leaves_the_rest() {
    let (dest, state) = fixtures();
    let tree = Tree::new()
        .file("a/b/gone.txt", "projected")
        .file("a/kept.txt", "projected")
        .file("top.txt", "projected");
    pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse).expect("project");

    let report = removal_pipeline(
        &dest,
        &state,
        "own",
        RemovalScope::Paths(&requested(&["a/b/gone.txt"])),
        DriftPolicy::Refuse,
    )
    .expect("removal");

    assert_eq!(
        report.outcomes,
        BTreeMap::from([("a/b/gone.txt".into(), ApplyOutcome::Removed)])
    );
    // The emptied `a/b` is pruned; `a` still holds kept.txt and survives.
    assert_tree(
        dest.root(),
        &Tree::new()
            .file("a/kept.txt", "projected")
            .file("top.txt", "projected"),
    );
    assert_eq!(
        persisted(&state).entries.keys().collect::<Vec<_>>(),
        vec![Utf8Path::new("a/kept.txt"), Utf8Path::new("top.txt")]
    );
}

#[test]
fn a_subset_removal_refuses_a_path_that_violates_containment() {
    let (dest, state) = fixtures();
    let tree = Tree::new().file("a.txt", "projected");
    pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse).expect("project");

    let error = removal_pipeline(
        &dest,
        &state,
        "own",
        RemovalScope::Paths(&requested(&["../escape", "a.txt"])),
        DriftPolicy::Refuse,
    )
    .expect_err("the escaping path refuses");

    assert!(matches!(
        &error,
        Error::Containment { paths } if paths == &BTreeSet::from([Utf8PathBuf::from("../escape")])
    ));
    // Up front, so the admitted path in the same request is untouched.
    assert_tree(dest.root(), &tree);
}

#[test]
fn removing_a_missing_path_drops_the_manifest_entry_alone() {
    let (dest, state) = fixtures();
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "gone.txt".into(),
        recorded(EntryKind::File, sha256_hex(b"bye"), &["own"]),
    );
    save_manifest(&dir_at(state.root()), &manifest).expect("seed the state dir");

    let report =
        pipeline(&dest, &state, "own", &BTreeMap::new(), DriftPolicy::Refuse).expect("removal");

    assert_eq!(
        report.outcomes,
        BTreeMap::from([("gone.txt".into(), ApplyOutcome::Removed)])
    );
    assert!(persisted(&state).entries.is_empty());
}

#[test]
fn removing_a_missing_path_refuses_if_a_node_appeared_in_the_gap() {
    let (dest, state) = fixtures();
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "gone.txt".into(),
        recorded(EntryKind::File, sha256_hex(b"bye"), &["own"]),
    );
    save_manifest(&dir_at(state.root()), &manifest).expect("seed the state dir");
    let (manifest, plan) = plan_for(&dest, &state, "own", &BTreeMap::new(), DriftPolicy::Refuse);
    assert_eq!(
        plan.actions,
        BTreeMap::from([("gone.txt".into(), Action::Remove { expected: None })])
    );
    fs::write(dest.path("gone.txt"), "reappeared").expect("a node appears in the gap");

    let error = apply_at(&dest, &state, &manifest, &plan).expect_err("the appearance refuses");

    match error {
        Error::Drift { paths } => assert_eq!(paths, BTreeSet::from(["gone.txt".into()])),
        other => panic!("expected Drift, got {other:?}"),
    }
    assert_tree(dest.root(), &Tree::new().file("gone.txt", "reappeared"));
}

#[test]
fn removing_a_recorded_symlink_unlinks_it_and_leaves_the_target() {
    let (dest, state) = fixtures();
    Tree::new()
        .file("notes", "the target")
        .symlink("latest", "notes")
        .write_under(dest.root());
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "latest".into(),
        recorded(EntryKind::Symlink, sha256_hex(b"notes"), &["own"]),
    );
    save_manifest(&dir_at(state.root()), &manifest).expect("seed the state dir");

    let report =
        pipeline(&dest, &state, "own", &BTreeMap::new(), DriftPolicy::Refuse).expect("removal");

    assert_eq!(
        report.outcomes,
        BTreeMap::from([("latest".into(), ApplyOutcome::Removed)])
    );
    assert_tree(dest.root(), &Tree::new().file("notes", "the target"));
}

#[test]
fn a_skip_records_the_joining_owner_and_release_drops_it() {
    let (dest, state) = fixtures();
    let tree = Tree::new().file("shared.txt", "same bytes");
    pipeline(&dest, &state, "one", &tree.entries(), DriftPolicy::Refuse).expect("owner one");

    let report =
        pipeline(&dest, &state, "two", &tree.entries(), DriftPolicy::Refuse).expect("owner two");
    assert_eq!(
        report.outcomes,
        BTreeMap::from([("shared.txt".into(), ApplyOutcome::Skipped)])
    );
    assert_eq!(
        report.manifest.entries[Utf8Path::new("shared.txt")].owners,
        BTreeSet::from(["one".to_owned(), "two".to_owned()])
    );

    // Owner two departs: released, disk untouched, owner one still holds.
    let report =
        pipeline(&dest, &state, "two", &BTreeMap::new(), DriftPolicy::Refuse).expect("release");
    assert_eq!(
        report.outcomes,
        BTreeMap::from([("shared.txt".into(), ApplyOutcome::Released)])
    );
    assert_eq!(
        report.manifest.entries[Utf8Path::new("shared.txt")].owners,
        BTreeSet::from(["one".to_owned()])
    );
    assert_tree(dest.root(), &tree);

    // The last owner out removes the file.
    pipeline(&dest, &state, "one", &BTreeMap::new(), DriftPolicy::Refuse).expect("removal");
    assert_tree(dest.root(), &Tree::new());
}

#[test]
fn a_plan_carrying_refusals_fails_up_front_and_writes_nothing() {
    let (dest, state) = fixtures();
    let v1 = Tree::new().file("keep.txt", "recorded");
    pipeline(&dest, &state, "own", &v1.entries(), DriftPolicy::Refuse).expect("project");
    fs::write(dest.path("keep.txt"), "edited").expect("drift the file");

    let desired = Tree::new()
        .file("keep.txt", "different")
        .file("new.txt", "fresh");
    let error = pipeline(
        &dest,
        &state,
        "own",
        &desired.entries(),
        DriftPolicy::Refuse,
    )
    .expect_err("the drifted path refuses the whole plan");

    match error {
        Error::Drift { paths } => assert_eq!(paths, BTreeSet::from(["keep.txt".into()])),
        other => panic!("expected Drift, got {other:?}"),
    }
    // Up front means up front: the writable half of the plan did not run.
    assert_tree(dest.root(), &Tree::new().file("keep.txt", "edited"));
    assert_eq!(
        persisted(&state).entries.keys().collect::<Vec<_>>(),
        vec![Utf8Path::new("keep.txt")]
    );
}

#[test]
fn a_write_target_appearing_in_the_gap_refuses_as_foreign() {
    let (dest, state) = fixtures();
    let desired = Tree::new().file("a.txt", "planned").entries();
    let (manifest, plan) = plan_for(&dest, &state, "own", &desired, DriftPolicy::Refuse);
    fs::write(dest.path("a.txt"), "squatter").expect("a foreign node appears in the gap");

    let error = apply_at(&dest, &state, &manifest, &plan).expect_err("never overwrite it");

    match error {
        Error::Foreign { paths } => assert_eq!(paths, BTreeSet::from(["a.txt".into()])),
        other => panic!("expected Foreign, got {other:?}"),
    }
    assert_tree(dest.root(), &Tree::new().file("a.txt", "squatter"));
}

#[test]
fn a_hand_built_plan_with_unnormalized_keys_refuses_containment() {
    let (dest, state) = fixtures();
    let entry = Entry::File {
        contents: b"evil".to_vec(),
        executable: false,
    };
    let plan = Plan {
        owner: "own".to_owned(),
        external_targets: ExternalTargetPolicy::Refuse,
        actions: BTreeMap::from([
            (
                "../escape".into(),
                Action::Write {
                    entry: entry.clone(),
                },
            ),
            ("a/../b".into(), Action::Write { entry }),
        ]),
    };

    let error =
        apply_at(&dest, &state, &Manifest::new(), &plan).expect_err("the gateway re-judges keys");

    match error {
        Error::Containment { paths } => {
            assert_eq!(paths, BTreeSet::from(["../escape".into(), "a/../b".into()]))
        }
        other => panic!("expected Containment, got {other:?}"),
    }
    assert_tree(dest.root(), &Tree::new());
}

/// A path at the depth observe walks is written; one past it is named and
/// nothing is written at all. `load_tree` cannot produce the second — its
/// own walk stops at the same limit — but `load_mapping` can, since it
/// judges keys for containment and never for depth, so this check is what
/// the deep mapping key and the hand-built plan both meet. It is what keeps
/// the projection from creating a destination its own next run could not
/// observe.
#[test]
fn a_plan_writing_past_the_walk_depth_is_named_and_writes_nothing() {
    let (dest, state) = fixtures();
    let entry = Entry::File {
        contents: b"deep".to_vec(),
        executable: false,
    };
    let at_the_limit = Utf8PathBuf::from(format!("{}/leaf", ["d"; MAX_WALK_DEPTH].join("/")));
    let past = Utf8PathBuf::from(format!("{}/leaf", ["d"; MAX_WALK_DEPTH + 1].join("/")));

    let plan = Plan {
        owner: "own".to_owned(),
        actions: BTreeMap::from([(
            at_the_limit.clone(),
            Action::Write {
                entry: entry.clone(),
            },
        )]),
        external_targets: ExternalTargetPolicy::Refuse,
    };
    apply_at(&dest, &state, &Manifest::new(), &plan).expect("the limit itself is writable");
    assert_eq!(
        fs::read_to_string(dest.path(at_the_limit.as_str())).expect("the deep file"),
        "deep"
    );

    let (dest, state) = fixtures();
    let plan = Plan {
        owner: "own".to_owned(),
        actions: BTreeMap::from([(past.clone(), Action::Write { entry })]),
        external_targets: ExternalTargetPolicy::Refuse,
    };

    let error = apply_at(&dest, &state, &Manifest::new(), &plan)
        .expect_err("a write observe could not read back");

    match error {
        Error::DestinationTooDeep { path, limit } => {
            assert_eq!(path, past.parent().expect("the leaf has a parent"));
            assert_eq!(limit, MAX_WALK_DEPTH);
        }
        other => panic!("expected DestinationTooDeep, got {other:?}"),
    }
    assert_tree(dest.root(), &Tree::new());
}

/// A key two directories long that lands 65 deep. Following an owned link
/// restarts the walk at the link's target, so the depth the plan spells is
/// not the depth the walk reaches — and it is the walk's depth that decides
/// whether the next observation can read the node back. The check on the
/// key cannot see this one; the walk names the directory it stopped at.
#[test]
fn a_write_landing_past_the_walk_depth_through_an_owned_link_is_named() {
    let (dest, state) = fixtures();
    let at_the_limit = ["d"; MAX_WALK_DEPTH].join("/");
    Tree::new()
        .dir(&at_the_limit)
        .symlink("deep", &at_the_limit)
        .write_under(dest.root());
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "deep".into(),
        recorded(
            EntryKind::Symlink,
            sha256_hex(at_the_limit.as_bytes()),
            &["own"],
        ),
    );
    let plan = Plan {
        owner: "own".to_owned(),
        actions: BTreeMap::from([(
            "deep/one-more/leaf".into(),
            Action::Write {
                entry: Entry::File {
                    contents: b"past the walk".to_vec(),
                    executable: false,
                },
            },
        )]),
        external_targets: ExternalTargetPolicy::Refuse,
    };

    let error =
        apply_at(&dest, &state, &manifest, &plan).expect_err("the resolved path is past the limit");

    match error {
        Error::DestinationTooDeep { path, limit } => {
            assert_eq!(path, Utf8PathBuf::from(format!("{at_the_limit}/one-more")));
            assert_eq!(limit, MAX_WALK_DEPTH);
        }
        other => panic!("expected DestinationTooDeep, got {other:?}"),
    }
    assert!(!dest.path(&at_the_limit).join("one-more").exists());
}

#[test]
fn a_forged_remove_of_an_unrecorded_path_refuses_foreign() {
    let (dest, state) = fixtures();
    Tree::new()
        .file("victim.txt", "precious")
        .write_under(dest.root());
    let plan = Plan {
        owner: "own".to_owned(),
        external_targets: ExternalTargetPolicy::Refuse,
        actions: BTreeMap::from([(
            "victim.txt".into(),
            Action::Remove {
                expected: Some(NodeSignature {
                    kind: EntryKind::File,
                    hash: sha256_hex(b"precious"),
                    executable: false,
                }),
            },
        )]),
    };

    let error = apply_at(&dest, &state, &Manifest::new(), &plan)
        .expect_err("a matching signature is not authorization");

    match error {
        Error::Foreign { paths } => assert_eq!(paths, BTreeSet::from(["victim.txt".into()])),
        other => panic!("expected Foreign, got {other:?}"),
    }
    assert_tree(dest.root(), &Tree::new().file("victim.txt", "precious"));
}

#[test]
fn a_forged_skip_of_an_unrecorded_path_refuses_instead_of_adopting() {
    let (dest, state) = fixtures();
    Tree::new()
        .file("theirs.txt", "same bytes")
        .write_under(dest.root());
    let plan = Plan {
        owner: "own".to_owned(),
        external_targets: ExternalTargetPolicy::Refuse,
        actions: BTreeMap::from([(
            "theirs.txt".into(),
            Action::Skip {
                expected: NodeSignature {
                    kind: EntryKind::File,
                    hash: sha256_hex(b"same bytes"),
                    executable: false,
                },
            },
        )]),
    };

    let error = apply_at(&dest, &state, &Manifest::new(), &plan)
        .expect_err("adoption would put a foreign file on the removal path");

    match error {
        Error::Foreign { paths } => assert_eq!(paths, BTreeSet::from(["theirs.txt".into()])),
        other => panic!("expected Foreign, got {other:?}"),
    }
    // Never adopted: the state dir records nothing.
    assert_eq!(persisted(&state), Manifest::new());
}

#[test]
fn a_hand_built_plan_replacing_a_region_with_a_whole_file_fails_up_front() {
    let (dest, state) = fixtures();
    Tree::new()
        .file("a.txt", "old")
        .file("conf", "author\n# proiectio\nbody\n")
        .write_under(dest.root());
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "a.txt".into(),
        recorded(EntryKind::File, sha256_hex(b"old"), &["own"]),
    );
    manifest.entries.insert(
        "conf".into(),
        recorded(
            block_kind(Placement::Append),
            sha256_hex(b"body\n"),
            &["own"],
        ),
    );
    let plan = Plan {
        owner: "own".to_owned(),
        external_targets: ExternalTargetPolicy::Refuse,
        actions: BTreeMap::from([
            (
                "a.txt".into(),
                Action::Remove {
                    expected: Some(NodeSignature {
                        kind: EntryKind::File,
                        hash: sha256_hex(b"old"),
                        executable: false,
                    }),
                },
            ),
            (
                "conf".into(),
                Action::Overwrite {
                    entry: Entry::File {
                        contents: b"new".to_vec(),
                        executable: false,
                    },
                    expected: NodeSignature {
                        kind: block_kind(Placement::Append),
                        hash: sha256_hex(b"body\n"),
                        executable: false,
                    },
                },
            ),
        ]),
    };

    let error = apply_at(&dest, &state, &manifest, &plan)
        .expect_err("a path never changes between a whole node and a block");

    match error {
        Error::Block { blocks } => assert_eq!(
            blocks,
            BTreeMap::from([("conf".into(), BlockFault::KindChange)])
        ),
        other => panic!("expected Block, got {other:?}"),
    }
    // Up front means up front: the removal half of the plan did not run.
    assert_tree(
        dest.root(),
        &Tree::new()
            .file("a.txt", "old")
            .file("conf", "author\n# proiectio\nbody\n"),
    );
}

#[test]
fn a_recorded_link_whose_matching_target_is_not_utf8_refuses_containment() {
    use std::os::unix::ffi::OsStrExt;

    let (dest, state) = fixtures();
    let target_bytes: &[u8] = b"re\xffal";
    std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(target_bytes), dest.path("logs"))
        .expect("plant a link with a non-UTF-8 target");
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "logs".into(),
        recorded(EntryKind::Symlink, sha256_hex(target_bytes), &["own"]),
    );
    let plan = Plan {
        owner: "own".to_owned(),
        external_targets: ExternalTargetPolicy::Refuse,
        actions: BTreeMap::from([(
            "logs/x.txt".into(),
            Action::Write {
                entry: Entry::File {
                    contents: b"x".to_vec(),
                    executable: false,
                },
            },
        )]),
    };

    let error = apply_at(&dest, &state, &manifest, &plan)
        .expect_err("an ungradable target is never followed");

    match error {
        Error::Containment { paths } => assert_eq!(paths, BTreeSet::from(["logs/x.txt".into()])),
        other => panic!("expected Containment, got {other:?}"),
    }
}

#[test]
fn an_owned_matching_in_dest_symlink_ancestor_is_followed() {
    let (dest, state) = fixtures();
    Tree::new()
        .dir("real")
        .symlink("logs", "real")
        .write_under(dest.root());
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "logs".into(),
        recorded(EntryKind::Symlink, sha256_hex(b"real"), &["own"]),
    );
    // Deciding never plans a write beneath a surviving link (its no-alias
    // rule), so a write reaches the followed arm only from a hand-built
    // plan — or from a link that appeared in the plan-to-apply gap. The
    // arm still has to do the right thing.
    let plan = Plan {
        owner: "own".to_owned(),
        external_targets: ExternalTargetPolicy::Refuse,
        actions: BTreeMap::from([(
            "logs/x.txt".into(),
            Action::Write {
                entry: Entry::File {
                    contents: b"through the owned link".to_vec(),
                    executable: false,
                },
            },
        )]),
    };

    let report = apply_at(&dest, &state, &manifest, &plan).expect("the owned link is followed");

    assert_eq!(
        report.outcomes,
        BTreeMap::from([("logs/x.txt".into(), ApplyOutcome::Written)])
    );
    // The walk restarted along the resolved path: the bytes live under the
    // link's target.
    assert_tree(
        dest.root(),
        &Tree::new()
            .file("real/x.txt", "through the owned link")
            .symlink("logs", "real"),
    );
    assert!(
        report
            .manifest
            .entries
            .contains_key(Utf8Path::new("logs/x.txt"))
    );
}

// A symlink is the one entry that arm is not allowed to relocate, because
// settling's wait-for set names what the run will still publish by action
// key. Without this refusal the plan below escapes: `a` is graded and
// published while nothing stands at `real/x`, then `pivot/x` goes down at
// `real/x` — a path no chain waited for, since the set names `pivot/x` —
// and `dest/a` resolves to the destination's grandparent.
#[test]
fn a_link_the_walk_would_relocate_through_an_owned_link_refuses() {
    let (dest, state) = fixtures();
    Tree::new()
        .dir("real")
        .symlink("pivot", "real")
        .write_under(dest.root());
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "pivot".into(),
        recorded(EntryKind::Symlink, sha256_hex(b"real"), &["own"]),
    );
    let plan = Plan {
        owner: "own".to_owned(),
        external_targets: ExternalTargetPolicy::Refuse,
        actions: BTreeMap::from([
            (
                "a".into(),
                Action::Write {
                    entry: Entry::Symlink {
                        target: "real/x/../../escape".to_owned(),
                    },
                },
            ),
            (
                "pivot/x".into(),
                Action::Write {
                    entry: Entry::Symlink {
                        target: "..".to_owned(),
                    },
                },
            ),
        ]),
    };

    let error = apply_at(&dest, &state, &manifest, &plan)
        .expect_err("a link is never published off its key");

    match error {
        Error::Containment { paths } => assert_eq!(paths, BTreeSet::from(["pivot/x".into()])),
        other => panic!("expected Containment, got {other:?}"),
    }
    // `a` landed — `real/x` is nothing, so it points at `escape` inside the
    // destination — and the link that would have moved it never went down.
    assert_tree(
        dest.root(),
        &Tree::new()
            .dir("real")
            .symlink("pivot", "real")
            .symlink("a", "real/x/../../escape"),
    );
}

#[test]
fn a_removal_through_an_owned_link_prunes_the_resolved_directory() {
    let (dest, state) = fixtures();
    Tree::new()
        .file("real/x.txt", "bytes")
        .symlink("logs", "real")
        .write_under(dest.root());
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "logs".into(),
        recorded(EntryKind::Symlink, sha256_hex(b"real"), &["own"]),
    );
    manifest.entries.insert(
        "logs/x.txt".into(),
        recorded(EntryKind::File, sha256_hex(b"bytes"), &["own"]),
    );
    let plan = Plan {
        owner: "own".to_owned(),
        external_targets: ExternalTargetPolicy::Refuse,
        actions: BTreeMap::from([(
            "logs/x.txt".into(),
            Action::Remove {
                expected: Some(NodeSignature {
                    kind: EntryKind::File,
                    hash: sha256_hex(b"bytes"),
                    executable: false,
                }),
            },
        )]),
    };

    let report = apply_at(&dest, &state, &manifest, &plan).expect("removal through the owned link");

    assert_eq!(
        report.outcomes,
        BTreeMap::from([("logs/x.txt".into(), ApplyOutcome::Removed)])
    );
    // The directory that actually lost the child — the resolved side, not
    // the action key's ancestry — is pruned; the owned link survives (a
    // dangling target is allowed).
    assert_tree(dest.root(), &Tree::new().symlink("logs", "real"));
}

#[test]
fn deciding_cannot_aim_that_removal_because_the_path_observes_absent() {
    // The companion to the test above, and the reason `docs/design.lex`
    // section 2 does not promise the pipeline cleans such a path up: the
    // walk observes no path beneath a link, so a path recorded under one
    // classifies Missing and its removal expects nothing — which apply
    // refuses as drift, having found a node there.
    let (dest, state) = fixtures();
    Tree::new()
        .file("real/x.txt", "bytes")
        .symlink("logs", "real")
        .write_under(dest.root());
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "logs".into(),
        recorded(EntryKind::Symlink, sha256_hex(b"real"), &["own"]),
    );
    manifest.entries.insert(
        "logs/x.txt".into(),
        recorded(EntryKind::File, sha256_hex(b"bytes"), &["own"]),
    );
    save_manifest(&dir_at(state.root()), &manifest).expect("seed the manifest");

    let (loaded, plan) = plan_for(&dest, &state, "own", &BTreeMap::new(), DriftPolicy::Refuse);

    assert_eq!(
        plan.actions.get(Utf8Path::new("logs/x.txt")),
        Some(&Action::Remove { expected: None })
    );
    match apply_at(&dest, &state, &loaded, &plan) {
        Err(Error::Drift { paths }) => {
            assert_eq!(paths, BTreeSet::from([Utf8PathBuf::from("logs/x.txt")]));
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn a_recorded_symlink_ancestor_with_a_changed_target_refuses_drift() {
    let (dest, state) = fixtures();
    Tree::new()
        .dir("real")
        .dir("other")
        .symlink("logs", "other")
        .write_under(dest.root());
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "logs".into(),
        recorded(EntryKind::Symlink, sha256_hex(b"real"), &["own"]),
    );
    let plan = Plan {
        owner: "own".to_owned(),
        external_targets: ExternalTargetPolicy::Refuse,
        actions: BTreeMap::from([(
            "logs/x.txt".into(),
            Action::Write {
                entry: Entry::File {
                    contents: b"x".to_vec(),
                    executable: false,
                },
            },
        )]),
    };

    let error = apply_at(&dest, &state, &manifest, &plan).expect_err("a swapped target is drift");

    match error {
        Error::Drift { paths } => assert_eq!(paths, BTreeSet::from(["logs".into()])),
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn a_recorded_symlink_ancestor_with_an_external_target_refuses_containment() {
    let (dest, state) = fixtures();
    Tree::new()
        .symlink("logs", "../outside")
        .write_under(dest.root());
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "logs".into(),
        recorded(EntryKind::Symlink, sha256_hex(b"../outside"), &["own"]),
    );
    let plan = Plan {
        owner: "own".to_owned(),
        external_targets: ExternalTargetPolicy::Refuse,
        actions: BTreeMap::from([(
            "logs/x.txt".into(),
            Action::Write {
                entry: Entry::File {
                    contents: b"x".to_vec(),
                    executable: false,
                },
            },
        )]),
    };

    let error = apply_at(&dest, &state, &manifest, &plan)
        .expect_err("never write through an external target");

    match error {
        Error::Containment { paths } => {
            assert_eq!(paths, BTreeSet::from(["logs/x.txt".into()]))
        }
        other => panic!("expected Containment, got {other:?}"),
    }
}

#[test]
fn an_owned_link_cycle_refuses_instead_of_looping() {
    let (dest, state) = fixtures();
    Tree::new()
        .symlink("l1", "l2")
        .symlink("l2", "l1")
        .write_under(dest.root());
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "l1".into(),
        recorded(EntryKind::Symlink, sha256_hex(b"l2"), &["own"]),
    );
    manifest.entries.insert(
        "l2".into(),
        recorded(EntryKind::Symlink, sha256_hex(b"l1"), &["own"]),
    );
    let plan = Plan {
        owner: "own".to_owned(),
        external_targets: ExternalTargetPolicy::Refuse,
        actions: BTreeMap::from([(
            "l1/x.txt".into(),
            Action::Write {
                entry: Entry::File {
                    contents: b"x".to_vec(),
                    executable: false,
                },
            },
        )]),
    };

    let error = apply_at(&dest, &state, &manifest, &plan).expect_err("cycles refuse");

    match error {
        Error::Containment { paths } => assert_eq!(paths, BTreeSet::from(["l1/x.txt".into()])),
        other => panic!("expected Containment, got {other:?}"),
    }
}

// --- symlinks: creation, replacement, transitions (issue #8) ---

#[test]
fn projects_links_with_their_targets_verbatim_dangling_included() {
    let (dest, state) = fixtures();
    let tree = Tree::new()
        .file("notes/a.txt", "alpha")
        .symlink("latest", "notes/a.txt")
        .symlink("nested/up", "../notes/a.txt")
        .symlink("someday", "notes/not-yet.txt");

    let report = pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse)
        .expect("in-dest targets need no permission");

    // Every target reached disk byte for byte, the dangling one included:
    // a link is a pointer, and nothing resolves it.
    assert_tree(dest.root(), &tree);
    assert_eq!(
        report.outcomes[Utf8Path::new("latest")],
        ApplyOutcome::Written
    );
    let entry = &report.manifest.entries[Utf8Path::new("nested/up")];
    assert_eq!(entry.kind, EntryKind::Symlink);
    assert!(!entry.executable);
    // The manifest hashes the target string, not what it points at.
    assert_eq!(entry.hash, sha256_hex(b"../notes/a.txt"));
}

#[test]
fn a_target_that_is_not_a_pathname_fails_up_front_and_writes_nothing() {
    // Deciding refuses such a target, so this is the hand-built half: the
    // whole-plan check catches it before any action runs, rather than
    // letting the OS reject it partway through the sorted order.
    let (dest, state) = fixtures();
    let plan = Plan {
        owner: "own".to_owned(),
        external_targets: ExternalTargetPolicy::Refuse,
        actions: BTreeMap::from([
            (
                "a.txt".into(),
                Action::Write {
                    entry: Entry::File {
                        contents: b"alpha".to_vec(),
                        executable: false,
                    },
                },
            ),
            (
                "z-link".into(),
                Action::Write {
                    entry: Entry::Symlink {
                        target: String::new(),
                    },
                },
            ),
        ]),
    };

    let error = apply_at(&dest, &state, &Manifest::new(), &plan)
        .expect_err("an empty target is not a path");

    match error {
        Error::InvalidTarget { links } => assert_eq!(
            links,
            BTreeMap::from([(Utf8PathBuf::from("z-link"), String::new())])
        ),
        other => panic!("expected InvalidTarget, got {other:?}"),
    }
    // Nothing ran, the file sorted before the link included.
    assert_tree(dest.root(), &Tree::new());
}

#[test]
fn a_changed_link_target_is_replaced_in_place() {
    let (dest, state) = fixtures();
    let v1 = Tree::new()
        .file("v1.txt", "one")
        .symlink("current", "v1.txt");
    pipeline(&dest, &state, "own", &v1.entries(), DriftPolicy::Refuse).expect("project v1");

    let v2 = Tree::new()
        .file("v1.txt", "one")
        .symlink("current", "v2.txt");
    let report =
        pipeline(&dest, &state, "own", &v2.entries(), DriftPolicy::Refuse).expect("project v2");

    assert_eq!(
        report.outcomes[Utf8Path::new("current")],
        ApplyOutcome::Overwritten
    );
    assert_tree(dest.root(), &v2);
    assert_eq!(
        report.manifest.entries[Utf8Path::new("current")].hash,
        sha256_hex(b"v2.txt")
    );
}

// Definition of done: both transitions, under the ordinary drift rules —
// the rename that publishes a node replaces whatever the leaf held.
#[test]
fn a_file_becomes_a_link_and_a_link_becomes_a_file() {
    let (dest, state) = fixtures();
    let files = Tree::new()
        .file("here", "bytes")
        .file("target", "pointed at");
    pipeline(&dest, &state, "own", &files.entries(), DriftPolicy::Refuse).expect("project files");

    // file → link
    let linked = Tree::new()
        .symlink("here", "target")
        .file("target", "pointed at");
    let report = pipeline(&dest, &state, "own", &linked.entries(), DriftPolicy::Refuse)
        .expect("file → link");
    assert_eq!(
        report.outcomes[Utf8Path::new("here")],
        ApplyOutcome::Overwritten
    );
    assert_tree(dest.root(), &linked);
    assert_eq!(
        report.manifest.entries[Utf8Path::new("here")].kind,
        EntryKind::Symlink
    );

    // link → file, and the link's former target keeps its bytes
    let report =
        pipeline(&dest, &state, "own", &files.entries(), DriftPolicy::Refuse).expect("link → file");
    assert_eq!(
        report.outcomes[Utf8Path::new("here")],
        ApplyOutcome::Overwritten
    );
    assert_tree(dest.root(), &files);
    assert_eq!(
        report.manifest.entries[Utf8Path::new("here")].kind,
        EntryKind::File
    );
}

#[test]
fn a_link_edited_on_disk_refuses_as_drift_and_force_replaces_it() {
    let (dest, state) = fixtures();
    let v1 = Tree::new().symlink("current", "v1.txt");
    pipeline(&dest, &state, "own", &v1.entries(), DriftPolicy::Refuse).expect("project v1");
    Tree::new()
        .symlink("current", "elsewhere.txt")
        .write_under(dest.root());

    let v2 = Tree::new().symlink("current", "v2.txt");
    let error = pipeline(&dest, &state, "own", &v2.entries(), DriftPolicy::Refuse)
        .expect_err("an edited target is a user edit like any other");
    match error {
        Error::Drift { paths } => assert_eq!(paths, BTreeSet::from(["current".into()])),
        other => panic!("expected Drift, got {other:?}"),
    }
    assert_tree(
        dest.root(),
        &Tree::new().symlink("current", "elsewhere.txt"),
    );

    pipeline(&dest, &state, "own", &v2.entries(), DriftPolicy::Overwrite).expect("--force");
    assert_tree(dest.root(), &v2);
}

// Definition of done: an external target refuses with the path unless the
// caller permits it, and lands byte-exact when it does.
#[test]
fn external_targets_refuse_without_the_policy_and_land_verbatim_with_it() {
    let (dest, state) = fixtures();
    let tree = Tree::new()
        .symlink("escape", "../outside")
        .symlink("absolute", "/etc/hosts")
        .file("kept.txt", "unrelated");

    let error = pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse)
        .expect_err("external targets are opt-in");

    match error {
        Error::ExternalTarget { links } => assert_eq!(
            links,
            BTreeMap::from([
                (Utf8PathBuf::from("absolute"), "/etc/hosts".to_owned()),
                (Utf8PathBuf::from("escape"), "../outside".to_owned()),
            ])
        ),
        other => panic!("expected ExternalTarget, got {other:?}"),
    }
    // Refused up front: the unrelated half of the plan did not run either.
    assert_tree(dest.root(), &Tree::new());

    let report = pipeline_with(
        &dest,
        &state,
        "own",
        &tree.entries(),
        PlanOptions {
            external_targets: ExternalTargetPolicy::Allow,
            ..PlanOptions::default()
        },
    )
    .expect("permitted external targets land");

    // Byte-exact, both of them: proiectio never rewrites a target.
    assert_tree(dest.root(), &tree);
    assert_eq!(
        report.manifest.entries[Utf8Path::new("absolute")].hash,
        sha256_hex(b"/etc/hosts")
    );
}

#[test]
fn nothing_is_written_through_a_permitted_external_link() {
    let (dest, state) = fixtures();
    let allowing = PlanOptions {
        external_targets: ExternalTargetPolicy::Allow,
        ..PlanOptions::default()
    };
    let link = Tree::new().symlink("out", "../outside");
    pipeline_with(&dest, &state, "own", &link.entries(), allowing).expect("plant the pointer");

    // A tree naming a path beneath the link is refused, permitted target or
    // not: the pointer stays a pointer. (The scaffolding will not declare a
    // node under a link either, so the desired map is built by hand.)
    let mut desired = link.entries();
    desired.insert(
        "out/x.txt".into(),
        Entry::File {
            contents: b"smuggled".to_vec(),
            executable: false,
        },
    );
    let error = pipeline_with(&dest, &state, "own", &desired, allowing)
        .expect_err("no write through an external target");

    match error {
        Error::TreeConflict { paths } => {
            assert_eq!(paths, BTreeSet::from(["out".into(), "out/x.txt".into()]))
        }
        other => panic!("expected TreeConflict, got {other:?}"),
    }
    assert_tree(dest.root(), &link);
}

// Definition of done (issue #29): a target reaching outside through a link
// the destination already holds needs the permission like any other
// external target.
#[test]
fn a_target_escaping_through_a_pivot_link_refuses_without_the_permission() {
    let (dest, state) = fixtures();
    let pivot = Tree::new().symlink("pivot", "/etc");
    pivot.write_under(dest.root());
    let desired = Tree::new().symlink("evil", "pivot/passwd").entries();

    let error = pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse)
        .expect_err("a pointer through a pivot reaches /etc/passwd");

    match error {
        Error::ExternalTarget { links } => assert_eq!(
            links,
            BTreeMap::from([(Utf8PathBuf::from("evil"), "pivot/passwd".to_owned())])
        ),
        other => panic!("expected ExternalTarget, got {other:?}"),
    }
    assert_tree(dest.root(), &pivot);

    // With the permission the target lands verbatim — and the apply-time
    // re-grade stands down, since a caller who permitted external targets
    // permitted them whatever the destination holds.
    let landed = Tree::new()
        .symlink("pivot", "/etc")
        .symlink("evil", "pivot/passwd");
    pipeline_with(
        &dest,
        &state,
        "own",
        &desired,
        PlanOptions {
            external_targets: ExternalTargetPolicy::Allow,
            ..PlanOptions::default()
        },
    )
    .expect("permitted targets land whatever they chain through");

    assert_tree(dest.root(), &landed);
}

#[test]
fn an_ordinary_in_dest_chain_lands_without_the_permission() {
    let (dest, state) = fixtures();
    Tree::new()
        .dir("real")
        .symlink("shared", "real")
        .write_under(dest.root());
    let desired = Tree::new().symlink("rc", "shared/rc").entries();

    pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse)
        .expect("an in-dest chain is an in-dest target");

    assert_tree(
        dest.root(),
        &Tree::new()
            .dir("real")
            .symlink("shared", "real")
            .symlink("rc", "shared/rc"),
    );
}

// The changed-since-plan re-check for a link's target: the plan graded the
// pointer against a pivot that pointed inside, and the pivot moved before
// apply reached the link.
#[test]
fn a_pivot_swapped_after_the_plan_refuses_instead_of_publishing_the_pointer() {
    let (dest, state) = fixtures();
    Tree::new()
        .dir("real")
        .symlink("pivot", "real")
        .write_under(dest.root());
    let desired = Tree::new().symlink("rc", "pivot/rc").entries();

    let (manifest, plan) = plan_for(&dest, &state, "own", &desired, DriftPolicy::Refuse);
    assert!(
        matches!(plan.actions[Utf8Path::new("rc")], Action::Write { .. }),
        "the plan graded the target in-dest"
    );

    // The gap between the two calls: the pivot now points outside.
    Tree::new()
        .symlink("pivot", "/etc")
        .write_under(dest.root());
    let error = apply_at(&dest, &state, &manifest, &plan)
        .expect_err("the plan-time verdict no longer holds");

    match error {
        Error::ExternalTarget { links } => assert_eq!(
            links,
            BTreeMap::from([(Utf8PathBuf::from("rc"), "pivot/rc".to_owned())])
        ),
        other => panic!("expected ExternalTarget, got {other:?}"),
    }
    assert_tree(
        dest.root(),
        &Tree::new().dir("real").symlink("pivot", "/etc"),
    );
}

// Two links a tree projects together are a chain like any other: read one
// at a time both land in-dest, and together they point at the destination's
// parent, because "b/.." pops the directory "b" resolved to.
#[test]
fn a_pointer_escaping_through_a_link_the_same_tree_projects_refuses() {
    let (dest, state) = fixtures();
    let desired = Tree::new()
        .symlink("a", "b/../escape")
        .symlink("b", ".")
        .entries();

    let error = pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse)
        .expect_err("the pair dereferences outside the destination");

    match error {
        Error::ExternalTarget { links } => assert_eq!(
            links,
            BTreeMap::from([(Utf8PathBuf::from("a"), "b/../escape".to_owned())])
        ),
        other => panic!("expected ExternalTarget, got {other:?}"),
    }
    // A refusal in the plan applies nothing, so neither link is on disk.
    assert_tree(dest.root(), &Tree::new());
}

// The other side of grading against the destination the run leaves: a
// pointer through a pivot this run replaces is graded as the link the pivot
// becomes. Apply publishes "evil" before it reaches "pivot", so re-grading
// against the half-written destination alone would refuse a run whose
// finished destination holds nothing external.
#[test]
fn a_pointer_through_a_pivot_this_run_replaces_lands_without_the_permission() {
    let (dest, state) = fixtures();
    pipeline_with(
        &dest,
        &state,
        "own",
        &Tree::new().symlink("pivot", "/etc").entries(),
        PlanOptions {
            external_targets: ExternalTargetPolicy::Allow,
            ..PlanOptions::default()
        },
    )
    .expect("the permitted external pivot lands");

    let desired = Tree::new()
        .symlink("pivot", "real")
        .symlink("evil", "pivot/x")
        .entries();

    pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse)
        .expect("the run replaces the pivot with an in-dest link");

    assert_tree(
        dest.root(),
        &Tree::new()
            .symlink("pivot", "real")
            .symlink("evil", "pivot/x"),
    );
}

// The plan answers for its own ancestors too. "d" is a link on disk and a
// file in the tree, so nothing lives beneath it once the run finishes and
// the chain ends there — which the re-grade has to read off the plan, since
// apply reaches "a" before it has replaced "d".
#[test]
fn a_pointer_under_a_link_this_run_replaces_with_a_file_lands_without_the_permission() {
    let (dest, state) = fixtures();
    pipeline_with(
        &dest,
        &state,
        "own",
        &Tree::new().symlink("d", "/etc").entries(),
        PlanOptions {
            external_targets: ExternalTargetPolicy::Allow,
            ..PlanOptions::default()
        },
    )
    .expect("the permitted external link lands");

    let desired = Tree::new()
        .file("d", "no longer a link\n")
        .symlink("a", "d/x")
        .entries();

    pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse)
        .expect("the run replaces the link with a file");

    assert_tree(
        dest.root(),
        &Tree::new()
            .file("d", "no longer a link\n")
            .symlink("a", "d/x"),
    );
}

// A run that fails partway leaves no pointer out of the destination
// behind: "evil" waits for the pivot it resolves through, the pivot's
// overwrite refuses, and the pointer is never published.
#[test]
fn a_held_link_is_not_published_when_the_pivot_it_waits_for_refuses() {
    let (dest, state) = fixtures();
    pipeline_with(
        &dest,
        &state,
        "own",
        &Tree::new().symlink("pivot", "/etc").entries(),
        PlanOptions {
            external_targets: ExternalTargetPolicy::Allow,
            ..PlanOptions::default()
        },
    )
    .expect("the permitted external pivot lands");

    let desired = Tree::new()
        .symlink("pivot", "real")
        .symlink("evil", "pivot/x")
        .entries();
    let (manifest, plan) = plan_for(&dest, &state, "own", &desired, DriftPolicy::Refuse);

    // The gap between the two calls: the pivot is edited, so its overwrite
    // will refuse the signature the plan expects.
    Tree::new()
        .symlink("pivot", "/tmp")
        .write_under(dest.root());
    let error = apply_at(&dest, &state, &manifest, &plan).expect_err("the pivot drifted");

    match error {
        Error::Drift { paths } => assert_eq!(paths, BTreeSet::from([Utf8PathBuf::from("pivot")])),
        other => panic!("expected Drift, got {other:?}"),
    }
    assert_tree(dest.root(), &Tree::new().symlink("pivot", "/tmp"));
}

// The destination for the two tests below: "dir" is an ordinary directory,
// and the run owns "b -> dir" and "c/c -> .." from an earlier projection.
fn pivot_chain_fixtures() -> (Fixture, Fixture) {
    let (dest, state) = fixtures();
    Tree::new().dir("dir").write_under(dest.root());
    pipeline(
        &dest,
        &state,
        "own",
        &Tree::new()
            .symlink("b", "dir")
            .symlink("c/c", "..")
            .entries(),
        DriftPolicy::Refuse,
    )
    .expect("both links land in dest");
    (dest, state)
}

// Grading a link in-dest at the moment it is published is not enough:
// publishing a later link can move where an earlier one lands. Here "a"
// grades in-dest through the old "b -> dir", and republishing "b" at "c/c"
// — still pointing at the destination root — would put "a" outside without
// either grading ever saying so. So a link also waits for every path its own
// resolution walked through that the run is still going to publish at.
#[test]
fn a_link_waits_for_every_link_the_run_will_still_publish_under_it() {
    let (dest, state) = pivot_chain_fixtures();
    let desired = Tree::new()
        .symlink("a", "b/../escape")
        .symlink("b", "c/c")
        .symlink("c/c", "../dir")
        .entries();

    pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse)
        .expect("the finished destination holds nothing external");

    assert_tree(
        dest.root(),
        &Tree::new()
            .dir("dir")
            .symlink("a", "b/../escape")
            .symlink("b", "c/c")
            .symlink("c/c", "../dir"),
    );
}

#[test]
fn a_link_waiting_on_a_pivot_that_refuses_is_never_published() {
    let (dest, state) = pivot_chain_fixtures();
    let desired = Tree::new()
        .symlink("a", "b/../escape")
        .symlink("b", "c/c")
        .symlink("c/c", "../dir")
        .entries();
    let (manifest, plan) = plan_for(&dest, &state, "own", &desired, DriftPolicy::Refuse);

    // The gap between the two calls: the pivot at the bottom of the chain
    // is edited, so its overwrite refuses the signature the plan expects.
    Tree::new()
        .symlink("c/c", "../elsewhere")
        .write_under(dest.root());
    let error = apply_at(&dest, &state, &manifest, &plan).expect_err("the deepest pivot drifted");

    match error {
        Error::Drift { paths } => assert_eq!(paths, BTreeSet::from([Utf8PathBuf::from("c/c")])),
        other => panic!("expected Drift, got {other:?}"),
    }
    // Neither link above it moved, so nothing points out of the destination.
    assert_tree(
        dest.root(),
        &Tree::new()
            .dir("dir")
            .symlink("b", "dir")
            .symlink("c/c", "../elsewhere"),
    );
}

// A link the run leaves in place is held to its plan-time verdict too: the
// run publishes nothing for a skip, but the pivot under it moved, and a
// fresh plan over the same disk would refuse.
#[test]
fn a_skipped_link_whose_pivot_was_swapped_refuses() {
    let (dest, state) = fixtures();
    Tree::new()
        .dir("real")
        .symlink("pivot", "real")
        .write_under(dest.root());
    let desired = Tree::new().symlink("rc", "pivot/x").entries();

    pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse)
        .expect("the chain lands in dest, so the link is written");

    // The same tree again: "rc" is clean, so the plan skips it.
    let (manifest, plan) = plan_for(&dest, &state, "own", &desired, DriftPolicy::Refuse);
    assert!(
        matches!(plan.actions[Utf8Path::new("rc")], Action::Skip { .. }),
        "an unchanged link is skipped"
    );

    // The gap between the two calls: the pivot now points outside.
    Tree::new()
        .symlink("pivot", "/etc")
        .write_under(dest.root());
    let error = apply_at(&dest, &state, &manifest, &plan)
        .expect_err("the skipped link now resolves outside the destination");

    match error {
        Error::ExternalTarget { links } => assert_eq!(
            links,
            BTreeMap::from([(Utf8PathBuf::from("rc"), "pivot/x".to_owned())])
        ),
        other => panic!("expected ExternalTarget, got {other:?}"),
    }
}

// The no-alias rule end to end: a path resolving through a link the plan
// leaves standing is refused, so nothing lands where the plan does not name.
#[test]
fn a_path_beneath_another_owners_link_refuses_containment() {
    let (dest, state) = fixtures();
    let linked = Tree::new().dir("real").symlink("logs", "real");
    linked.write_under(dest.root());
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "logs".into(),
        recorded(EntryKind::Symlink, sha256_hex(b"real"), &["other"]),
    );
    save_manifest(&dir_at(state.root()), &manifest).expect("seed the state dir");

    let desired = BTreeMap::from([(
        Utf8PathBuf::from("logs/x.txt"),
        Entry::File {
            contents: b"aliased".to_vec(),
            executable: false,
        },
    )]);
    let error = pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse)
        .expect_err("a projected path never resolves through a link");

    match error {
        Error::Containment { paths } => assert_eq!(paths, BTreeSet::from(["logs/x.txt".into()])),
        other => panic!("expected Containment, got {other:?}"),
    }
    assert_tree(dest.root(), &linked);
}

#[test]
fn a_path_beneath_a_link_this_run_removes_is_written_as_an_ordinary_path() {
    let (dest, state) = fixtures();
    Tree::new()
        .dir("real")
        .symlink("logs", "real")
        .write_under(dest.root());
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "logs".into(),
        recorded(EntryKind::Symlink, sha256_hex(b"real"), &["own"]),
    );
    save_manifest(&dir_at(state.root()), &manifest).expect("seed the state dir");

    // The desired tree drops the link and names a path under its name: act
    // removes the link first, so the write lands in a real directory.
    let replaced = Tree::new().dir("real").file("logs/x.txt", "a real file");
    let report = pipeline(
        &dest,
        &state,
        "own",
        &replaced.entries(),
        DriftPolicy::Refuse,
    )
    .expect("the removal clears the way for the write");

    assert_eq!(
        report.outcomes,
        BTreeMap::from([
            ("logs".into(), ApplyOutcome::Removed),
            ("logs/x.txt".into(), ApplyOutcome::Written),
        ])
    );
    assert_tree(dest.root(), &replaced);
}

// --- blocks: splicing a region into somebody else's file ---

/// The marker every block test uses.
const MARKER: &str = "# proiectio";

fn block_kind(placement: Placement) -> EntryKind {
    EntryKind::Block {
        marker: MARKER.to_owned(),
        placement,
    }
}

/// A desired block entry under [`MARKER`].
fn block(body: &str, placement: Placement) -> Entry {
    Entry::Block {
        body: body.as_bytes().to_vec(),
        marker: MARKER.to_owned(),
        placement,
    }
}

/// A desired tree of one block at `path`.
fn block_tree(path: &str, body: &str, placement: Placement) -> BTreeMap<Utf8PathBuf, Entry> {
    BTreeMap::from([(Utf8PathBuf::from(path), block(body, placement))])
}

/// The container's bytes as they stand on disk.
fn container(dest: &Fixture, path: &str) -> String {
    fs::read_to_string(dest.path(path)).expect("read the container")
}

#[test]
fn a_block_splices_a_region_into_a_container_it_does_not_own() {
    let (dest, state) = fixtures();
    Tree::new()
        .file("rc", "author line\nsecond\n")
        .write_under(dest.root());

    let report = pipeline(
        &dest,
        &state,
        "own",
        &block_tree("rc", "managed\n", Placement::Append),
        DriftPolicy::Refuse,
    )
    .expect("the container is unrecorded, which is what a block is for");

    assert_eq!(
        report.outcomes,
        BTreeMap::from([("rc".into(), ApplyOutcome::Written)])
    );
    assert_eq!(
        container(&dest, "rc"),
        "author line\nsecond\n# proiectio\nmanaged\n"
    );
    // The manifest records the region's body, never the container's bytes.
    assert_eq!(
        report.manifest.entries[Utf8Path::new("rc")],
        recorded(
            block_kind(Placement::Append),
            sha256_hex(b"managed\n"),
            &["own"]
        )
    );
}

#[test]
fn prepending_puts_the_region_before_the_author() {
    let (dest, state) = fixtures();
    Tree::new()
        .file("rc", "author line\n")
        .write_under(dest.root());

    pipeline(
        &dest,
        &state,
        "own",
        &block_tree("rc", "managed\n", Placement::Prepend),
        DriftPolicy::Refuse,
    )
    .expect("project");

    assert_eq!(
        container(&dest, "rc"),
        "managed\n# proiectio\nauthor line\n"
    );
}

#[test]
fn re_applying_a_block_is_a_no_op() {
    let (dest, state) = fixtures();
    Tree::new().file("rc", "author\n").write_under(dest.root());
    let desired = block_tree("rc", "managed\n", Placement::Append);
    pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse).expect("project");
    let after_first = container(&dest, "rc");

    let report =
        pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse).expect("re-apply is a no-op");

    assert_eq!(
        report.outcomes,
        BTreeMap::from([("rc".into(), ApplyOutcome::Skipped)])
    );
    // The marker is what makes this idempotent: without one the second run
    // could not tell its own bytes from the author's, and would append again.
    assert_eq!(container(&dest, "rc"), after_first);
}

#[test]
fn an_edit_outside_the_region_is_not_drift_and_an_edit_inside_it_is() {
    let (dest, state) = fixtures();
    Tree::new().file("rc", "author\n").write_under(dest.root());
    let desired = block_tree("rc", "managed\n", Placement::Append);
    pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse).expect("project");

    // The author edits their own side and re-runs: nothing to do.
    fs::write(dest.path("rc"), "author\nand more\n# proiectio\nmanaged\n").expect("edit outside");
    let report = pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse)
        .expect("outside is not drift");
    assert_eq!(
        report.outcomes,
        BTreeMap::from([("rc".into(), ApplyOutcome::Skipped)])
    );
    assert_eq!(
        container(&dest, "rc"),
        "author\nand more\n# proiectio\nmanaged\n"
    );

    // The author edits inside the region: that is drift, and it refuses.
    fs::write(dest.path("rc"), "author\nand more\n# proiectio\nedited\n").expect("edit inside");
    let error = pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse)
        .expect_err("inside the region is drift");
    match error {
        Error::Drift { paths } => assert_eq!(paths, BTreeSet::from(["rc".into()])),
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn an_edit_past_the_regions_outer_edge_is_drift() {
    // The region runs to the file's edge, so an author appending past an
    // appended region has written inside it. This is the cost `Prepend`
    // avoids, and it is why `EntryKind::Block` says so.
    let (dest, state) = fixtures();
    Tree::new().file("rc", "author\n").write_under(dest.root());
    let desired = block_tree("rc", "managed\n", Placement::Append);
    pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse).expect("project");
    fs::write(
        dest.path("rc"),
        "author\n# proiectio\nmanaged\ntheir new line\n",
    )
    .expect("append past the region");

    let error = pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse)
        .expect_err("past the edge is inside the region");

    match error {
        Error::Drift { paths } => assert_eq!(paths, BTreeSet::from(["rc".into()])),
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn changing_the_body_replaces_only_the_region() {
    let (dest, state) = fixtures();
    Tree::new()
        .file("rc", "author\nkeep me\n")
        .write_under(dest.root());
    pipeline(
        &dest,
        &state,
        "own",
        &block_tree("rc", "v1\n", Placement::Append),
        DriftPolicy::Refuse,
    )
    .expect("project v1");

    let report = pipeline(
        &dest,
        &state,
        "own",
        &block_tree("rc", "v2\n", Placement::Append),
        DriftPolicy::Refuse,
    )
    .expect("project v2");

    assert_eq!(
        report.outcomes,
        BTreeMap::from([("rc".into(), ApplyOutcome::Overwritten)])
    );
    assert_eq!(container(&dest, "rc"), "author\nkeep me\n# proiectio\nv2\n");
}

#[test]
fn a_changed_marker_migrates_the_region_in_one_publish() {
    let (dest, state) = fixtures();
    Tree::new().file("rc", "author\n").write_under(dest.root());
    pipeline(
        &dest,
        &state,
        "own",
        &block_tree("rc", "managed\n", Placement::Append),
        DriftPolicy::Refuse,
    )
    .expect("project under the first marker");

    let renamed = BTreeMap::from([(
        Utf8PathBuf::from("rc"),
        Entry::Block {
            body: b"managed\n".to_vec(),
            marker: "# renamed".to_owned(),
            placement: Placement::Append,
        },
    )]);
    let report =
        pipeline(&dest, &state, "own", &renamed, DriftPolicy::Refuse).expect("migrate the marker");

    assert_eq!(
        report.outcomes,
        BTreeMap::from([("rc".into(), ApplyOutcome::Overwritten)])
    );
    // One publish: the old region is gone, not left beside the new one.
    assert_eq!(container(&dest, "rc"), "author\n# renamed\nmanaged\n");
}

#[test]
fn removing_a_block_leaves_the_container_byte_identical_apart_from_the_region() {
    let (dest, state) = fixtures();
    let author = "author\nkeep me\n";
    Tree::new().file("rc", author).write_under(dest.root());
    for placement in [Placement::Append, Placement::Prepend] {
        pipeline(
            &dest,
            &state,
            "own",
            &block_tree("rc", "managed\n", placement),
            DriftPolicy::Refuse,
        )
        .expect("project");

        let (manifest, plan) = {
            let dest_dir = dir_at(dest.root());
            let state_dir = dir_at(state.root());
            let manifest = load_manifest(&state_dir).expect("load manifest");
            let observations = observe(&dest_dir, &manifest).expect("observe");
            let plan = decide_removal(
                "own",
                RemovalScope::Everything,
                &manifest,
                &observations,
                None,
                PlanOptions::default(),
            );
            (manifest, plan)
        };
        let report = apply_at(&dest, &state, &manifest, &plan).expect("strip the region");

        assert_eq!(
            report.outcomes,
            BTreeMap::from([("rc".into(), ApplyOutcome::Removed)])
        );
        // The container stays, and every byte outside the region is where it
        // was: a block never deletes a file it does not own whole.
        assert_eq!(container(&dest, "rc"), author, "{placement:?}");
        assert_eq!(report.manifest, Manifest::new());
    }
}

#[test]
fn a_region_whose_body_already_matches_is_adopted() {
    // Refusing a destination that is already in the desired state would be
    // the alternative; the region is recorded and nothing is written.
    let (dest, state) = fixtures();
    Tree::new()
        .file("rc", "author\n# proiectio\nmanaged\n")
        .write_under(dest.root());

    let report = pipeline(
        &dest,
        &state,
        "own",
        &block_tree("rc", "managed\n", Placement::Append),
        DriftPolicy::Refuse,
    )
    .expect("adopt the region already there");

    assert_eq!(
        report.outcomes,
        BTreeMap::from([("rc".into(), ApplyOutcome::Skipped)])
    );
    assert_eq!(container(&dest, "rc"), "author\n# proiectio\nmanaged\n");
    assert_eq!(
        report.manifest.entries[Utf8Path::new("rc")],
        recorded(
            block_kind(Placement::Append),
            sha256_hex(b"managed\n"),
            &["own"]
        )
    );
}

#[test]
fn an_ambiguous_container_is_not_adopted() {
    // Two whole-line marker occurrences identify no region, so the extreme
    // one's body matching what this run would write is not evidence the
    // projection wrote it. Adopting would record a region nothing in the
    // manifest can locate again, and every later run over that path refuses.
    let (dest, state) = fixtures();
    let author = "# proiectio\nmanaged\n# proiectio\nmanaged\n";
    Tree::new().file("rc", author).write_under(dest.root());

    let error = pipeline(
        &dest,
        &state,
        "own",
        &block_tree("rc", "managed\n", Placement::Append),
        DriftPolicy::Refuse,
    )
    .expect_err("nothing says which occurrence bounds a region");

    match error {
        Error::Foreign { paths } => assert_eq!(paths, BTreeSet::from(["rc".into()])),
        other => panic!("expected Foreign, got {other:?}"),
    }
    assert_eq!(container(&dest, "rc"), author);
    assert_eq!(persisted(&state), Manifest::new());
}

#[test]
fn an_unrecorded_region_carrying_other_bytes_refuses_as_foreign() {
    let (dest, state) = fixtures();
    Tree::new()
        .file("rc", "author\n# proiectio\nsomebody else wrote this\n")
        .write_under(dest.root());

    let error = pipeline(
        &dest,
        &state,
        "own",
        &block_tree("rc", "managed\n", Placement::Append),
        DriftPolicy::Refuse,
    )
    .expect_err("the region is on disk and unrecorded");

    match error {
        Error::Foreign { paths } => assert_eq!(paths, BTreeSet::from(["rc".into()])),
        other => panic!("expected Foreign, got {other:?}"),
    }
    assert_eq!(
        container(&dest, "rc"),
        "author\n# proiectio\nsomebody else wrote this\n"
    );
}

#[test]
fn a_block_never_creates_its_container_or_a_directory_for_one() {
    let (dest, state) = fixtures();

    let error = pipeline(
        &dest,
        &state,
        "own",
        &block_tree("etc/rc", "managed\n", Placement::Append),
        DriftPolicy::Refuse,
    )
    .expect_err("there is nothing to splice into");

    match error {
        Error::Block { blocks } => assert_eq!(
            blocks,
            BTreeMap::from([("etc/rc".into(), BlockFault::ContainerMissing)])
        ),
        other => panic!("expected Block, got {other:?}"),
    }
    // No stranded directory either: the walk runs with `create = false`.
    assert_tree(dest.root(), &Tree::new());
}

#[test]
fn appending_to_a_container_with_no_final_newline_refuses() {
    // Ansible appends a newline to the author's last line to make room, which
    // edits a byte outside the block and makes insert-then-strip lose it.
    let (dest, state) = fixtures();
    Tree::new()
        .file("rc", "no newline")
        .write_under(dest.root());

    let error = pipeline(
        &dest,
        &state,
        "own",
        &block_tree("rc", "managed\n", Placement::Append),
        DriftPolicy::Refuse,
    )
    .expect_err("neither side's bytes get normalized");

    match error {
        Error::Block { blocks } => assert_eq!(
            blocks,
            BTreeMap::from([("rc".into(), BlockFault::ContainerNotNewlineTerminated)])
        ),
        other => panic!("expected Block, got {other:?}"),
    }
    assert_eq!(container(&dest, "rc"), "no newline");
}

#[test]
fn a_symlink_at_the_container_path_is_foreign_unrecorded_and_drift_recorded() {
    let (dest, state) = fixtures();
    Tree::new()
        .file("real", "author\n")
        .symlink("rc", "real")
        .write_under(dest.root());
    let desired = block_tree("rc", "managed\n", Placement::Append);

    // Unrecorded: the projection never wrote the link, so it is foreign.
    let unrecorded = pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse)
        .expect_err("a link is not a container");
    match unrecorded {
        Error::Foreign { paths } => assert!(paths.contains(Utf8Path::new("rc"))),
        other => panic!("expected Foreign, got {other:?}"),
    }

    // Recorded as a region, and swapped for a link since: drift of kind,
    // which `--force` does not lift because no signature expresses it.
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "rc".into(),
        recorded(
            block_kind(Placement::Append),
            sha256_hex(b"managed\n"),
            &["own"],
        ),
    );
    save_manifest(&dir_at(state.root()), &manifest).expect("record the region");
    let recorded_error = pipeline(&dest, &state, "own", &desired, DriftPolicy::Overwrite)
        .expect_err("a swapped container is drift");
    match recorded_error {
        Error::Drift { paths } => assert_eq!(paths, BTreeSet::from(["rc".into()])),
        other => panic!("expected Drift, got {other:?}"),
    }
    // Nothing was written through the link.
    assert_eq!(container(&dest, "real"), "author\n");
}

#[test]
fn force_over_a_container_that_became_a_directory_still_refuses() {
    let (dest, state) = fixtures();
    Tree::new().file("rc", "author\n").write_under(dest.root());
    let desired = block_tree("rc", "managed\n", Placement::Append);
    pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse).expect("project");
    fs::remove_file(dest.path("rc")).expect("remove the container");
    fs::create_dir(dest.path("rc")).expect("put a directory there");

    let error = pipeline(&dest, &state, "own", &desired, DriftPolicy::Overwrite)
        .expect_err("no signature expresses a directory");

    match error {
        Error::Drift { paths } => assert_eq!(paths, BTreeSet::from(["rc".into()])),
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn force_overwrites_an_edited_region_and_leaves_the_author_alone() {
    let (dest, state) = fixtures();
    Tree::new().file("rc", "author\n").write_under(dest.root());
    pipeline(
        &dest,
        &state,
        "own",
        &block_tree("rc", "v1\n", Placement::Append),
        DriftPolicy::Refuse,
    )
    .expect("project v1");
    fs::write(dest.path("rc"), "author edited\n# proiectio\nedited\n").expect("drift both sides");

    let report = pipeline(
        &dest,
        &state,
        "own",
        &block_tree("rc", "v2\n", Placement::Append),
        DriftPolicy::Overwrite,
    )
    .expect("--force lifts a drifted region");

    assert_eq!(
        report.outcomes,
        BTreeMap::from([("rc".into(), ApplyOutcome::Overwritten)])
    );
    // The author's edit outside the region survives; theirs inside it does
    // not, which is what the placement tradeoff is about.
    assert_eq!(container(&dest, "rc"), "author edited\n# proiectio\nv2\n");
}

#[test]
fn a_deleted_region_is_spliced_back_and_a_deleted_container_refuses() {
    let (dest, state) = fixtures();
    Tree::new().file("rc", "author\n").write_under(dest.root());
    let desired = block_tree("rc", "managed\n", Placement::Append);
    pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse).expect("project");

    // The author deletes the region but keeps the file: write heals.
    fs::write(dest.path("rc"), "author\n").expect("delete the region");
    let healed =
        pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse).expect("write heals");
    assert_eq!(
        healed.outcomes,
        BTreeMap::from([("rc".into(), ApplyOutcome::Written)])
    );
    assert_eq!(container(&dest, "rc"), "author\n# proiectio\nmanaged\n");

    // The author deletes the whole file: a block never creates one.
    fs::remove_file(dest.path("rc")).expect("delete the container");
    let error = pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse)
        .expect_err("a block never creates its container");
    match error {
        Error::Block { blocks } => assert_eq!(
            blocks,
            BTreeMap::from([("rc".into(), BlockFault::ContainerMissing)])
        ),
        other => panic!("expected Block, got {other:?}"),
    }
}

#[test]
fn the_containers_mode_is_the_authors_and_survives_the_rename() {
    let (dest, state) = fixtures();
    Tree::new()
        .executable("rc", "#!/bin/sh\n")
        .write_under(dest.root());

    pipeline(
        &dest,
        &state,
        "own",
        &block_tree("rc", "managed\n", Placement::Append),
        DriftPolicy::Refuse,
    )
    .expect("project");

    let mode = fs::symlink_metadata(dest.path("rc"))
        .expect("stat the container")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o755);
    // And the manifest says nothing about it: `executable` is always false
    // for a block.
    assert!(!persisted(&state).entries[Utf8Path::new("rc")].executable);
}

#[test]
fn a_containers_setuid_bits_do_not_survive_the_rename() {
    // Publishing replaces the inode, so the new file belongs to whoever ran
    // the projection. Carrying the author's setuid bit across that would
    // re-create somebody else's privileged file under a new owner — content
    // widening what content may do, which `docs/security.lex` section 1
    // forbids and section 4 already forbids for archive members.
    let (dest, state) = fixtures();
    Tree::new().file("rc", "author\n").write_under(dest.root());
    fs::set_permissions(
        dest.path("rc").as_std_path(),
        fs::Permissions::from_mode(0o4755),
    )
    .expect("plant a setuid container");

    pipeline(
        &dest,
        &state,
        "own",
        &block_tree("rc", "managed\n", Placement::Append),
        DriftPolicy::Refuse,
    )
    .expect("project");

    let mode = fs::symlink_metadata(dest.path("rc"))
        .expect("stat the container")
        .permissions()
        .mode();
    assert_eq!(mode & 0o7000, 0, "setuid, setgid and sticky are dropped");
    assert_eq!(mode & 0o777, 0o755, "the permission bits are the author's");
}

#[test]
fn the_bytes_outside_a_region_are_never_interpreted() {
    // conda substitutes its block for a literal sentinel and expands it back,
    // so an rc file containing that string is corrupted. A byte-range splice
    // has nothing to substitute: the author's side here carries the marker
    // text indented, a would-be placeholder, and a stray CR, and comes back
    // exactly as written.
    let (dest, state) = fixtures();
    let author = "  # proiectio\n__CONDA_REPLACE_ME_123__\n{{ handlebars }}\r\n";
    Tree::new().file("rc", author).write_under(dest.root());
    let desired = block_tree("rc", "managed\n", Placement::Append);

    pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse).expect("project");
    assert_eq!(
        container(&dest, "rc"),
        format!("{author}# proiectio\nmanaged\n")
    );

    let (manifest, plan) = {
        let dest_dir = dir_at(dest.root());
        let state_dir = dir_at(state.root());
        let manifest = load_manifest(&state_dir).expect("load manifest");
        let observations = observe(&dest_dir, &manifest).expect("observe");
        let plan = decide_removal(
            "own",
            RemovalScope::Everything,
            &manifest,
            &observations,
            None,
            PlanOptions::default(),
        );
        (manifest, plan)
    };
    apply_at(&dest, &state, &manifest, &plan).expect("strip the region");

    assert_eq!(container(&dest, "rc"), author);
}

#[test]
fn a_marker_terminated_by_end_of_file_still_reads_and_strips() {
    // The projection writes `\n` after the marker, but a line-ending
    // conversion or a hand edit can leave one at the very end of the file.
    let (dest, state) = fixtures();
    Tree::new()
        .file("rc", "author\n# proiectio")
        .write_under(dest.root());

    let report = pipeline(
        &dest,
        &state,
        "own",
        &block_tree("rc", "", Placement::Append),
        DriftPolicy::Refuse,
    )
    .expect("an empty region already carries the desired body");

    assert_eq!(
        report.outcomes,
        BTreeMap::from([("rc".into(), ApplyOutcome::Skipped)])
    );
}

#[test]
fn two_owners_share_a_region_only_while_agreeing_on_the_marker() {
    let (dest, state) = fixtures();
    Tree::new().file("rc", "author\n").write_under(dest.root());
    let desired = block_tree("rc", "managed\n", Placement::Append);
    pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse).expect("project as own");

    // Same marker, same body: the second owner joins the entry.
    let joined = pipeline(&dest, &state, "other", &desired, DriftPolicy::Refuse)
        .expect("identical entries share a path");
    assert_eq!(
        joined.manifest.entries[Utf8Path::new("rc")].owners,
        BTreeSet::from(["other".to_owned(), "own".to_owned()])
    );

    // A different marker is a different kind, so it conflicts.
    let renamed = BTreeMap::from([(
        Utf8PathBuf::from("rc"),
        Entry::Block {
            body: b"managed\n".to_vec(),
            marker: "# renamed".to_owned(),
            placement: Placement::Append,
        },
    )]);
    let error = pipeline(&dest, &state, "third", &renamed, DriftPolicy::Refuse)
        .expect_err("the owners must agree first");
    match error {
        Error::OwnerConflict { conflicts } => {
            assert_eq!(
                conflicts[Utf8Path::new("rc")],
                BTreeSet::from(["other".to_owned(), "own".to_owned()])
            );
        }
        other => panic!("expected OwnerConflict, got {other:?}"),
    }
}

#[test]
fn a_second_marker_line_past_the_edge_refuses_and_strands_nothing() {
    // The last occurrence is the projection's only while every other one is
    // a line outside the region. A second bare marker line leaves two, and
    // stripping the last would republish the container with the first region
    // still in it and the manifest recording only the new one — a stranded
    // body nothing owns. The second region's body decides nothing: a copy of
    // the recorded one would otherwise read clean and be overwritten in
    // place, which is the same stranding by a quieter route.
    for theirs in ["theirs\n", "managed\n"] {
        let (dest, state) = fixtures();
        Tree::new().file("rc", "author\n").write_under(dest.root());
        let desired = block_tree("rc", "managed\n", Placement::Append);
        pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse).expect("project");
        let edited = format!("author\n# proiectio\nmanaged\n# proiectio\n{theirs}");
        fs::write(dest.path("rc"), &edited).expect("append a second marker line past the edge");

        // Nothing has a region it can name: not the ordinary overwrite, not
        // the forced one, and not the forced removal.
        let mut errors = vec![
            pipeline(
                &dest,
                &state,
                "own",
                &block_tree("rc", "v2\n", Placement::Append),
                DriftPolicy::Refuse,
            )
            .expect_err("no occurrence is known to be the recorded one"),
        ];
        errors.push(
            pipeline(
                &dest,
                &state,
                "own",
                &block_tree("rc", "v2\n", Placement::Append),
                DriftPolicy::Overwrite,
            )
            .expect_err("and --force lifts nothing it cannot re-verify"),
        );
        let (manifest, plan) = {
            let dest_dir = dir_at(dest.root());
            let state_dir = dir_at(state.root());
            let manifest = load_manifest(&state_dir).expect("load manifest");
            let observations = observe(&dest_dir, &manifest).expect("observe");
            let plan = decide_removal(
                "own",
                RemovalScope::Everything,
                &manifest,
                &observations,
                None,
                PlanOptions {
                    drift: DriftPolicy::Overwrite,
                    ..PlanOptions::default()
                },
            );
            (manifest, plan)
        };
        errors.push(apply_at(&dest, &state, &manifest, &plan).expect_err("nor does the removal"));

        for error in errors {
            match error {
                Error::Drift { paths } => assert_eq!(paths, BTreeSet::from(["rc".into()])),
                other => panic!("expected Drift, got {other:?}"),
            }
        }
        assert_eq!(container(&dest, "rc"), edited);
        assert!(persisted(&state).entries.contains_key(Utf8Path::new("rc")));
    }
}

#[test]
fn a_hand_built_plan_expecting_another_marker_fails_up_front() {
    // The marker is what tells the projection's bytes from the author's, so
    // an expectation naming a line the author wrote would point the strip at
    // the author's own tail. The record's marker is the only one an
    // expectation may name.
    let (dest, state) = fixtures();
    let author = "author\n# theirs\ntheir tail\n";
    Tree::new().file("rc", author).write_under(dest.root());
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "rc".into(),
        recorded(
            block_kind(Placement::Append),
            sha256_hex(b"managed\n"),
            &["own"],
        ),
    );
    let plan = Plan {
        owner: "own".to_owned(),
        external_targets: ExternalTargetPolicy::Refuse,
        actions: BTreeMap::from([(
            "rc".into(),
            Action::Remove {
                expected: Some(NodeSignature {
                    kind: EntryKind::Block {
                        marker: "# theirs".to_owned(),
                        placement: Placement::Append,
                    },
                    hash: sha256_hex(b"their tail\n"),
                    executable: false,
                }),
            },
        )]),
    };

    let error = apply_at(&dest, &state, &manifest, &plan)
        .expect_err("the expectation names a region the manifest does not record");

    match error {
        Error::Block { blocks } => assert_eq!(
            blocks,
            BTreeMap::from([("rc".into(), BlockFault::SignatureNotRecorded)])
        ),
        other => panic!("expected Block, got {other:?}"),
    }
    // Nothing of the author's was stripped.
    assert_eq!(container(&dest, "rc"), author);
}

#[test]
fn a_recorded_region_back_under_the_old_marker_refuses_rather_than_stranding_it() {
    let (dest, state) = fixtures();
    let author = "author\n";
    Tree::new().file("rc", author).write_under(dest.root());
    pipeline(
        &dest,
        &state,
        "own",
        &block_tree("rc", "managed\n", Placement::Append),
        DriftPolicy::Refuse,
    )
    .expect("project under the first marker");

    // The region goes missing, so changing the marker plans a write rather
    // than the overwrite that would have migrated it.
    fs::write(dest.path("rc"), author).expect("strip the region by hand");
    let renamed = BTreeMap::from([(
        Utf8PathBuf::from("rc"),
        Entry::Block {
            body: b"managed\n".to_vec(),
            marker: "# renamed".to_owned(),
            placement: Placement::Append,
        },
    )]);
    let (manifest, plan) = plan_for(&dest, &state, "own", &renamed, DriftPolicy::Refuse);
    assert!(matches!(
        plan.actions[Utf8Path::new("rc")],
        Action::Write { .. }
    ));

    // And it comes back in the gap. Splicing under the new marker would
    // leave this one standing with nothing recording it — one stranded body
    // per marker change.
    let restored = "author\n# proiectio\nmanaged\n";
    fs::write(dest.path("rc"), restored).expect("restore the old region");

    let error =
        apply_at(&dest, &state, &manifest, &plan).expect_err("the region changed under the plan");
    match error {
        Error::Drift { paths } => assert_eq!(paths, BTreeSet::from(["rc".into()])),
        other => panic!("expected Drift, got {other:?}"),
    }
    assert_eq!(container(&dest, "rc"), restored);
}

#[test]
fn drift_policy_overwrite_lifts_the_refusal_but_still_guards_the_gap() {
    let (dest, state) = fixtures();
    let v1 = Tree::new().file("m.txt", "recorded");
    pipeline(&dest, &state, "own", &v1.entries(), DriftPolicy::Refuse).expect("project");
    fs::write(dest.path("m.txt"), "user edit").expect("drift the file");

    // Lifted: the plan expects the drifted node it observed, and applies.
    let v2 = Tree::new().file("m.txt", "forced");
    let report = pipeline(&dest, &state, "own", &v2.entries(), DriftPolicy::Overwrite)
        .expect("--force overwrites the edit");
    assert_eq!(
        report.outcomes,
        BTreeMap::from([("m.txt".into(), ApplyOutcome::Overwritten)])
    );
    assert_tree(dest.root(), &v2);

    // But the lift is for the node the plan saw: a second edit in the gap
    // still refuses.
    fs::write(dest.path("m.txt"), "another edit").expect("drift again");
    let (manifest, plan) = plan_for(&dest, &state, "own", &v1.entries(), DriftPolicy::Overwrite);
    fs::write(dest.path("m.txt"), "yet another edit").expect("edit in the gap");
    let error = apply_at(&dest, &state, &manifest, &plan).expect_err("the gap re-check holds");
    match error {
        Error::Drift { paths } => assert_eq!(paths, BTreeSet::from(["m.txt".into()])),
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn load_manifest_reads_an_absent_file_as_empty() {
    let (_, state) = fixtures();
    assert_eq!(persisted(&state), Manifest::new());
}

#[test]
fn load_manifest_reports_format_and_version_defects() {
    let (_, state) = fixtures();
    fs::write(state.path(MANIFEST_FILE_NAME), "not json").expect("write garbage");
    match load_manifest(&dir_at(state.root())) {
        Err(Error::ManifestFormat { path, .. }) => {
            assert_eq!(path, Utf8PathBuf::from(MANIFEST_FILE_NAME))
        }
        other => panic!("expected ManifestFormat, got {other:?}"),
    }

    fs::write(
        state.path(MANIFEST_FILE_NAME),
        r#"{"version": 9, "entries": {}}"#,
    )
    .expect("write a future version");
    match load_manifest(&dir_at(state.root())) {
        Err(Error::ManifestVersion {
            found, supported, ..
        }) => {
            assert_eq!((found, supported), (9, MANIFEST_VERSION))
        }
        other => panic!("expected ManifestVersion, got {other:?}"),
    }
}

#[test]
fn save_manifest_round_trips_and_leaves_no_litter() {
    let (_, state) = fixtures();
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "a.txt".into(),
        recorded(EntryKind::File, sha256_hex(b"alpha"), &["one", "two"]),
    );

    save_manifest(&dir_at(state.root()), &manifest).expect("save");

    assert_eq!(persisted(&state), manifest);
    assert_eq!(names_in(&state), vec![MANIFEST_FILE_NAME.to_owned()]);
}
