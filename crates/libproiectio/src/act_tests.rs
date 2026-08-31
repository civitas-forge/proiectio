use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::fs_utf8::Dir;

use super::*;
use crate::test_support::{
    Fixture, Tree, assert_tree, paths_of, plant, refusals_of, sourced_of, state_at,
};
use crate::{
    BlockMarkers, Desired, DriftPolicy, Dropped, EntryKind, ExternalTargetPolicy, Origin,
    OverwriteReason, PlanOptions, RefusalKind, RemovalScope, block_markers, decide, decide_removal,
    observe,
};

// Ambient authority is the test's to spend; the library never opens ambient paths.
fn dir_at(root: &Utf8Path) -> Dir {
    Dir::open_ambient_dir(root, cap_std::ambient_authority()).expect("open fixture root as a Dir")
}

fn fixtures() -> (Fixture, Fixture) {
    (Tree::new().materialize(), Tree::new().materialize())
}

// Observe → decide split out so tests can mutate the disk in the plan-to-apply gap.
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

fn plan_for_with(
    dest: &Fixture,
    state: &Fixture,
    owner: &str,
    desired: &BTreeMap<Utf8PathBuf, Entry>,
    options: PlanOptions,
) -> (Manifest, Plan) {
    let dest = dir_at(dest.root());
    let state = state_at(state.root());
    let manifest = load_manifest(&state).expect("load manifest");
    let desired = Desired::from_caller(desired.clone());
    let observations =
        observe(&dest, &manifest, &block_markers(&desired)).expect("observe destination");
    let plan = decide(owner, &desired, &manifest, &observations, None, options).expect("decide");
    (manifest, plan)
}

fn plan_from(
    dest: &Fixture,
    state: &Fixture,
    owner: &str,
    desired: &BTreeMap<Utf8PathBuf, Entry>,
    origin: Origin,
) -> (Manifest, Plan) {
    let (manifest, mut plan) = plan_for(dest, state, owner, desired, DriftPolicy::Refuse);
    plan.origins = desired
        .keys()
        .map(|path| (path.clone(), origin.clone()))
        .collect();
    (manifest, plan)
}

// Applies a plan, keeping the error a stopped run stopped with.
fn apply_at(
    dest: &Fixture,
    state: &Fixture,
    manifest: &Manifest,
    plan: &Plan,
) -> Result<ApplyReport> {
    stopping(dest, state, manifest, plan).map_err(|aborted| match aborted.stopped {
        Stopped::Applying(error) => error,
        stopped => panic!("expected a stop at an action, got {stopped:?}"),
    })
}

fn stopping(
    dest: &Fixture,
    state: &Fixture,
    manifest: &Manifest,
    plan: &Plan,
) -> std::result::Result<ApplyReport, Box<Aborted>> {
    apply(
        &dir_at(dest.root()),
        &state_at(state.root()),
        manifest,
        plan,
    )
}

fn stopping_at(dest: &Fixture, state: &Fixture, manifest: &Manifest, plan: &Plan) -> Box<Aborted> {
    stopping(dest, state, manifest, plan).expect_err("a run that stops part-way")
}

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

fn persisted(state: &Fixture) -> Manifest {
    load_manifest(&state_at(state.root())).expect("load persisted manifest")
}

fn recorded(kind: EntryKind, hash: String, owners: &[&str]) -> ManifestEntry {
    ManifestEntry {
        kind,
        hash,
        executable: false,
        owners: owners.iter().map(|owner| (*owner).to_owned()).collect(),
    }
}

fn verdicts(report: &ApplyReport) -> BTreeMap<Utf8PathBuf, ApplyOutcome> {
    report
        .report
        .rows
        .iter()
        .map(|(path, row)| (path.clone(), row.verdict))
        .collect()
}

fn stated_facts<V>(
    rows: &BTreeMap<Utf8PathBuf, Row<V>>,
) -> BTreeMap<Utf8PathBuf, Option<PathFacts>> {
    rows.iter()
        .map(|(path, row)| (path.clone(), row.facts.clone()))
        .collect()
}

fn facts_at<'a>(report: &'a ApplyReport, path: &str) -> &'a PathFacts {
    report.report.rows[Utf8Path::new(path)]
        .facts
        .as_ref()
        .expect("the row carries facts")
}

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
        verdicts(&report),
        BTreeMap::from([
            ("notes/a.txt".into(), ApplyOutcome::Written),
            ("bin/run".into(), ApplyOutcome::Written),
        ])
    );
    assert_eq!(
        facts_at(&report, "bin/run"),
        &PathFacts {
            shape: Some(PathShape::File { executable: true }),
            owners: BTreeSet::from(["own".to_owned()]),
            origin: Some(Origin::Caller),
        }
    );
    assert_eq!(
        facts_at(&report, "notes/a.txt").shape,
        Some(PathShape::File { executable: false })
    );
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
        verdicts(&report)
            .values()
            .all(|outcome| *outcome == ApplyOutcome::Skipped),
        "expected every outcome skipped, got {:?}",
        verdicts(&report)
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
        verdicts(&report),
        BTreeMap::from([("tool".into(), ApplyOutcome::Overwritten)])
    );
    assert_tree(dest.root(), &v2);
    assert!(report.manifest.entries[Utf8Path::new("tool")].executable);
}

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

    assert!(!error.is_refusal(), "expected an I/O error, got {error:?}");
    assert_tree(dest.root(), &Tree::new().file("a.txt", "alpha").dir("ro"));
    let manifest = persisted(&state);
    assert_eq!(
        manifest.entries.keys().collect::<Vec<_>>(),
        vec![Utf8Path::new("a.txt")]
    );

    fs::set_permissions(&ro, fs::Permissions::from_mode(0o755)).expect("heal the directory");
    let report = pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse)
        .expect("the re-run completes cleanly");

    assert_eq!(
        verdicts(&report),
        BTreeMap::from([
            ("a.txt".into(), ApplyOutcome::Skipped),
            ("ro/x.txt".into(), ApplyOutcome::Written),
            ("z.txt".into(), ApplyOutcome::Written),
        ])
    );
    assert_tree(dest.root(), &tree);
}

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
        Error::Refused(refused) if refused.kind() == RefusalKind::Drift => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["m.txt".into()]))
        }
        other => panic!("expected Drift, got {other:?}"),
    }
    assert_tree(dest.root(), &Tree::new().file("m.txt", "tampered"));
    assert_eq!(persisted(&state), manifest);
}

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
            Error::Refused(refused) if refused.kind() == RefusalKind::Containment => {
                assert_eq!(paths_of(&refused), BTreeSet::from(["logs/x.txt".into()]));
                assert_eq!(
                    refused.to_string(),
                    "refusing paths that violate containment: \
                     logs/x.txt (below the symlink logs)"
                );
            }
            other => panic!("expected Containment for target {target}, got {other:?}"),
        }
        assert!(
            !dest.path("real").join("x.txt").exists(),
            "nothing may land through the link"
        );
    }
}

#[test]
fn a_directory_swapped_for_a_link_after_the_plan_names_the_link_it_met() {
    let (dest, state) = fixtures();
    Tree::new().dir("logs").write_under(dest.root());
    let desired = Tree::new().file("logs/x.txt", "written").entries();

    let (manifest, plan) = plan_for(&dest, &state, "own", &desired, DriftPolicy::Refuse);
    assert!(
        matches!(
            plan.actions[Utf8Path::new("logs/x.txt")],
            Action::Write { .. }
        ),
        "the plan walked an ordinary directory"
    );

    // The gap between the two calls: `logs` is a link nobody recorded.
    std::fs::remove_dir(dest.path("logs")).expect("clear the directory");
    Tree::new()
        .dir("real")
        .symlink("logs", "real")
        .write_under(dest.root());
    let error = apply_at(&dest, &state, &manifest, &plan)
        .expect_err("no write through a link nobody recorded");

    match error {
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment => assert_eq!(
            refused.to_string(),
            "refusing paths that violate containment: logs/x.txt (below the symlink logs)"
        ),
        other => panic!("expected Containment, got {other:?}"),
    }
    assert!(
        !dest.path("real").join("x.txt").exists(),
        "nothing may land through the link"
    );
}

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

    assert_tree(
        dest.root(),
        &Tree::new()
            .file("keep.txt", "precious")
            .file("data/nested.txt", "also precious")
            .file("data/mine.txt", "projected"),
    );

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
        verdicts(&report)
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

fn removal_pipeline(
    dest: &Fixture,
    state: &Fixture,
    owner: &str,
    scope: RemovalScope<'_>,
    policy: DriftPolicy,
) -> Result<ApplyReport> {
    let (manifest, plan) = removal_plan_for(dest, state, owner, scope, policy);
    apply_at(dest, state, &manifest, &plan)
}

fn removal_plan_for(
    dest: &Fixture,
    state: &Fixture,
    owner: &str,
    scope: RemovalScope<'_>,
    policy: DriftPolicy,
) -> (Manifest, Plan) {
    let dest = dir_at(dest.root());
    let state = state_at(state.root());
    let manifest = load_manifest(&state).expect("load manifest");
    let observations =
        observe(&dest, &manifest, &BlockMarkers::new()).expect("observe destination");
    let plan = decide_removal(owner, scope, &manifest, &observations, None, policy);
    (manifest, plan)
}

fn requested(paths: &[&str]) -> BTreeSet<Utf8PathBuf> {
    paths.iter().map(Utf8PathBuf::from).collect()
}

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
        Error::Refused(refused) if refused.kind() == RefusalKind::Drift && paths_of(refused) == BTreeSet::from([Utf8PathBuf::from("a/b.txt")])
    ));
    assert_tree(dest.root(), &edited);
    assert_eq!(
        persisted(&state).entries.keys().collect::<Vec<_>>(),
        vec![Utf8Path::new("a/b.txt"), Utf8Path::new("c.txt")]
    );

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
fn removal_prunes_the_dirs_a_hand_deleted_path_left_empty() {
    let (dest, state) = fixtures();
    let tree = Tree::new().file("only/deep/file.txt", "projected");
    pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse).expect("project");
    fs::remove_file(dest.path("only/deep/file.txt")).expect("delete the file by hand");

    let report = removal_pipeline(
        &dest,
        &state,
        "own",
        RemovalScope::Everything,
        DriftPolicy::Refuse,
    )
    .expect("removal");

    assert_eq!(
        verdicts(&report),
        BTreeMap::from([("only/deep/file.txt".into(), ApplyOutcome::Forgot)])
    );
    assert_tree(dest.root(), &Tree::new());
    assert!(persisted(&state).entries.is_empty());
}

