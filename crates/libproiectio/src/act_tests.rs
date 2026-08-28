use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::fs_utf8::Dir;

use super::*;
use crate::test_support::{Fixture, Tree, assert_tree};
use crate::{DriftPolicy, EntryKind, ExternalTargetPolicy, PlanOptions, decide, observe};

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
fn an_overwrite_expecting_a_block_signature_fails_up_front_and_writes_nothing() {
    let (dest, state) = fixtures();
    Tree::new()
        .file("a.txt", "old")
        .file("conf", "anything")
        .write_under(dest.root());
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "a.txt".into(),
        recorded(EntryKind::File, sha256_hex(b"old"), &["own"]),
    );
    manifest.entries.insert(
        "conf".into(),
        recorded(EntryKind::Block, sha256_hex(b"body"), &["own"]),
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
                        kind: EntryKind::Block,
                        hash: sha256_hex(b"body"),
                        executable: false,
                    },
                },
            ),
        ]),
    };

    let error =
        apply_at(&dest, &state, &manifest, &plan).expect_err("the block seam reports up front");

    match error {
        Error::ApplyBlockUnimplemented { paths } => {
            assert_eq!(paths, BTreeSet::from(["conf".into()]))
        }
        other => panic!("expected ApplyBlockUnimplemented, got {other:?}"),
    }
    // Up front means up front: the removal half of the plan did not run.
    assert_tree(
        dest.root(),
        &Tree::new().file("a.txt", "old").file("conf", "anything"),
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

#[test]
fn a_planned_block_write_is_a_structured_seam_error() {
    let (dest, state) = fixtures();
    let desired = BTreeMap::from([(
        Utf8PathBuf::from("shared.conf"),
        Entry::Block {
            body: b"managed region\n".to_vec(),
        },
    )]);

    let error = pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse)
        .expect_err("block regions are issue #14");

    match error {
        Error::ApplyBlockUnimplemented { paths } => {
            assert_eq!(paths, BTreeSet::from(["shared.conf".into()]))
        }
        other => panic!("expected ApplyBlockUnimplemented, got {other:?}"),
    }
    assert_tree(dest.root(), &Tree::new());
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