#[test]
fn removal_prunes_the_dirs_left_standing_above_a_hand_deleted_one() {
    let (dest, state) = fixtures();
    let tree = Tree::new().file("only/deep/file.txt", "projected");
    pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse).expect("project");
    fs::remove_dir_all(dest.path("only/deep")).expect("delete the directory by hand");

    let report = removal_pipeline(
        &dest,
        &state,
        "own",
        RemovalScope::Everything,
        DriftPolicy::Refuse,
    )
    .expect("removal");

    assert_eq!(
        verdicts(&report),
        BTreeMap::from([("only/deep/file.txt".into(), ApplyOutcome::Forgot)])
    );
    assert_tree(dest.root(), &Tree::new());
    assert!(persisted(&state).entries.is_empty());
}

#[test]
fn a_named_path_the_owner_does_not_hold_is_reported_and_nothing_else() {
    let (dest, state) = fixtures();
    let tree = Tree::new().file("mine.txt", "projected");
    pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse).expect("project");
    fs::write(dest.path("foreign.txt"), "theirs").expect("plant a foreign file");

    let report = removal_pipeline(
        &dest,
        &state,
        "own",
        RemovalScope::Paths(&requested(&["typo.txt", "foreign.txt", "mine.txt"])),
        DriftPolicy::Refuse,
    )
    .expect("removal");

    assert_eq!(
        verdicts(&report),
        BTreeMap::from([
            ("foreign.txt".into(), ApplyOutcome::NotRecorded),
            ("mine.txt".into(), ApplyOutcome::Removed),
            ("typo.txt".into(), ApplyOutcome::NotRecorded),
        ])
    );
    assert_tree(dest.root(), &Tree::new().file("foreign.txt", "theirs"));
    assert!(persisted(&state).entries.is_empty());
}

// --- a directory standing where a file or a link belongs ---

#[test]
fn a_recorded_file_becomes_a_directory_in_one_run() {
    let (dest, state) = fixtures();
    let flat = Tree::new().file("build", "one file\n");
    pipeline(&dest, &state, "own", &flat.entries(), DriftPolicy::Refuse).expect("project");

    let nested = Tree::new().file("build/main.rs", "fn main() {}\n");
    pipeline(&dest, &state, "own", &nested.entries(), DriftPolicy::Refuse)
        .expect("a file becomes a directory in one run");

    assert_tree(dest.root(), &nested);
}

#[test]
fn a_directory_the_projection_wrote_becomes_a_file_in_one_run() {
    let (dest, state) = fixtures();
    let nested = Tree::new().file("build.sh/main.sh", "#!/bin/sh\n");
    pipeline(&dest, &state, "own", &nested.entries(), DriftPolicy::Refuse).expect("project");

    let flat = Tree::new().executable("build.sh", "#!/bin/sh\nmake\n");
    let report = pipeline(&dest, &state, "own", &flat.entries(), DriftPolicy::Refuse)
        .expect("a directory becomes a file in one run");

    assert_eq!(
        verdicts(&report),
        BTreeMap::from([
            ("build.sh".into(), ApplyOutcome::Written),
            ("build.sh/main.sh".into(), ApplyOutcome::Removed),
        ])
    );
    assert_tree(dest.root(), &flat);
    assert_eq!(
        persisted(&state).entries.keys().collect::<Vec<_>>(),
        [Utf8Path::new("build.sh")]
    );
}

#[test]
fn a_directory_left_empty_by_a_hand_deletion_becomes_the_desired_file() {
    let (dest, state) = fixtures();
    let nested = Tree::new().file("build.sh/main.sh", "#!/bin/sh\n");
    pipeline(&dest, &state, "own", &nested.entries(), DriftPolicy::Refuse).expect("project");
    fs::remove_file(dest.path("build.sh/main.sh")).expect("delete the file by hand");

    let flat = Tree::new().executable("build.sh", "#!/bin/sh\nmake\n");
    let report = pipeline(&dest, &state, "own", &flat.entries(), DriftPolicy::Refuse)
        .expect("the emptied directory gives way");

    assert_eq!(
        verdicts(&report),
        BTreeMap::from([
            ("build.sh".into(), ApplyOutcome::Written),
            ("build.sh/main.sh".into(), ApplyOutcome::Forgot),
        ])
    );
    assert_tree(dest.root(), &flat);
}

#[test]
fn a_node_nothing_records_holds_the_directory_and_the_run_names_it() {
    let (dest, state) = fixtures();
    let nested = Tree::new().file("build.sh/main.sh", "#!/bin/sh\n");
    pipeline(&dest, &state, "own", &nested.entries(), DriftPolicy::Refuse).expect("project");
    fs::write(dest.path("build.sh/notes.md"), "mine").expect("plant a foreign file");

    let flat = Tree::new().executable("build.sh", "#!/bin/sh\nmake\n");
    for policy in [DriftPolicy::Refuse, DriftPolicy::Overwrite] {
        let error = pipeline(&dest, &state, "own", &flat.entries(), policy)
            .expect_err("the foreign file holds the directory");
        assert!(
            matches!(
                &error,
                Error::Refused(refused)
                    if refused.kind() == RefusalKind::DirectoryInTheWay
                        && paths_of(refused) == BTreeSet::from([Utf8PathBuf::from("build.sh")])
            ),
            "{error}"
        );
        assert!(error.to_string().contains("build.sh/notes.md"), "{error}");
    }
    assert_tree(
        dest.root(),
        &nested.clone().file("build.sh/notes.md", "mine"),
    );
}

#[test]
fn an_empty_directory_nested_in_the_scaffolding_holds_it() {
    let (dest, state) = fixtures();
    let nested = Tree::new().file("build.sh/main.sh", "#!/bin/sh\n");
    pipeline(&dest, &state, "own", &nested.entries(), DriftPolicy::Refuse).expect("project");
    fs::create_dir(dest.path("build.sh/scratch")).expect("make a directory by hand");

    let flat = Tree::new().executable("build.sh", "#!/bin/sh\nmake\n");
    let error = pipeline(
        &dest,
        &state,
        "own",
        &flat.entries(),
        DriftPolicy::Overwrite,
    )
    .expect_err("the hand-made directory holds it");

    assert!(
        matches!(
            &error,
            Error::Refused(refused) if refused.kind() == RefusalKind::DirectoryInTheWay
        ),
        "{error}"
    );
    assert!(error.to_string().contains("build.sh/scratch"), "{error}");
    assert_tree(dest.root(), &nested.clone().dir("build.sh/scratch"));
}

#[test]
fn a_name_the_walk_cannot_read_holds_the_directory_and_nothing_is_written() {
    let (dest, state) = fixtures();
    let nested = Tree::new().file("build.sh/main.sh", "#!/bin/sh\n");
    pipeline(&dest, &state, "own", &nested.entries(), DriftPolicy::Refuse).expect("project");
    let unnameable = dest
        .path("build.sh")
        .as_std_path()
        .join(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(b"bad-\xff-name"));
    if !plant(&unnameable) {
        return;
    }

    let flat = Tree::new().executable("build.sh", "#!/bin/sh\nmake\n");
    for policy in [DriftPolicy::Refuse, DriftPolicy::Overwrite] {
        let error = pipeline(&dest, &state, "own", &flat.entries(), policy)
            .expect_err("the unreadable name holds the directory");
        assert!(
            matches!(
                &error,
                Error::Refused(refused)
                    if refused.kind() == RefusalKind::DirectoryInTheWay
                        && paths_of(refused) == BTreeSet::from([Utf8PathBuf::from("build.sh")])
            ),
            "{error}"
        );
        assert!(error.to_string().contains("not UTF-8"), "{error}");
    }

    assert_eq!(
        fs::read_to_string(dest.path("build.sh/main.sh")).expect("the orphan is still there"),
        "#!/bin/sh\n"
    );
    assert_eq!(
        persisted(&state).entries.keys().collect::<Vec<_>>(),
        [Utf8Path::new("build.sh/main.sh")]
    );
}

#[test]
fn a_recorded_path_drifted_to_an_empty_directory_is_forced_over() {
    let (dest, state) = fixtures();
    let tree = Tree::new().file("g.txt", "projected\n");
    pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse).expect("project");
    fs::remove_file(dest.path("g.txt")).expect("remove the file");
    fs::create_dir(dest.path("g.txt")).expect("put a directory there");

    let refused = pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse)
        .expect_err("the unforced run states the drift");
    assert!(
        matches!(
            &refused,
            Error::Refused(refused) if refused.kind() == RefusalKind::Drift
        ),
        "{refused}"
    );

    let report = pipeline(
        &dest,
        &state,
        "own",
        &tree.entries(),
        DriftPolicy::Overwrite,
    )
    .expect("forcing replaces the empty directory");

    assert_eq!(
        verdicts(&report),
        BTreeMap::from([("g.txt".into(), ApplyOutcome::Overwritten)])
    );
    assert_tree(dest.root(), &tree);
}

#[test]
fn a_directory_overwrite_interrupted_before_its_write_states_the_removal() {
    let (dest, state) = fixtures();
    let tree = Tree::new().file("b.txt", "projected\n").file("z", "one\n");
    pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse).expect("project");
    fs::remove_file(dest.path("z")).expect("remove the file");
    fs::create_dir(dest.path("z")).expect("put a directory there");
    fs::write(dest.path("b.txt"), "edited\n").expect("edit in place");

    let next = Tree::new().file("b.txt", "wanted\n").file("z", "two\n");
    let (manifest, plan) = plan_for(
        &dest,
        &state,
        "own",
        &next.entries(),
        DriftPolicy::Overwrite,
    );
    // The gap: `b.txt` changes again, failing its re-check after the removal pass took `z`.
    fs::write(dest.path("b.txt"), "tampered\n").expect("tamper in the gap");

    let error = apply_at(&dest, &state, &manifest, &plan).expect_err("the re-check refuses");
    assert!(
        matches!(&error, Error::Refused(refused) if refused.kind() == RefusalKind::Drift),
        "{error}"
    );

    assert!(!dest.path("z").exists(), "the directory was removed");
    assert_eq!(
        persisted(&state).entries.keys().collect::<Vec<_>>(),
        [Utf8Path::new("b.txt")]
    );

    fs::write(dest.path("b.txt"), "wanted\n").expect("settle the drift by hand");
    let report = pipeline(&dest, &state, "own", &next.entries(), DriftPolicy::Refuse)
        .expect("the next run reconciles");
    assert_eq!(
        verdicts(&report),
        BTreeMap::from([
            ("b.txt".into(), ApplyOutcome::Skipped),
            ("z".into(), ApplyOutcome::Written),
        ])
    );
    assert_tree(dest.root(), &next);
}

#[test]
fn a_directory_overwrite_interrupted_before_its_write_leaves_no_empty_ancestor() {
    let (dest, state) = fixtures();
    let tree = Tree::new()
        .file("b.txt", "projected\n")
        .file("only/z", "one\n");
    pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse).expect("project");
    fs::remove_file(dest.path("only/z")).expect("remove the file");
    fs::create_dir(dest.path("only/z")).expect("put a directory there");
    fs::write(dest.path("b.txt"), "edited\n").expect("edit in place");

    let next = Tree::new()
        .file("b.txt", "wanted\n")
        .file("only/z", "two\n");
    let (manifest, plan) = plan_for(
        &dest,
        &state,
        "own",
        &next.entries(),
        DriftPolicy::Overwrite,
    );
    fs::write(dest.path("b.txt"), "tampered\n").expect("tamper in the gap");

    apply_at(&dest, &state, &manifest, &plan).expect_err("the re-check refuses");

    assert!(!dest.path("only").exists(), "no empty ancestor is left");
    assert_eq!(
        persisted(&state).entries.keys().collect::<Vec<_>>(),
        [Utf8Path::new("b.txt")]
    );

    fs::write(dest.path("b.txt"), "wanted\n").expect("settle the drift by hand");
    pipeline(&dest, &state, "own", &next.entries(), DriftPolicy::Refuse)
        .expect("the next run reconciles");
    assert_tree(dest.root(), &next);
}

#[test]
fn a_removal_clears_a_path_drifted_to_an_empty_directory_under_force() {
    let (dest, state) = fixtures();
    let tree = Tree::new().file("only/g.txt", "projected\n");
    pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse).expect("project");
    fs::remove_file(dest.path("only/g.txt")).expect("remove the file");
    fs::create_dir(dest.path("only/g.txt")).expect("put a directory there");

    let report = removal_pipeline(
        &dest,
        &state,
        "own",
        RemovalScope::Everything,
        DriftPolicy::Overwrite,
    )
    .expect("forcing clears the empty directory");

    assert_eq!(
        verdicts(&report),
        BTreeMap::from([("only/g.txt".into(), ApplyOutcome::Removed)])
    );
    assert_tree(dest.root(), &Tree::new());
    assert!(persisted(&state).entries.is_empty());
}

#[test]
fn a_path_drifted_to_a_directory_holding_anything_refuses_however_it_is_run() {
    let (dest, state) = fixtures();
    let tree = Tree::new().file("g.txt", "projected\n");
    pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse).expect("project");
    fs::remove_file(dest.path("g.txt")).expect("remove the file");
    fs::create_dir(dest.path("g.txt")).expect("put a directory there");
    fs::write(dest.path("g.txt/note.md"), "theirs").expect("plant a file inside it");

    for policy in [DriftPolicy::Refuse, DriftPolicy::Overwrite] {
        let writing =
            pipeline(&dest, &state, "own", &tree.entries(), policy).expect_err("the write refuses");
        let removing = removal_pipeline(&dest, &state, "own", RemovalScope::Everything, policy)
            .expect_err("the removal refuses");
        for error in [writing, removing] {
            assert!(
                matches!(
                    &error,
                    Error::Refused(refused)
                        if refused.kind() == RefusalKind::DirectoryInTheWay
                            && paths_of(refused) == BTreeSet::from([Utf8PathBuf::from("g.txt")])
                ),
                "{error}"
            );
            assert!(
                error
                    .to_string()
                    .contains("holding g.txt/note.md, which --force does not remove"),
                "{error}"
            );
        }
    }
    assert_tree(dest.root(), &Tree::new().file("g.txt/note.md", "theirs"));
}

#[test]
fn no_write_records_a_key_beneath_an_owned_link() {
    let linked = Tree::new().symlink("logs", "real/missing");
    let beneath = Tree::new().file("logs/deep/file.txt", "projected");

    let (dest, state) = fixtures();
    pipeline(&dest, &state, "own", &linked.entries(), DriftPolicy::Refuse).expect("project a link");
    let error = pipeline(
        &dest,
        &state,
        "second",
        &beneath.entries(),
        DriftPolicy::Refuse,
    )
    .expect_err("a key behind the link refuses");
    assert!(
        matches!(
            &error,
            Error::Refused(refused)
                if refused.kind() == RefusalKind::Containment
                    && paths_of(refused)
                        == BTreeSet::from([Utf8PathBuf::from("logs/deep/file.txt")])
        ),
        "{error}"
    );

    let (dest, state) = fixtures();
    pipeline(
        &dest,
        &state,
        "own",
        &beneath.entries(),
        DriftPolicy::Refuse,
    )
    .expect("project");
    for policy in [DriftPolicy::Refuse, DriftPolicy::Overwrite] {
        let error = pipeline(&dest, &state, "second", &linked.entries(), policy)
            .expect_err("a link over the recorded ancestry refuses");
        assert!(
            matches!(
                &error,
                Error::Refused(refused)
                    if refused.kind() == RefusalKind::DirectoryInTheWay
                        && paths_of(refused) == BTreeSet::from([Utf8PathBuf::from("logs")])
            ),
            "{error}"
        );
        assert!(
            error
                .to_string()
                .contains("logs/deep/file.txt (held by own)"),
            "{error}"
        );
    }
    assert_tree(dest.root(), &beneath);
}

#[test]
fn a_removal_states_the_same_facts_whether_it_is_planned_or_applied() {
    let (dest, state) = fixtures();
    let mine = Tree::new()
        .file("gone.txt", "projected")
        .file("mine.txt", "projected");
    pipeline(&dest, &state, "own", &mine.entries(), DriftPolicy::Refuse).expect("project");
    let theirs = Tree::new().file("theirs.txt", "projected");
    pipeline(
        &dest,
        &state,
        "other",
        &theirs.entries(),
        DriftPolicy::Refuse,
    )
    .expect("project under a second owner");
    fs::remove_file(dest.path("gone.txt")).expect("delete one recorded file by hand");

    let (manifest, plan) = removal_plan_for(
        &dest,
        &state,
        "own",
        RemovalScope::Paths(&requested(&[
            "gone.txt",
            "mine.txt",
            "theirs.txt",
            "typo.txt",
        ])),
        DriftPolicy::Refuse,
    );
    let planned = plan.report(&manifest);
    let applied = apply_at(&dest, &state, &manifest, &plan).expect("removal");

    assert_eq!(
        verdicts(&applied),
        BTreeMap::from([
            ("gone.txt".into(), ApplyOutcome::Forgot),
            ("mine.txt".into(), ApplyOutcome::Removed),
            ("theirs.txt".into(), ApplyOutcome::NotRecorded),
            ("typo.txt".into(), ApplyOutcome::NotRecorded),
        ])
    );
    assert_eq!(
        stated_facts(&planned.rows),
        stated_facts(&applied.report.rows)
    );
    assert_eq!(
        facts_at(&applied, "theirs.txt").owners,
        BTreeSet::from(["other".to_owned()])
    );
    assert_eq!(
        facts_at(&applied, "gone.txt").shape,
        Some(PathShape::File { executable: false })
    );
    assert_eq!(applied.report.rows[Utf8Path::new("typo.txt")].facts, None);
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
        verdicts(&report),
        BTreeMap::from([("a/b/gone.txt".into(), ApplyOutcome::Removed)])
    );
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
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment && paths_of(refused) == BTreeSet::from([Utf8PathBuf::from("../escape")])
    ));
    assert_tree(dest.root(), &tree);
}

#[test]
fn removing_a_missing_path_forgets_it_rather_than_claiming_a_removal() {
    let (dest, state) = fixtures();
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "gone.txt".into(),
        recorded(EntryKind::File, sha256_hex(b"bye"), &["own"]),
    );
    save_manifest(&state_at(state.root()), &manifest).expect("seed the state dir");

    let report =
        pipeline(&dest, &state, "own", &BTreeMap::new(), DriftPolicy::Refuse).expect("removal");

    assert_eq!(
        verdicts(&report),
        BTreeMap::from([("gone.txt".into(), ApplyOutcome::Forgot)])
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
    save_manifest(&state_at(state.root()), &manifest).expect("seed the state dir");
    let (manifest, plan) = plan_for(&dest, &state, "own", &BTreeMap::new(), DriftPolicy::Refuse);
    assert_eq!(
        plan.actions,
        BTreeMap::from([("gone.txt".into(), Action::Remove { expected: None })])
    );
    fs::write(dest.path("gone.txt"), "reappeared").expect("a node appears in the gap");

    let error = apply_at(&dest, &state, &manifest, &plan).expect_err("the appearance refuses");

    match error {
        Error::Refused(refused) if refused.kind() == RefusalKind::Drift => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["gone.txt".into()]))
        }
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
    save_manifest(&state_at(state.root()), &manifest).expect("seed the state dir");

    let report =
        pipeline(&dest, &state, "own", &BTreeMap::new(), DriftPolicy::Refuse).expect("removal");

    assert_eq!(
        verdicts(&report),
        BTreeMap::from([("latest".into(), ApplyOutcome::Removed)])
    );
    assert_eq!(
        facts_at(&report, "latest"),
        &PathFacts {
            shape: Some(PathShape::Symlink { target: None }),
            owners: BTreeSet::from(["own".to_owned()]),
            origin: Some(Origin::Caller),
        }
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
        verdicts(&report),
        BTreeMap::from([("shared.txt".into(), ApplyOutcome::Skipped)])
    );
    assert_eq!(
        report.manifest.entries[Utf8Path::new("shared.txt")].owners,
        BTreeSet::from(["one".to_owned(), "two".to_owned()])
    );
    assert_eq!(
        facts_at(&report, "shared.txt").owners,
        BTreeSet::from(["one".to_owned(), "two".to_owned()])
    );

    let report =
        pipeline(&dest, &state, "two", &BTreeMap::new(), DriftPolicy::Refuse).expect("release");
    assert_eq!(
        verdicts(&report),
        BTreeMap::from([("shared.txt".into(), ApplyOutcome::Released)])
    );
    assert_eq!(
        report.manifest.entries[Utf8Path::new("shared.txt")].owners,
        BTreeSet::from(["one".to_owned()])
    );
    assert_eq!(
        facts_at(&report, "shared.txt").owners,
        BTreeSet::from(["one".to_owned(), "two".to_owned()])
    );
    assert_tree(dest.root(), &tree);

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
        Error::Refused(refused) if refused.kind() == RefusalKind::Drift => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["keep.txt".into()]))
        }
        other => panic!("expected Drift, got {other:?}"),
    }
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
        Error::Refused(refused) if refused.kind() == RefusalKind::Foreign => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["a.txt".into()]))
        }
        other => panic!("expected Foreign, got {other:?}"),
    }
    assert_tree(dest.root(), &Tree::new().file("a.txt", "squatter"));
}

#[test]
fn applying_a_plan_carries_its_drops_onto_the_report() {
    let (dest, state) = fixtures();
    let dropped = Dropped {
        member: Utf8PathBuf::from("._pkg"),
        prefix: Utf8PathBuf::new(),
        strip: 1,
        origin: Origin::Archive {
            path: Utf8PathBuf::from("/assets/vendor.tar.gz"),
            via: None,
        },
    };
    let plan = Plan {
        dropped: BTreeSet::from([dropped.clone()]),
        owner: "own".to_owned(),
        origins: BTreeMap::new(),
        external_targets: ExternalTargetPolicy::Refuse,
        actions: BTreeMap::new(),
    };

    let applied = apply_at(&dest, &state, &Manifest::new(), &plan).expect("apply");

    assert!(applied.report.is_empty());
    assert_eq!(applied.dropped, BTreeSet::from([dropped]));
    assert!(applied.manifest.entries.is_empty());
}

#[test]
fn applying_a_plan_with_two_refusal_kinds_reports_the_one_precedence_ranks_first() {
    let (dest, state) = fixtures();
    let plan = Plan {
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::from([("z/escape".into(), Origin::Files)]),
        external_targets: ExternalTargetPolicy::Refuse,
        actions: BTreeMap::from([
            (
                "a.txt".into(),
                Action::Refuse {
                    refusal: Refusal::Drift,
                },
            ),
            (
                "z/escape".into(),
                Action::Refuse {
                    refusal: Refusal::Containment { through: None },
                },
            ),
        ]),
    };

    let error =
        apply_at(&dest, &state, &Manifest::new(), &plan).expect_err("refusals apply nothing");

    assert_eq!(
        error.to_string(),
        "refusing paths that violate containment: z/escape (from individually named files)"
    );
    match error {
        Error::Refused(refused) => {
            assert_eq!(refused.kind(), RefusalKind::Containment);
            assert_eq!(
                sourced_of(&refused),
                BTreeMap::from([(
                    "z/escape".into(),
                    (Refusal::Containment { through: None }, Origin::Files)
                )])
            );
        }
        other => panic!("expected Containment, got {other:?}"),
    }
}

#[test]
fn a_hand_built_plan_with_unnormalized_keys_refuses_containment() {
    let (dest, state) = fixtures();
    let entry = Entry::File {
        contents: b"evil".to_vec(),
        executable: false,
    };
    let plan = Plan {
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::new(),
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
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment => {
            assert_eq!(
                paths_of(&refused),
                BTreeSet::from(["../escape".into(), "a/../b".into()])
            )
        }
        other => panic!("expected Containment, got {other:?}"),
    }
    assert_tree(dest.root(), &Tree::new());
}

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
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::new(),
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
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::new(),
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

#[test]
fn a_plan_removing_past_the_walk_depth_is_named_and_removes_nothing() {
    let (dest, state) = fixtures();
    let past = Utf8PathBuf::from(format!("{}/leaf", ["d"; MAX_WALK_DEPTH + 1].join("/")));
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        past.clone(),
        recorded(EntryKind::File, sha256_hex(b"deep"), &["own"]),
    );
    let plan = Plan {
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::new(),
        actions: BTreeMap::from([(past.clone(), Action::Remove { expected: None })]),
        external_targets: ExternalTargetPolicy::Refuse,
    };

    let error = apply_at(&dest, &state, &manifest, &plan)
        .expect_err("a destination past the limit is not cleaned up either");

    match error {
        Error::DestinationTooDeep { path, limit } => {
            assert_eq!(path, past.parent().expect("the leaf has a parent"));
            assert_eq!(limit, MAX_WALK_DEPTH);
        }
        other => panic!("expected DestinationTooDeep, got {other:?}"),
    }
    assert_tree(dest.root(), &Tree::new());
}

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
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::new(),
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
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::new(),
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
        Error::Refused(refused) if refused.kind() == RefusalKind::Foreign => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["victim.txt".into()]))
        }
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
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::new(),
        external_targets: ExternalTargetPolicy::Refuse,
        actions: BTreeMap::from([(
            "theirs.txt".into(),
            Action::Skip {
                entry: Entry::File {
                    contents: b"same bytes".to_vec(),
                    executable: false,
                },
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
        Error::Refused(refused) if refused.kind() == RefusalKind::Foreign => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["theirs.txt".into()]))
        }
        other => panic!("expected Foreign, got {other:?}"),
    }
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
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::new(),
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
                    reason: OverwriteReason::ContentChanged,
                },
            ),
        ]),
    };

    let error = apply_at(&dest, &state, &manifest, &plan)
        .expect_err("a path never changes between a whole node and a block");

    match error {
        Error::Refused(refused) => assert_eq!(
            refusals_of(&refused),
            BTreeMap::from([(
                "conf".into(),
                Refusal::Block {
                    fault: BlockFault::KindChange
                }
            )])
        ),
        other => panic!("expected Block, got {other:?}"),
    }
    assert_tree(
        dest.root(),
        &Tree::new()
            .file("a.txt", "old")
            .file("conf", "author\n# proiectio\nbody\n"),
    );
}

#[test]
fn a_hand_built_directory_removal_of_a_block_record_fails_up_front() {
    let (dest, state) = fixtures();
    let container = "author\n# proiectio\nbody\n";
    Tree::new().file("conf", container).write_under(dest.root());
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "conf".into(),
        recorded(
            block_kind(Placement::Append),
            sha256_hex(b"body\n"),
            &["own"],
        ),
    );
    let plan = Plan {
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::new(),
        external_targets: ExternalTargetPolicy::Refuse,
        actions: BTreeMap::from([("conf".into(), Action::RemoveDirectory)]),
    };

    let error = apply_at(&dest, &state, &manifest, &plan)
        .expect_err("a block record never stands for a directory");

    match error {
        Error::Refused(refused) => assert_eq!(
            refusals_of(&refused),
            BTreeMap::from([(
                "conf".into(),
                Refusal::Block {
                    fault: BlockFault::KindChange
                }
            )])
        ),
        other => panic!("expected Block, got {other:?}"),
    }
    assert_tree(dest.root(), &Tree::new().file("conf", container));
    assert_eq!(persisted(&state), Manifest::new());
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
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::new(),
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
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["logs/x.txt".into()]))
        }
        other => panic!("expected Containment, got {other:?}"),
    }
}

#[test]
fn a_file_write_the_walk_would_relocate_through_an_owned_link_refuses() {
    let (dest, state) = fixtures();
    let linked = Tree::new().dir("real").symlink("logs", "real");
    linked.write_under(dest.root());
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "logs".into(),
        recorded(EntryKind::Symlink, sha256_hex(b"real"), &["own"]),
    );
    let plan = Plan {
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::new(),
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

    let error =
        apply_at(&dest, &state, &manifest, &plan).expect_err("a file is never written off its key");

    match error {
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment => assert_eq!(
            refused.to_string(),
            "refusing paths that violate containment: logs/x.txt (below the symlink logs)"
        ),
        other => panic!("expected Containment, got {other:?}"),
    }
    assert_tree(dest.root(), &linked);
    assert!(
        !persisted(&state)
            .entries
            .contains_key(Utf8Path::new("logs/x.txt"))
    );
}

#[test]
fn a_block_whose_container_the_walk_relocates_refuses() {
    let (dest, state) = fixtures();
    let linked = Tree::new()
        .file("real/rc", "author line\n")
        .symlink("logs", "real");
    linked.write_under(dest.root());
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "logs".into(),
        recorded(EntryKind::Symlink, sha256_hex(b"real"), &["own"]),
    );
    let plan = Plan {
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::new(),
        external_targets: ExternalTargetPolicy::Refuse,
        actions: BTreeMap::from([(
            "logs/rc".into(),
            Action::Write {
                entry: block("spliced\n", Placement::Append),
            },
        )]),
    };

    let error = apply_at(&dest, &state, &manifest, &plan)
        .expect_err("a region is never spliced off its key");

    match error {
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment => assert_eq!(
            refused.to_string(),
            "refusing paths that violate containment: logs/rc (below the symlink logs)"
        ),
        other => panic!("expected Containment, got {other:?}"),
    }
    assert_tree(dest.root(), &linked);
}

#[test]
fn a_link_released_and_reappearing_in_the_gap_does_not_relocate_the_write() {
    let (dest, state) = fixtures();
    Tree::new().dir("real").write_under(dest.root());
    let mut seeded = Manifest::new();
    seeded.entries.insert(
        "pivot".into(),
        recorded(EntryKind::Symlink, sha256_hex(b"real"), &["own", "other"]),
    );
    save_manifest(&state_at(state.root()), &seeded).expect("seed the state dir");

    let desired = BTreeMap::from([(
        Utf8PathBuf::from("pivot/x.txt"),
        Entry::File {
            contents: b"aliased".to_vec(),
            executable: false,
        },
    )]);
    let (manifest, plan) = plan_for(&dest, &state, "own", &desired, DriftPolicy::Refuse);
    assert_eq!(
        plan.actions.get(Utf8Path::new("pivot")),
        Some(&Action::Release)
    );

    std::os::unix::fs::symlink("real", dest.path("pivot")).expect("the link reappears");

    let stopped = stopping_at(&dest, &state, &manifest, &plan);

    match stopped.stopped.error() {
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment => {
            assert_eq!(paths_of(refused), BTreeSet::from(["pivot/x.txt".into()]))
        }
        other => panic!("expected Containment, got {other:?}"),
    }
    assert!(stopped.applied_anything());
    assert_eq!(
        stopped.applied.report.rows[Utf8Path::new("pivot")].verdict,
        ApplyOutcome::Released
    );
    assert_eq!(
        stopped.applied.manifest.entries[Utf8Path::new("pivot")].owners,
        BTreeSet::from(["other".to_owned()])
    );
    assert_tree(
        dest.root(),
        &Tree::new().dir("real").symlink("pivot", "real"),
    );
    assert!(stopped.stopped.recorded());
    assert!(matches!(stopped.stopped, Stopped::Applying(_)));
    let after = persisted(&state);
    assert_eq!(
        after.entries[Utf8Path::new("pivot")].owners,
        BTreeSet::from(["other".to_owned()])
    );
    assert!(!after.entries.contains_key(Utf8Path::new("pivot/x.txt")));
}

#[test]
fn a_run_that_could_not_record_what_it_applied_leaves_foreign_paths() {
    let (dest, state) = fixtures();
    let desired = BTreeMap::from([(
        Utf8PathBuf::from("bin/tool"),
        Entry::File {
            contents: b"tool".to_vec(),
            executable: false,
        },
    )]);
    let (manifest, plan) = plan_for(&dest, &state, "own", &desired, DriftPolicy::Refuse);
    let (dest_dir, state_dir) = (dir_at(dest.root()), state_at(state.root()));
    std::fs::remove_dir_all(state.root()).expect("the state directory goes");

    let stopped =
        apply(&dest_dir, &state_dir, &manifest, &plan).expect_err("the manifest cannot be written");

    assert!(matches!(stopped.stopped, Stopped::Recording(_)));
    assert!(!stopped.stopped.recorded());
    match stopped.stopped.recording() {
        Some(Error::Io { path, .. }) => assert_eq!(*path, state.path(MANIFEST_FILE_NAME)),
        other => panic!("expected an I/O error naming the manifest, got {other:?}"),
    }
    assert_eq!(
        stopped.applied.report.rows[Utf8Path::new("bin/tool")].verdict,
        ApplyOutcome::Written
    );
    assert!(
        stopped
            .to_string()
            .starts_with("the run applied 1 path and could not record them"),
        "{stopped}"
    );
    assert_tree(
        dest.root(),
        &Tree::new().dir("bin").file("bin/tool", "tool"),
    );

    std::fs::create_dir_all(state.root()).expect("a state directory again");
    let (_, next) = plan_for(&dest, &state, "own", &desired, DriftPolicy::Refuse);
    assert_eq!(
        next.actions.get(Utf8Path::new("bin/tool")),
        Some(&Action::Refuse {
            refusal: Refusal::Foreign
        })
    );
}

#[test]
fn a_run_that_stopped_part_way_and_could_not_record_it_keeps_both_errors() {
    let (dest, state) = fixtures();
    Tree::new().dir("real").write_under(dest.root());
    let mut seeded = Manifest::new();
    seeded.entries.insert(
        "pivot".into(),
        recorded(EntryKind::Symlink, sha256_hex(b"real"), &["own", "other"]),
    );
    save_manifest(&state_at(state.root()), &seeded).expect("seed the state dir");
    let desired = BTreeMap::from([(
        Utf8PathBuf::from("pivot/x.txt"),
        Entry::File {
            contents: b"aliased".to_vec(),
            executable: false,
        },
    )]);
    let (manifest, plan) = plan_for(&dest, &state, "own", &desired, DriftPolicy::Refuse);
    let (dest_dir, state_dir) = (dir_at(dest.root()), state_at(state.root()));
    std::os::unix::fs::symlink("real", dest.path("pivot")).expect("the link reappears");
    std::fs::remove_dir_all(state.root()).expect("the state directory goes");

    let stopped =
        apply(&dest_dir, &state_dir, &manifest, &plan).expect_err("the write refuses part-way");

    let Stopped::ApplyingAndRecording {
        applying,
        recording,
    } = &stopped.stopped
    else {
        panic!("expected both halves, got {:?}", stopped.stopped)
    };
    assert!(
        matches!(applying, Error::Refused(refused) if refused.kind() == RefusalKind::Containment)
    );
    assert!(matches!(recording, Error::Io { .. }));
    assert!(!stopped.stopped.recorded());
    assert_eq!(
        stopped.applied.report.rows[Utf8Path::new("pivot")].verdict,
        ApplyOutcome::Released
    );
    assert!(
        stopped
            .to_string()
            .contains("the state directory does not record"),
        "{stopped}"
    );
}

#[test]
fn a_release_drops_the_owner_over_a_drifted_node_without_touching_it() {
    let (dest, state) = fixtures();
    let tree = Tree::new().file("shared.txt", "same bytes");
    pipeline(&dest, &state, "one", &tree.entries(), DriftPolicy::Refuse).expect("owner one");
    pipeline(&dest, &state, "two", &tree.entries(), DriftPolicy::Refuse).expect("owner two");

    let (manifest, plan) = plan_for(&dest, &state, "two", &BTreeMap::new(), DriftPolicy::Refuse);
    assert_eq!(
        plan.actions.get(Utf8Path::new("shared.txt")),
        Some(&Action::Release)
    );
    let edited = Tree::new().file("shared.txt", "edited by somebody");
    edited.write_under(dest.root());

    let report = apply_at(&dest, &state, &manifest, &plan).expect("the owner departs regardless");

    assert_eq!(
        verdicts(&report),
        BTreeMap::from([("shared.txt".into(), ApplyOutcome::Released)])
    );
    assert_eq!(
        report.manifest.entries[Utf8Path::new("shared.txt")].owners,
        BTreeSet::from(["one".to_owned()])
    );
    assert_tree(dest.root(), &edited);
}

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
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::new(),
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
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment => assert_eq!(
            refused.to_string(),
            "refusing paths that violate containment: pivot/x (below the symlink pivot)"
        ),
        other => panic!("expected Containment, got {other:?}"),
    }
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
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::new(),
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
        verdicts(&report),
        BTreeMap::from([("logs/x.txt".into(), ApplyOutcome::Removed)])
    );
    assert_tree(dest.root(), &Tree::new().symlink("logs", "real"));
}

#[test]
fn deciding_aims_that_removal_at_the_node_the_walk_resolves_to() {
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
    save_manifest(&state_at(state.root()), &manifest).expect("seed the manifest");

    let (loaded, plan) = plan_for(&dest, &state, "own", &BTreeMap::new(), DriftPolicy::Refuse);

    assert_eq!(
        plan.actions.get(Utf8Path::new("logs/x.txt")),
        Some(&Action::Remove {
            expected: Some(NodeSignature {
                kind: EntryKind::File,
                hash: sha256_hex(b"bytes"),
                executable: false,
            }),
        })
    );
    let report = apply_at(&dest, &state, &loaded, &plan).expect("the decided removal applies");

    assert_eq!(
        verdicts(&report),
        BTreeMap::from([
            ("logs".into(), ApplyOutcome::Removed),
            ("logs/x.txt".into(), ApplyOutcome::Removed),
        ])
    );
    assert_tree(dest.root(), &Tree::new());
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
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::new(),
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
        Error::Refused(refused) if refused.kind() == RefusalKind::Drift => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["logs".into()]))
        }
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
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::new(),
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
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["logs/x.txt".into()]))
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
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::new(),
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
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["l1/x.txt".into()]))
        }
        other => panic!("expected Containment, got {other:?}"),
    }
}

// --- symlinks: creation, replacement, transitions ---

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

    assert_tree(dest.root(), &tree);
    assert_eq!(
        report.report.rows[Utf8Path::new("latest")].verdict,
        ApplyOutcome::Written
    );
    assert_eq!(
        facts_at(&report, "nested/up"),
        &PathFacts {
            shape: Some(PathShape::Symlink {
                target: Some("../notes/a.txt".to_owned()),
            }),
            owners: BTreeSet::from(["own".to_owned()]),
            origin: Some(Origin::Caller),
        }
    );
    let entry = &report.manifest.entries[Utf8Path::new("nested/up")];
    assert_eq!(entry.kind, EntryKind::Symlink);
    assert!(!entry.executable);
    assert_eq!(entry.hash, sha256_hex(b"../notes/a.txt"));
}

#[test]
fn re_applying_an_unchanged_link_reports_the_target_it_left_in_place() {
    let (dest, state) = fixtures();
    let tree = Tree::new()
        .file("notes/a.txt", "alpha")
        .symlink("latest", "notes/a.txt");
    pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse).expect("first apply");

    let report =
        pipeline(&dest, &state, "own", &tree.entries(), DriftPolicy::Refuse).expect("second apply");

    assert_eq!(
        verdicts(&report),
        BTreeMap::from([
            ("latest".into(), ApplyOutcome::Skipped),
            ("notes/a.txt".into(), ApplyOutcome::Skipped),
        ])
    );
    assert_eq!(
        facts_at(&report, "latest"),
        &PathFacts {
            shape: Some(PathShape::Symlink {
                target: Some("notes/a.txt".to_owned()),
            }),
            owners: BTreeSet::from(["own".to_owned()]),
            origin: Some(Origin::Caller),
        }
    );
}

#[test]
fn a_sourced_key_reports_its_source_at_the_path_the_action_lands_on() {
    let (dest, state) = fixtures();
    let desired = Desired::from_source(
        BTreeMap::from([(
            "a/../b.txt".into(),
            Entry::File {
                contents: b"alpha".to_vec(),
                executable: false,
            },
        )]),
        Origin::Files,
    );
    let dest_dir = dir_at(dest.root());
    let state_dir = state_at(state.root());
    let manifest = load_manifest(&state_dir).expect("load manifest");
    let observations =
        observe(&dest_dir, &manifest, &block_markers(&desired)).expect("observe destination");
    let plan = decide(
        "own",
        &desired,
        &manifest,
        &observations,
        None,
        PlanOptions::default(),
    )
    .expect("decide");

    let report = apply(&dest_dir, &state_dir, &manifest, &plan).expect("apply");

    assert_eq!(
        verdicts(&report),
        BTreeMap::from([("b.txt".into(), ApplyOutcome::Written)])
    );
    assert_eq!(facts_at(&report, "b.txt").origin, Some(Origin::Files));
}

#[test]
fn a_target_that_is_not_a_pathname_fails_up_front_and_writes_nothing() {
    let (dest, state) = fixtures();
    let plan = Plan {
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::new(),
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
        Error::Refused(refused) => assert_eq!(
            sourced_of(&refused),
            BTreeMap::from([(
                Utf8PathBuf::from("z-link"),
                (
                    Refusal::InvalidTarget {
                        target: String::new()
                    },
                    Origin::Caller
                )
            )])
        ),
        other => panic!("expected InvalidTarget, got {other:?}"),
    }
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
        report.report.rows[Utf8Path::new("current")].verdict,
        ApplyOutcome::Overwritten
    );
    assert_tree(dest.root(), &v2);
    assert_eq!(
        report.manifest.entries[Utf8Path::new("current")].hash,
        sha256_hex(b"v2.txt")
    );
}

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
        report.report.rows[Utf8Path::new("here")].verdict,
        ApplyOutcome::Overwritten
    );
    assert_tree(dest.root(), &linked);
    assert_eq!(
        report.manifest.entries[Utf8Path::new("here")].kind,
        EntryKind::Symlink
    );

    // link → file
    let report =
        pipeline(&dest, &state, "own", &files.entries(), DriftPolicy::Refuse).expect("link → file");
    assert_eq!(
        report.report.rows[Utf8Path::new("here")].verdict,
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
        Error::Refused(refused) if refused.kind() == RefusalKind::Drift => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["current".into()]))
        }
        other => panic!("expected Drift, got {other:?}"),
    }
    assert_tree(
        dest.root(),
        &Tree::new().symlink("current", "elsewhere.txt"),
    );

    pipeline(&dest, &state, "own", &v2.entries(), DriftPolicy::Overwrite).expect("--force");
    assert_tree(dest.root(), &v2);
}

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
        Error::Refused(refused) => assert_eq!(
            sourced_of(&refused),
            BTreeMap::from([
                (
                    Utf8PathBuf::from("absolute"),
                    (
                        Refusal::ExternalTarget {
                            target: "/etc/hosts".to_owned()
                        },
                        Origin::Caller
                    ),
                ),
                (
                    Utf8PathBuf::from("escape"),
                    (
                        Refusal::ExternalTarget {
                            target: "../outside".to_owned()
                        },
                        Origin::Caller
                    ),
                ),
            ])
        ),
        other => panic!("expected ExternalTarget, got {other:?}"),
    }
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
        Error::Refused(refused) if refused.kind() == RefusalKind::TreeConflict => {
            assert_eq!(
                paths_of(&refused),
                BTreeSet::from(["out".into(), "out/x.txt".into()])
            )
        }
        other => panic!("expected TreeConflict, got {other:?}"),
    }
    assert_tree(dest.root(), &link);
}

#[test]
fn a_target_escaping_through_a_pivot_link_refuses_without_the_permission() {
    let (dest, state) = fixtures();
    let pivot = Tree::new().symlink("pivot", "/etc");
    pivot.write_under(dest.root());
    let desired = Tree::new().symlink("evil", "pivot/passwd").entries();

    let error = pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse)
        .expect_err("a pointer through a pivot reaches /etc/passwd");

    match error {
        Error::Refused(refused) => assert_eq!(
            sourced_of(&refused),
            BTreeMap::from([(
                Utf8PathBuf::from("evil"),
                (
                    Refusal::ExternalTarget {
                        target: "pivot/passwd".to_owned()
                    },
                    Origin::Caller
                )
            )])
        ),
        other => panic!("expected ExternalTarget, got {other:?}"),
    }
    assert_tree(dest.root(), &pivot);

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
        Error::Refused(refused) => assert_eq!(
            sourced_of(&refused),
            BTreeMap::from([(
                Utf8PathBuf::from("rc"),
                (
                    Refusal::ExternalTarget {
                        target: "pivot/rc".to_owned()
                    },
                    Origin::Caller
                )
            )])
        ),
        other => panic!("expected ExternalTarget, got {other:?}"),
    }
    assert_tree(
        dest.root(),
        &Tree::new().dir("real").symlink("pivot", "/etc"),
    );
}

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
        Error::Refused(refused) => assert_eq!(
            sourced_of(&refused),
            BTreeMap::from([(
                Utf8PathBuf::from("a"),
                (
                    Refusal::ExternalTarget {
                        target: "b/../escape".to_owned()
                    },
                    Origin::Caller
                )
            )])
        ),
        other => panic!("expected ExternalTarget, got {other:?}"),
    }
    assert_tree(dest.root(), &Tree::new());
}

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

    // The gap: the pivot is edited, so its overwrite refuses.
    Tree::new()
        .symlink("pivot", "/tmp")
        .write_under(dest.root());
    let error = apply_at(&dest, &state, &manifest, &plan).expect_err("the pivot drifted");

    match error {
        Error::Refused(refused) if refused.kind() == RefusalKind::Drift => assert_eq!(
            paths_of(&refused),
            BTreeSet::from([Utf8PathBuf::from("pivot")])
        ),
        other => panic!("expected Drift, got {other:?}"),
    }
    assert_tree(dest.root(), &Tree::new().symlink("pivot", "/tmp"));
}

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

    // The gap: the bottom pivot is edited, so its overwrite refuses.
    Tree::new()
        .symlink("c/c", "../elsewhere")
        .write_under(dest.root());
    let error = apply_at(&dest, &state, &manifest, &plan).expect_err("the deepest pivot drifted");

    match error {
        Error::Refused(refused) if refused.kind() == RefusalKind::Drift => assert_eq!(
            paths_of(&refused),
            BTreeSet::from([Utf8PathBuf::from("c/c")])
        ),
        other => panic!("expected Drift, got {other:?}"),
    }
    assert_tree(
        dest.root(),
        &Tree::new()
            .dir("dir")
            .symlink("b", "dir")
            .symlink("c/c", "../elsewhere"),
    );
}

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
        Error::Refused(refused) => assert_eq!(
            sourced_of(&refused),
            BTreeMap::from([(
                Utf8PathBuf::from("rc"),
                (
                    Refusal::ExternalTarget {
                        target: "pivot/x".to_owned()
                    },
                    Origin::Caller
                )
            )])
        ),
        other => panic!("expected ExternalTarget, got {other:?}"),
    }
}

#[test]
fn a_refused_ancestor_is_attributed_to_the_planned_key_beneath_it() {
    let mapping = Origin::Mapping {
        path: "/maps/deploy.toml".into(),
    };
    let (dest, state) = fixtures();
    let desired = Tree::new().file("dir/file", "one").entries();
    let (manifest, plan) = plan_from(&dest, &state, "own", &desired, mapping.clone());
    assert!(matches!(
        plan.actions[Utf8Path::new("dir/file")],
        Action::Write { .. }
    ));
    // The gap: `dir` appears as a file nobody recorded.
    Tree::new().file("dir", "squatter").write_under(dest.root());

    let error = apply_at(&dest, &state, &manifest, &plan).expect_err("the ancestor is foreign");

    assert_eq!(
        error.to_string(),
        "refusing to touch foreign paths (not written by this projection): \
         dir (from mapping /maps/deploy.toml); no flag overrides this: remove the paths by \
         hand to let the projection write them — for a block, the marker region rather than \
         the container holding it"
    );
    match error {
        Error::Refused(refused) => assert_eq!(
            sourced_of(&refused),
            BTreeMap::from([("dir".into(), (Refusal::Foreign, mapping))])
        ),
        other => panic!("expected Foreign, got {other:?}"),
    }
    assert_tree(dest.root(), &Tree::new().file("dir", "squatter"));
}

#[test]
fn an_apply_time_refusal_names_the_plans_source_for_its_key() {
    let mapping = || Origin::Mapping {
        path: "/maps/deploy.toml".into(),
    };

    let (dest, state) = fixtures();
    Tree::new()
        .dir("real")
        .symlink("pivot", "real")
        .write_under(dest.root());
    let desired = Tree::new().symlink("rc", "pivot/rc").entries();
    let (manifest, plan) = plan_from(&dest, &state, "own", &desired, mapping());
    Tree::new()
        .symlink("pivot", "/etc")
        .write_under(dest.root());

    let error = apply_at(&dest, &state, &manifest, &plan).expect_err("the pointer escapes");
    match &error {
        Error::Refused(refused) => assert_eq!(
            sourced_of(refused),
            BTreeMap::from([(
                Utf8PathBuf::from("rc"),
                (
                    Refusal::ExternalTarget {
                        target: "pivot/rc".to_owned()
                    },
                    mapping()
                ),
            )])
        ),
        other => panic!("expected ExternalTarget, got {other:?}"),
    }
    assert_eq!(
        error.to_string(),
        "refusing symlinks with targets outside the destination: \
         rc -> pivot/rc (from mapping /maps/deploy.toml); pass --allow-external-targets to \
         write them"
    );

    let (dest, state) = fixtures();
    Tree::new()
        .dir("real")
        .symlink("pivot", "real")
        .write_under(dest.root());
    let desired = Tree::new().symlink("rc", "pivot/x").entries();
    pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse).expect("the chain lands in dest");
    let (manifest, plan) = plan_from(&dest, &state, "own", &desired, mapping());
    assert!(
        matches!(plan.actions[Utf8Path::new("rc")], Action::Skip { .. }),
        "an unchanged link is skipped"
    );
    Tree::new()
        .symlink("pivot", "/etc")
        .write_under(dest.root());

    let error = apply_at(&dest, &state, &manifest, &plan).expect_err("the skipped link escapes");
    match &error {
        Error::Refused(refused) => assert_eq!(
            sourced_of(refused),
            BTreeMap::from([(
                Utf8PathBuf::from("rc"),
                (
                    Refusal::ExternalTarget {
                        target: "pivot/x".to_owned()
                    },
                    mapping()
                ),
            )])
        ),
        other => panic!("expected ExternalTarget, got {other:?}"),
    }
    assert_eq!(
        error.to_string(),
        "refusing symlinks with targets outside the destination: \
         rc -> pivot/x (from mapping /maps/deploy.toml); pass --allow-external-targets to \
         write them"
    );
}

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
    save_manifest(&state_at(state.root()), &manifest).expect("seed the state dir");

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
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["logs/x.txt".into()]))
        }
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
    save_manifest(&state_at(state.root()), &manifest).expect("seed the state dir");

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
        verdicts(&report),
        BTreeMap::from([
            ("logs".into(), ApplyOutcome::Removed),
            ("logs/x.txt".into(), ApplyOutcome::Written),
        ])
    );
    assert_tree(dest.root(), &replaced);
}

// --- blocks: splicing a region into somebody else's file ---

const MARKER: &str = "# proiectio";

fn block_kind(placement: Placement) -> EntryKind {
    EntryKind::Block {
        marker: MARKER.to_owned(),
        placement,
    }
}

fn block(body: &str, placement: Placement) -> Entry {
    Entry::Block {
        body: body.as_bytes().to_vec(),
        marker: MARKER.to_owned(),
        placement,
    }
}

fn block_tree(path: &str, body: &str, placement: Placement) -> BTreeMap<Utf8PathBuf, Entry> {
    BTreeMap::from([(Utf8PathBuf::from(path), block(body, placement))])
}

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
        verdicts(&report),
        BTreeMap::from([("rc".into(), ApplyOutcome::Written)])
    );
    assert_eq!(
        container(&dest, "rc"),
        "author line\nsecond\n# proiectio\nmanaged\n"
    );
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
        verdicts(&report),
        BTreeMap::from([("rc".into(), ApplyOutcome::Skipped)])
    );
    assert_eq!(container(&dest, "rc"), after_first);
}

#[test]
fn an_edit_outside_the_region_is_not_drift_and_an_edit_inside_it_is() {
    let (dest, state) = fixtures();
    Tree::new().file("rc", "author\n").write_under(dest.root());
    let desired = block_tree("rc", "managed\n", Placement::Append);
    pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse).expect("project");

    fs::write(dest.path("rc"), "author\nand more\n# proiectio\nmanaged\n").expect("edit outside");
    let report = pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse)
        .expect("outside is not drift");
    assert_eq!(
        verdicts(&report),
        BTreeMap::from([("rc".into(), ApplyOutcome::Skipped)])
    );
    assert_eq!(
        container(&dest, "rc"),
        "author\nand more\n# proiectio\nmanaged\n"
    );

    fs::write(dest.path("rc"), "author\nand more\n# proiectio\nedited\n").expect("edit inside");
    let error = pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse)
        .expect_err("inside the region is drift");
    match error {
        Error::Refused(refused) if refused.kind() == RefusalKind::Drift => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["rc".into()]))
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn an_edit_past_the_regions_outer_edge_is_drift() {
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
        Error::Refused(refused) if refused.kind() == RefusalKind::Drift => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["rc".into()]))
        }
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
        verdicts(&report),
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
        verdicts(&report),
        BTreeMap::from([("rc".into(), ApplyOutcome::Overwritten)])
    );
    assert_eq!(container(&dest, "rc"), "author\n# renamed\nmanaged\n");
}

#[test]
fn a_migration_into_a_marker_the_author_already_wrote_refuses() {
    let (dest, state) = fixtures();
    let author = "author\n# renamed\n";
    Tree::new().file("rc", author).write_under(dest.root());
    pipeline(
        &dest,
        &state,
        "own",
        &block_tree("rc", "managed\n", Placement::Append),
        DriftPolicy::Refuse,
    )
    .expect("project under the first marker");
    let before = container(&dest, "rc");

    let renamed = BTreeMap::from([(
        Utf8PathBuf::from("rc"),
        Entry::Block {
            body: b"managed\n".to_vec(),
            marker: "# renamed".to_owned(),
            placement: Placement::Append,
        },
    )]);
    let error = pipeline(&dest, &state, "own", &renamed, DriftPolicy::Refuse)
        .expect_err("the author's side already carries the new marker");

    match error {
        Error::Refused(refused) => assert_eq!(
            refusals_of(&refused),
            BTreeMap::from([(
                "rc".into(),
                Refusal::Block {
                    fault: BlockFault::MarkerInAuthorText
                }
            )])
        ),
        other => panic!("expected Block, got {other:?}"),
    }
    assert_eq!(container(&dest, "rc"), before);
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
            let state_dir = state_at(state.root());
            let manifest = load_manifest(&state_dir).expect("load manifest");
            let observations =
                observe(&dest_dir, &manifest, &BlockMarkers::new()).expect("observe");
            let plan = decide_removal(
                "own",
                RemovalScope::Everything,
                &manifest,
                &observations,
                None,
                DriftPolicy::Refuse,
            );
            (manifest, plan)
        };
        let report = apply_at(&dest, &state, &manifest, &plan).expect("strip the region");

        assert_eq!(
            verdicts(&report),
            BTreeMap::from([("rc".into(), ApplyOutcome::Removed)])
        );
        assert_eq!(container(&dest, "rc"), author, "{placement:?}");
        assert_eq!(report.manifest, Manifest::new());
    }
}

#[test]
fn a_region_whose_body_already_matches_is_adopted() {
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
        verdicts(&report),
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
        Error::Refused(refused) if refused.kind() == RefusalKind::Foreign => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["rc".into()]))
        }
        other => panic!("expected Foreign, got {other:?}"),
    }
    assert_eq!(container(&dest, "rc"), author);
    assert_eq!(persisted(&state), Manifest::new());
}

const DOUBLED: &str = "# proiectio\nmanaged\nauthor\n# proiectio\nmanaged\n";

#[test]
fn a_duplicate_marker_in_the_gap_refuses_every_action_on_the_region() {
    let desired = block_tree("rc", "managed\n", Placement::Append);
    let plans: [(&str, BTreeMap<Utf8PathBuf, Entry>); 4] = [
        ("remove", BTreeMap::new()),
        (
            "overwrite",
            block_tree("rc", "different\n", Placement::Append),
        ),
        (
            "migration",
            BTreeMap::from([(
                Utf8PathBuf::from("rc"),
                Entry::Block {
                    body: b"managed\n".to_vec(),
                    marker: "# renamed".to_owned(),
                    placement: Placement::Append,
                },
            )]),
        ),
        ("skip", desired.clone()),
    ];
    for (name, second) in plans {
        let (dest, state) = fixtures();
        Tree::new().file("rc", "author\n").write_under(dest.root());
        pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse).expect("project");

        let (manifest, plan) = plan_for(&dest, &state, "own", &second, DriftPolicy::Refuse);
        fs::write(dest.path("rc"), DOUBLED).expect("duplicate the region in the gap");

        let error =
            apply_at(&dest, &state, &manifest, &plan).expect_err("the marker identifies no region");

        match error {
            Error::Refused(refused) if refused.kind() == RefusalKind::Drift => {
                assert_eq!(paths_of(&refused), BTreeSet::from(["rc".into()]), "{name}")
            }
            other => panic!("{name}: expected Drift, got {other:?}"),
        }
        assert_eq!(container(&dest, "rc"), DOUBLED, "{name}");
        assert_eq!(persisted(&state), manifest, "{name}");
    }
}

#[test]
fn a_recorded_region_back_in_the_gap_refuses_even_where_it_matches() {
    let (dest, state) = fixtures();
    Tree::new().file("rc", "author\n").write_under(dest.root());
    let desired = block_tree("rc", "managed\n", Placement::Append);
    pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse).expect("project");

    fs::write(dest.path("rc"), "author\n").expect("strip the region");
    let (manifest, plan) = plan_for(&dest, &state, "own", &desired, DriftPolicy::Refuse);
    // The gap: it comes back, byte for byte.
    let restored = "author\n# proiectio\nmanaged\n";
    fs::write(dest.path("rc"), restored).expect("restore in the gap");

    let error =
        apply_at(&dest, &state, &manifest, &plan).expect_err("the region came back since the plan");

    match error {
        Error::Refused(refused) if refused.kind() == RefusalKind::Drift => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["rc".into()]))
        }
        other => panic!("expected Drift, got {other:?}"),
    }
    assert_eq!(container(&dest, "rc"), restored);
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
        Error::Refused(refused) if refused.kind() == RefusalKind::Foreign => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["rc".into()]))
        }
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
        Error::Refused(refused) => assert_eq!(
            refusals_of(&refused),
            BTreeMap::from([(
                "etc/rc".into(),
                Refusal::Block {
                    fault: BlockFault::ContainerMissing
                }
            )])
        ),
        other => panic!("expected Block, got {other:?}"),
    }
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
        Error::Refused(refused) => assert_eq!(
            refusals_of(&refused),
            BTreeMap::from([(
                "rc".into(),
                Refusal::Block {
                    fault: BlockFault::ContainerNotNewlineTerminated
                }
            )])
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

    let unrecorded = pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse)
        .expect_err("a link is not a container");
    match unrecorded {
        Error::Refused(refused) if refused.kind() == RefusalKind::Foreign => {
            assert!(paths_of(&refused).contains(Utf8Path::new("rc")))
        }
        other => panic!("expected Foreign, got {other:?}"),
    }

    let mut manifest = Manifest::new();
    manifest.entries.insert(
        "rc".into(),
        recorded(
            block_kind(Placement::Append),
            sha256_hex(b"managed\n"),
            &["own"],
        ),
    );
    save_manifest(&state_at(state.root()), &manifest).expect("record the region");
    let recorded_error = pipeline(&dest, &state, "own", &desired, DriftPolicy::Overwrite)
        .expect_err("a swapped container is drift");
    match recorded_error {
        Error::Refused(refused) if refused.kind() == RefusalKind::Drift => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["rc".into()]))
        }
        other => panic!("expected Drift, got {other:?}"),
    }
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
        Error::Refused(refused) if refused.kind() == RefusalKind::Drift => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["rc".into()]))
        }
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
        verdicts(&report),
        BTreeMap::from([("rc".into(), ApplyOutcome::Overwritten)])
    );
    assert_eq!(container(&dest, "rc"), "author edited\n# proiectio\nv2\n");
}

#[test]
fn a_deleted_region_is_spliced_back_and_a_deleted_container_refuses() {
    let (dest, state) = fixtures();
    Tree::new().file("rc", "author\n").write_under(dest.root());
    let desired = block_tree("rc", "managed\n", Placement::Append);
    pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse).expect("project");

    fs::write(dest.path("rc"), "author\n").expect("delete the region");
    let healed =
        pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse).expect("write heals");
    assert_eq!(
        verdicts(&healed),
        BTreeMap::from([("rc".into(), ApplyOutcome::Written)])
    );
    assert_eq!(container(&dest, "rc"), "author\n# proiectio\nmanaged\n");

    fs::remove_file(dest.path("rc")).expect("delete the container");
    let error = pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse)
        .expect_err("a block never creates its container");
    match error {
        Error::Refused(refused) => assert_eq!(
            refusals_of(&refused),
            BTreeMap::from([(
                "rc".into(),
                Refusal::Block {
                    fault: BlockFault::ContainerMissing
                }
            )])
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
    assert!(!persisted(&state).entries[Utf8Path::new("rc")].executable);
}

#[test]
fn a_containers_setuid_bits_do_not_survive_the_rename() {
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
    // conda substitutes its block for a literal sentinel and corrupts rc files
    // containing that string; a byte-range splice has nothing to substitute.
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
        let state_dir = state_at(state.root());
        let manifest = load_manifest(&state_dir).expect("load manifest");
        let observations = observe(&dest_dir, &manifest, &BlockMarkers::new()).expect("observe");
        let plan = decide_removal(
            "own",
            RemovalScope::Everything,
            &manifest,
            &observations,
            None,
            DriftPolicy::Refuse,
        );
        (manifest, plan)
    };
    apply_at(&dest, &state, &manifest, &plan).expect("strip the region");

    assert_eq!(container(&dest, "rc"), author);
}

#[test]
fn a_marker_terminated_by_end_of_file_still_reads_and_strips() {
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
        verdicts(&report),
        BTreeMap::from([("rc".into(), ApplyOutcome::Skipped)])
    );
}

#[test]
fn two_owners_share_a_region_only_while_agreeing_on_the_marker() {
    let (dest, state) = fixtures();
    Tree::new().file("rc", "author\n").write_under(dest.root());
    let desired = block_tree("rc", "managed\n", Placement::Append);
    pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse).expect("project as own");

    let joined = pipeline(&dest, &state, "other", &desired, DriftPolicy::Refuse)
        .expect("identical entries share a path");
    assert_eq!(
        joined.manifest.entries[Utf8Path::new("rc")].owners,
        BTreeSet::from(["other".to_owned(), "own".to_owned()])
    );

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
        Error::Refused(refused) => {
            assert_eq!(
                refusals_of(&refused)[Utf8Path::new("rc")],
                Refusal::OwnerConflict {
                    owners: BTreeSet::from(["other".to_owned(), "own".to_owned()])
                }
            );
        }
        other => panic!("expected OwnerConflict, got {other:?}"),
    }
}

#[test]
fn a_second_marker_line_past_the_edge_refuses_and_strands_nothing() {
    for theirs in ["theirs\n", "managed\n"] {
        let (dest, state) = fixtures();
        Tree::new().file("rc", "author\n").write_under(dest.root());
        let desired = block_tree("rc", "managed\n", Placement::Append);
        pipeline(&dest, &state, "own", &desired, DriftPolicy::Refuse).expect("project");
        let edited = format!("author\n# proiectio\nmanaged\n# proiectio\n{theirs}");
        fs::write(dest.path("rc"), &edited).expect("append a second marker line past the edge");

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
            let state_dir = state_at(state.root());
            let manifest = load_manifest(&state_dir).expect("load manifest");
            let observations =
                observe(&dest_dir, &manifest, &BlockMarkers::new()).expect("observe");
            let plan = decide_removal(
                "own",
                RemovalScope::Everything,
                &manifest,
                &observations,
                None,
                DriftPolicy::Overwrite,
            );
            (manifest, plan)
        };
        errors.push(apply_at(&dest, &state, &manifest, &plan).expect_err("nor does the removal"));

        for error in errors {
            match error {
                Error::Refused(refused) if refused.kind() == RefusalKind::Drift => {
                    assert_eq!(paths_of(&refused), BTreeSet::from(["rc".into()]))
                }
                other => panic!("expected Drift, got {other:?}"),
            }
        }
        assert_eq!(container(&dest, "rc"), edited);
        assert!(persisted(&state).entries.contains_key(Utf8Path::new("rc")));
    }
}

#[test]
fn a_hand_built_plan_expecting_another_marker_fails_up_front() {
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
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::new(),
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
        Error::Refused(refused) => assert_eq!(
            refusals_of(&refused),
            BTreeMap::from([(
                "rc".into(),
                Refusal::Block {
                    fault: BlockFault::SignatureNotRecorded
                }
            )])
        ),
        other => panic!("expected Block, got {other:?}"),
    }
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

    let restored = "author\n# proiectio\nmanaged\n";
    fs::write(dest.path("rc"), restored).expect("restore the old region");

    let error =
        apply_at(&dest, &state, &manifest, &plan).expect_err("the region changed under the plan");
    match error {
        Error::Refused(refused) if refused.kind() == RefusalKind::Drift => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["rc".into()]))
        }
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

    let v2 = Tree::new().file("m.txt", "forced");
    let report = pipeline(&dest, &state, "own", &v2.entries(), DriftPolicy::Overwrite)
        .expect("--force overwrites the edit");
    assert_eq!(
        verdicts(&report),
        BTreeMap::from([("m.txt".into(), ApplyOutcome::Overwritten)])
    );
    assert_tree(dest.root(), &v2);

    fs::write(dest.path("m.txt"), "another edit").expect("drift again");
    let (manifest, plan) = plan_for(&dest, &state, "own", &v1.entries(), DriftPolicy::Overwrite);
    fs::write(dest.path("m.txt"), "yet another edit").expect("edit in the gap");
    let error = apply_at(&dest, &state, &manifest, &plan).expect_err("the gap re-check holds");
    match error {
        Error::Refused(refused) if refused.kind() == RefusalKind::Drift => {
            assert_eq!(paths_of(&refused), BTreeSet::from(["m.txt".into()]))
        }
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
    match load_manifest(&state_at(state.root())) {
        Err(Error::ManifestFormat { path, .. }) => {
            assert_eq!(path, state.path(MANIFEST_FILE_NAME))
        }
        other => panic!("expected ManifestFormat, got {other:?}"),
    }

    fs::write(
        state.path(MANIFEST_FILE_NAME),
        r#"{"version": 9, "entries": {}}"#,
    )
    .expect("write a future version");
    match load_manifest(&state_at(state.root())) {
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

    save_manifest(&state_at(state.root()), &manifest).expect("save");

    assert_eq!(persisted(&state), manifest);
    assert_eq!(names_in(&state), vec![MANIFEST_FILE_NAME.to_owned()]);
}

// --- plan/apply parity ---

fn plan_result(
    dest: &Fixture,
    state: &Fixture,
    desired: &BTreeMap<Utf8PathBuf, Entry>,
) -> (Manifest, Result<Plan>) {
    let dest_dir = dir_at(dest.root());
    let state_dir = state_at(state.root());
    let manifest = load_manifest(&state_dir).expect("load manifest");
    let desired = Desired::from_caller(desired.clone());
    let observations =
        observe(&dest_dir, &manifest, &block_markers(&desired)).expect("observe destination");
    let planned = decide(
        "own",
        &desired,
        &manifest,
        &observations,
        None,
        PlanOptions::default(),
    );
    (manifest, planned)
}

fn assert_plan_and_apply_agree(
    dest: &Fixture,
    state: &Fixture,
    desired: &BTreeMap<Utf8PathBuf, Entry>,
) {
    let (manifest, planned) = plan_result(dest, state, desired);
    let Ok(plan) = planned else {
        return;
    };
    let refused: Vec<&Utf8PathBuf> = plan
        .actions
        .iter()
        .filter(|(_, action)| matches!(action, Action::Refuse { .. }))
        .map(|(path, _)| path)
        .collect();
    if !refused.is_empty() {
        return;
    }
    if let Err(error) = apply_at(dest, state, &manifest, &plan) {
        panic!("deciding planned no refusal over {desired:?}, applying failed: {error}");
    }
}

#[test]
fn deciding_and_applying_agree_over_an_empty_destination() {
    let (dest, state) = fixtures();
    let desired = Tree::new()
        .file("a.txt", "one")
        .file("nested/b.txt", "two")
        .symlink("link", "a.txt")
        .entries();

    assert_plan_and_apply_agree(&dest, &state, &desired);
}

#[test]
fn deciding_and_applying_agree_over_writes_skips_and_removals() {
    let (dest, state) = fixtures();
    let first = Tree::new()
        .file("keep.txt", "same")
        .file("change.txt", "before")
        .file("drop.txt", "gone soon")
        .entries();
    pipeline(&dest, &state, "own", &first, DriftPolicy::Refuse).expect("first projection");

    let second = Tree::new()
        .file("keep.txt", "same")
        .file("change.txt", "after")
        .file("added.txt", "new")
        .entries();

    assert_plan_and_apply_agree(&dest, &state, &second);
}

#[test]
fn deciding_and_applying_agree_over_a_block_into_a_standing_container() {
    let (dest, state) = fixtures();
    Tree::new()
        .file("rc", "author line\n")
        .write_under(dest.root());

    assert_plan_and_apply_agree(
        &dest,
        &state,
        &block_tree("rc", "managed\n", Placement::Append),
    );
}

#[test]
fn deciding_reports_a_desired_path_past_the_walk_depth() {
    let (dest, state) = fixtures();
    let past = format!("{}/leaf", ["d"; MAX_WALK_DEPTH + 1].join("/"));
    let desired = Tree::new().file(&past, "deep").entries();

    let (_, planned) = plan_result(&dest, &state, &desired);

    match planned.expect_err("a path observe could not read back") {
        Error::DestinationTooDeep { path, limit } => {
            assert_eq!(path, ["d"; MAX_WALK_DEPTH + 1].join("/"));
            assert_eq!(limit, MAX_WALK_DEPTH);
        }
        other => panic!("expected DestinationTooDeep, got {other:?}"),
    }
    assert_tree(dest.root(), &Tree::new());
}

#[test]
fn deciding_accepts_a_desired_path_at_the_walk_depth() {
    let (dest, state) = fixtures();
    let at_the_limit = format!("{}/leaf", ["d"; MAX_WALK_DEPTH].join("/"));
    let desired = Tree::new().file(&at_the_limit, "deep").entries();

    assert_plan_and_apply_agree(&dest, &state, &desired);
    assert_eq!(
        fs::read_to_string(dest.path(&at_the_limit)).expect("the deep file"),
        "deep"
    );
}

#[test]
fn deciding_refuses_an_unrecorded_container_that_already_carries_the_marker() {
    let (dest, state) = fixtures();
    Tree::new()
        .file("rc", "author\n# proiectio\nsomebody elses body\n")
        .write_under(dest.root());
    let desired = block_tree("rc", "managed\n", Placement::Append);

    let plan = plan_result(&dest, &state, &desired).1.expect("decide");

    assert_eq!(
        plan.actions.get(Utf8Path::new("rc")),
        Some(&Action::Refuse {
            refusal: Refusal::Foreign
        })
    );
    assert_plan_and_apply_agree(&dest, &state, &desired);
}

#[test]
fn deciding_adopts_an_unrecorded_container_whose_region_already_matches() {
    let (dest, state) = fixtures();
    Tree::new()
        .file("rc", "author\n# proiectio\nmanaged\n")
        .write_under(dest.root());
    let desired = block_tree("rc", "managed\n", Placement::Append);

    let plan = plan_result(&dest, &state, &desired).1.expect("decide");

    assert!(matches!(
        plan.actions.get(Utf8Path::new("rc")),
        Some(Action::Write { .. })
    ));
    assert_plan_and_apply_agree(&dest, &state, &desired);
}

#[test]
fn deciding_refuses_an_append_into_a_container_without_a_trailing_newline() {
    let (dest, state) = fixtures();
    Tree::new().file("rc", "author").write_under(dest.root());
    let desired = block_tree("rc", "managed\n", Placement::Append);

    let plan = plan_result(&dest, &state, &desired).1.expect("decide");

    assert_eq!(
        plan.actions.get(Utf8Path::new("rc")),
        Some(&Action::Refuse {
            refusal: Refusal::Block {
                fault: BlockFault::ContainerNotNewlineTerminated
            }
        })
    );
    assert_plan_and_apply_agree(&dest, &state, &desired);
}

#[test]
fn deciding_refuses_a_migration_into_a_marker_the_author_already_wrote() {
    let (dest, state) = fixtures();
    Tree::new()
        .file("rc", "author\n# renamed\n")
        .write_under(dest.root());
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

    let plan = plan_result(&dest, &state, &renamed).1.expect("decide");

    assert_eq!(
        plan.actions.get(Utf8Path::new("rc")),
        Some(&Action::Refuse {
            refusal: Refusal::Block {
                fault: BlockFault::MarkerInAuthorText
            }
        })
    );
    assert_plan_and_apply_agree(&dest, &state, &renamed);
}
