use super::*;
use crate::{Desired, DesiredRegion, Origin, RefusalKind};

// The owner every test plans for unless it says otherwise.
const OWNER: &str = "site";

fn file(contents: &str, executable: bool) -> Entry {
    Entry::File {
        contents: contents.as_bytes().to_vec(),
        executable,
    }
}

fn link(target: &str) -> Entry {
    Entry::Symlink {
        target: target.to_owned(),
    }
}

fn signature(entry: &Entry) -> NodeSignature {
    NodeSignature {
        kind: entry.kind(),
        hash: desired_hash(entry),
        executable: desired_executable(entry),
        target: match entry {
            Entry::Symlink { target } => Some(target.clone()),
            Entry::File { .. } | Entry::Block { .. } => None,
        },
    }
}

fn recorded(entry: &Entry, owners: &[&str]) -> ManifestEntry {
    ManifestEntry {
        kind: entry.kind(),
        hash: desired_hash(entry),
        executable: desired_executable(entry),
        owners: owners.iter().map(|owner| (*owner).to_owned()).collect(),
    }
}

fn on_disk(entry: &Entry) -> Observation {
    match entry {
        Entry::File {
            contents,
            executable,
        } => Observation::File {
            hash: sha256_hex(contents),
            executable: *executable,
        },
        Entry::Symlink { target } => Observation::Symlink {
            hash: sha256_hex(target.as_bytes()),
            target: Some(target.clone()),
        },
        // The author's side ends at the marker's line start, newline-terminated by construction.
        Entry::Block { body, .. } => Observation::Block {
            hash: Some(sha256_hex(body)),
            newline_terminated: true,
            occurrences: 1,
            desired: None,
        },
    }
}

const MARKER: &str = "# proiectio";

fn block(body: &str, placement: Placement) -> Entry {
    Entry::Block {
        body: body.as_bytes().to_vec(),
        marker: MARKER.to_owned(),
        placement,
    }
}

// A container the region is gone from; `terminated` picks the author's final newline.
fn no_region(newline_terminated: bool) -> Observation {
    Observation::Block {
        hash: None,
        newline_terminated,
        occurrences: 0,
        desired: None,
    }
}

// A region edited on disk, under a container holding the marker on `occurrences` lines.
fn edited_region(body: &str, occurrences: usize) -> Observation {
    Observation::Block {
        hash: Some(sha256_hex(body.as_bytes())),
        newline_terminated: true,
        occurrences,
        desired: None,
    }
}

fn tree(entries: &[(&str, &Entry)]) -> Desired {
    Desired::from_caller(
        entries
            .iter()
            .map(|(path, entry)| (Utf8PathBuf::from(*path), (*entry).clone()))
            .collect(),
    )
}

fn manifest_of(entries: &[(&str, ManifestEntry)]) -> Manifest {
    Manifest {
        version: crate::MANIFEST_VERSION,
        entries: entries
            .iter()
            .map(|(path, entry)| (Utf8PathBuf::from(*path), entry.clone()))
            .collect(),
    }
}

// An ancestor no case declares is an observed directory; a case meaning
// something else there declares it.
fn observed(paths: &[(&str, Observation)]) -> Observations {
    let mut observed: BTreeMap<Utf8PathBuf, Observation> = paths
        .iter()
        .map(|(path, observation)| (Utf8PathBuf::from(*path), observation.clone()))
        .collect();
    for (path, _) in paths {
        for ancestor in Utf8Path::new(path).ancestors().skip(1) {
            if !ancestor.as_str().is_empty() {
                observed
                    .entry(ancestor.to_owned())
                    .or_insert(Observation::Directory);
            }
        }
    }
    Observations {
        paths: observed,
        ..Observations::default()
    }
}

// [`observed`], plus directories holding a name that is not UTF-8.
fn observed_with_unreadable(paths: &[(&str, Observation)], unreadable: &[&str]) -> Observations {
    Observations {
        unreadable: unreadable.iter().map(Utf8PathBuf::from).collect(),
        ..observed(paths)
    }
}

fn plan(
    desired: &Desired,
    manifest: &Manifest,
    observations: &Observations,
    policy: DriftPolicy,
) -> Plan {
    plan_with(
        desired,
        manifest,
        observations,
        PlanOptions {
            drift: policy,
            ..PlanOptions::default()
        },
    )
}

fn plan_with(
    desired: &Desired,
    manifest: &Manifest,
    observations: &Observations,
    options: PlanOptions,
) -> Plan {
    decide(OWNER, desired, manifest, observations, None, options).expect("decide")
}

fn action<'plan>(plan: &'plan Plan, path: &str) -> &'plan Action {
    plan.actions
        .get(Utf8Path::new(path))
        .unwrap_or_else(|| panic!("no action at {path}"))
}

// --- classification (`docs/design.lex` section 2, the state table) ---

#[test]
fn clean_when_disk_matches_the_recorded_entry() {
    let entry = file("alpha\n", false);
    let manifest = manifest_of(&[("a.txt", recorded(&entry, &[OWNER]))]);
    let observations = observed(&[("a.txt", on_disk(&entry))]);

    let status = classify(&manifest, &observations, None);

    assert_eq!(
        status.rows[Utf8Path::new("a.txt")].verdict,
        PathState::Clean
    );
}

#[test]
fn drifted_when_bytes_differ() {
    let manifest = manifest_of(&[("a.txt", recorded(&file("alpha\n", false), &[OWNER]))]);
    let observations = observed(&[("a.txt", on_disk(&file("edited\n", false)))]);

    let status = classify(&manifest, &observations, None);

    assert_eq!(
        status.rows[Utf8Path::new("a.txt")].verdict,
        PathState::Drifted
    );
}

#[test]
fn drifted_when_the_executable_bit_differs() {
    let manifest = manifest_of(&[("run.sh", recorded(&file("#!/bin/sh\n", false), &[OWNER]))]);
    let observations = observed(&[("run.sh", on_disk(&file("#!/bin/sh\n", true)))]);

    let status = classify(&manifest, &observations, None);

    assert_eq!(
        status.rows[Utf8Path::new("run.sh")].verdict,
        PathState::Drifted
    );
}

#[test]
fn drifted_when_the_kind_differs() {
    let manifest = manifest_of(&[("a", recorded(&file("alpha\n", false), &[OWNER]))]);
    let observations = observed(&[("a", on_disk(&link("alpha\n")))]);

    let status = classify(&manifest, &observations, None);

    assert_eq!(status.rows[Utf8Path::new("a")].verdict, PathState::Drifted);
}

#[test]
fn drifted_when_a_recorded_path_is_now_a_directory() {
    let manifest = manifest_of(&[("a", recorded(&file("alpha\n", false), &[OWNER]))]);
    let observations = observed(&[("a", Observation::Directory)]);

    let status = classify(&manifest, &observations, None);

    assert_eq!(status.rows[Utf8Path::new("a")].verdict, PathState::Drifted);
}

#[test]
fn missing_when_a_recorded_path_is_gone() {
    let manifest = manifest_of(&[("a.txt", recorded(&file("alpha\n", false), &[OWNER]))]);
    let observations = observed(&[("a.txt", Observation::Absent)]);

    let status = classify(&manifest, &observations, None);

    assert_eq!(
        status.rows[Utf8Path::new("a.txt")].verdict,
        PathState::Missing
    );
}

#[test]
fn missing_when_the_snapshot_lacks_a_recorded_path() {
    let manifest = manifest_of(&[("a.txt", recorded(&file("alpha\n", false), &[OWNER]))]);
    let observations = observed(&[]);

    let status = classify(&manifest, &observations, None);

    assert_eq!(
        status.rows[Utf8Path::new("a.txt")].verdict,
        PathState::Missing
    );
}

#[test]
fn foreign_when_on_disk_and_unrecorded() {
    let manifest = Manifest::new();
    let observations = observed(&[("notes.txt", on_disk(&file("mine\n", false)))]);

    let status = classify(&manifest, &observations, None);

    assert_eq!(
        status.rows[Utf8Path::new("notes.txt")].verdict,
        PathState::Foreign
    );
}

#[test]
fn an_unrecorded_directory_classifies_foreign() {
    let status = classify(
        &Manifest::new(),
        &observed(&[("existing", Observation::Directory)]),
        None,
    );

    assert_eq!(
        status.rows[Utf8Path::new("existing")].verdict,
        PathState::Foreign
    );
}

#[test]
fn an_unrecorded_node_of_another_kind_classifies_foreign() {
    let status = classify(
        &Manifest::new(),
        &observed(&[("pipe", Observation::Other)]),
        None,
    );

    assert_eq!(
        status.rows[Utf8Path::new("pipe")].verdict,
        PathState::Foreign
    );
}

#[test]
fn a_link_with_a_matching_target_classifies_clean_and_a_changed_one_drifted() {
    let entry = link("../shared/rc");
    let manifest = manifest_of(&[
        ("kept", recorded(&entry, &[OWNER])),
        ("moved", recorded(&entry, &[OWNER])),
    ]);
    let observations = observed(&[
        ("kept", on_disk(&entry)),
        ("moved", on_disk(&link("/etc/rc"))),
    ]);

    let status = classify(&manifest, &observations, None);

    assert_eq!(status.rows[Utf8Path::new("kept")].verdict, PathState::Clean);
    assert_eq!(
        status.rows[Utf8Path::new("moved")].verdict,
        PathState::Drifted
    );
}

#[test]
fn a_link_target_edited_to_non_utf8_classifies_drifted() {
    let entry = link("ok");
    let manifest = manifest_of(&[("l", recorded(&entry, &[OWNER]))]);
    let observations = observed(&[(
        "l",
        Observation::Symlink {
            hash: sha256_hex(b"\xff\xfe"),
            target: None,
        },
    )]);

    let status = classify(&manifest, &observations, None);

    assert_eq!(status.rows[Utf8Path::new("l")].verdict, PathState::Drifted);
}

#[test]
fn a_recorded_block_classifies_over_its_region() {
    let entry = block("managed\n", Placement::Append);
    let manifest = manifest_of(&[("conf", recorded(&entry, &[OWNER]))]);
    let cases: &[(Observation, PathState)] = &[
        (on_disk(&entry), PathState::Clean),
        (edited_region("edited\n", 1), PathState::Drifted),
        (no_region(true), PathState::Missing),
        (Observation::Absent, PathState::Missing),
        (Observation::Directory, PathState::Drifted),
    ];
    for (observation, want) in cases {
        let status = classify(&manifest, &observed(&[("conf", observation.clone())]), None);
        assert_eq!(
            status.rows[Utf8Path::new("conf")].verdict,
            *want,
            "{observation:?}"
        );
    }

    let whole = manifest_of(&[("conf", recorded(&file("whole\n", false), &[OWNER]))]);
    for observation in [no_region(true), on_disk(&entry)] {
        let status = classify(&whole, &observed(&[("conf", observation.clone())]), None);
        assert_eq!(
            status.rows[Utf8Path::new("conf")].verdict,
            PathState::Drifted,
            "{observation:?}"
        );
    }
}

#[test]
fn the_state_subtree_never_classifies() {
    let manifest = manifest_of(&[("a.txt", recorded(&file("alpha\n", false), &[OWNER]))]);
    let observations = observed(&[
        ("a.txt", on_disk(&file("alpha\n", false))),
        (".proiectio", Observation::Directory),
        (
            ".proiectio/manifest.json",
            on_disk(&file("{\"version\":1}", false)),
        ),
    ]);

    let status = classify(&manifest, &observations, Some(Utf8Path::new(".proiectio")));

    assert_eq!(
        status.rows.keys().collect::<Vec<_>>(),
        [Utf8Path::new("a.txt")]
    );
}

// --- the action table (`docs/design.lex` section 2), row by row ---

#[test]
fn disk_already_equal_to_desired_skips() {
    let entry = file("alpha\n", false);
    let manifest = manifest_of(&[("a.txt", recorded(&entry, &[OWNER]))]);
    let observations = observed(&[("a.txt", on_disk(&entry))]);

    let plan = plan(
        &tree(&[("a.txt", &entry)]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "a.txt"),
        &Action::Skip {
            entry: entry.clone(),
            expected: signature(&entry),
        }
    );
    assert_eq!(plan.owner, OWNER);
}

#[test]
fn clean_disk_with_changed_desired_overwrites() {
    let old = file("v1\n", false);
    let new = file("v2\n", false);
    let manifest = manifest_of(&[("a.txt", recorded(&old, &[OWNER]))]);
    let observations = observed(&[("a.txt", on_disk(&old))]);

    let plan = plan(
        &tree(&[("a.txt", &new)]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "a.txt"),
        &Action::Overwrite {
            entry: new,
            expected: signature(&old),
            reason: OverwriteReason::ContentChanged,
        }
    );
}

#[test]
fn a_drifted_path_refuses_and_names_it() {
    let manifest = manifest_of(&[("a.txt", recorded(&file("v1\n", false), &[OWNER]))]);
    let observations = observed(&[("a.txt", on_disk(&file("edited\n", false)))]);

    let plan = plan(
        &tree(&[("a.txt", &file("v2\n", false))]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "a.txt"),
        &Action::Refuse {
            refusal: Refusal::Drift,
        }
    );
}

#[test]
fn drift_policy_overwrite_lifts_the_drift_refusal() {
    let drifted = file("edited\n", false);
    let new = file("v2\n", false);
    let manifest = manifest_of(&[("a.txt", recorded(&file("v1\n", false), &[OWNER]))]);
    let observations = observed(&[("a.txt", on_disk(&drifted))]);

    let plan = plan(
        &tree(&[("a.txt", &new)]),
        &manifest,
        &observations,
        DriftPolicy::Overwrite,
    );

    assert_eq!(
        action(&plan, "a.txt"),
        &Action::Overwrite {
            entry: new,
            expected: signature(&drifted),
            reason: OverwriteReason::ForcedDrift,
        }
    );
}

#[test]
fn a_path_edited_into_agreement_with_desired_skips() {
    let entry = file("v2\n", false);
    let manifest = manifest_of(&[("a.txt", recorded(&file("v1\n", false), &[OWNER]))]);
    let observations = observed(&[("a.txt", on_disk(&entry))]);

    let plan = plan(
        &tree(&[("a.txt", &entry)]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "a.txt"),
        &Action::Skip {
            entry: entry.clone(),
            expected: signature(&entry),
        }
    );
}

#[test]
fn an_agreement_skip_carries_the_desired_signature() {
    let agreed = file("x\n", true);
    let manifest = manifest_of(&[("bin/tool", recorded(&file("x\n", false), &[OWNER]))]);
    let observations = observed(&[("bin/tool", on_disk(&agreed))]);

    let plan = plan(
        &tree(&[("bin/tool", &agreed)]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "bin/tool"),
        &Action::Skip {
            entry: agreed.clone(),
            expected: NodeSignature {
                kind: EntryKind::File,
                hash: desired_hash(&agreed),
                executable: true,
                target: None,
            },
        }
    );
}

#[test]
fn a_foreign_path_refuses_always() {
    let manifest = Manifest::new();
    let observations = observed(&[("notes.txt", on_disk(&file("mine\n", false)))]);

    for policy in [DriftPolicy::Refuse, DriftPolicy::Overwrite] {
        let plan = plan(
            &tree(&[("notes.txt", &file("theirs\n", false))]),
            &manifest,
            &observations,
            policy,
        );

        assert_eq!(
            action(&plan, "notes.txt"),
            &Action::Refuse {
                refusal: Refusal::Foreign,
            }
        );
    }
}

#[test]
fn a_foreign_path_with_identical_bytes_still_refuses() {
    let entry = file("same\n", false);
    let observations = observed(&[("notes.txt", on_disk(&entry))]);

    let plan = plan(
        &tree(&[("notes.txt", &entry)]),
        &Manifest::new(),
        &observations,
        DriftPolicy::Overwrite,
    );

    assert_eq!(
        action(&plan, "notes.txt"),
        &Action::Refuse {
            refusal: Refusal::Foreign,
        }
    );
}

#[test]
fn a_desired_path_over_an_empty_foreign_directory_refuses() {
    let plan = plan(
        &tree(&[("existing", &file("now a file\n", false))]),
        &Manifest::new(),
        &observed(&[("existing", Observation::Directory)]),
        DriftPolicy::Overwrite,
    );

    assert_eq!(
        action(&plan, "existing"),
        &Action::Refuse {
            refusal: Refusal::DirectoryInTheWay {
                holding: BTreeMap::new(),
                unreadable: BTreeSet::new(),
            },
        }
    );
}

#[test]
fn an_orphan_removes_when_disk_matches_the_recorded_hash() {
    let entry = file("old\n", false);
    let manifest = manifest_of(&[("old.txt", recorded(&entry, &[OWNER]))]);
    let observations = observed(&[("old.txt", on_disk(&entry))]);

    let plan = plan(&tree(&[]), &manifest, &observations, DriftPolicy::Refuse);

    assert_eq!(
        action(&plan, "old.txt"),
        &Action::Remove {
            expected: Some(signature(&entry)),
        }
    );
}

#[test]
fn a_drifted_orphan_refuses() {
    let manifest = manifest_of(&[("old.txt", recorded(&file("old\n", false), &[OWNER]))]);
    let observations = observed(&[("old.txt", on_disk(&file("edited\n", false)))]);

    let plan = plan(&tree(&[]), &manifest, &observations, DriftPolicy::Refuse);

    assert_eq!(
        action(&plan, "old.txt"),
        &Action::Refuse {
            refusal: Refusal::Drift,
        }
    );
}

#[test]
fn drift_policy_overwrite_lifts_a_drifted_orphan_to_removal() {
    let drifted = file("edited\n", false);
    let manifest = manifest_of(&[("old.txt", recorded(&file("old\n", false), &[OWNER]))]);
    let observations = observed(&[("old.txt", on_disk(&drifted))]);

    let plan = plan(&tree(&[]), &manifest, &observations, DriftPolicy::Overwrite);

    assert_eq!(
        action(&plan, "old.txt"),
        &Action::Remove {
            expected: Some(signature(&drifted)),
        }
    );
}

#[test]
fn a_removed_recorded_link_plans_and_reports_the_observed_target() {
    let entry = link("themes/dark");
    let manifest = manifest_of(&[("current", recorded(&entry, &[OWNER]))]);
    let observations = observed(&[("current", on_disk(&entry))]);

    let plan = plan(&tree(&[]), &manifest, &observations, DriftPolicy::Refuse);

    assert_eq!(
        action(&plan, "current"),
        &Action::Remove {
            expected: Some(signature(&entry)),
        }
    );
    let report = plan.report(&manifest);
    let row = report.rows.get(Utf8Path::new("current")).expect("a row");
    assert_eq!(
        row.facts.as_ref().and_then(|facts| facts.shape.as_ref()),
        Some(&PathShape::Symlink {
            target: Some("themes/dark".to_owned())
        })
    );
}

#[test]
fn a_lifted_drifted_link_removal_states_the_target_the_disk_holds() {
    let drifted = link("themes/light");
    let manifest = manifest_of(&[("current", recorded(&link("themes/dark"), &[OWNER]))]);
    let observations = observed(&[("current", on_disk(&drifted))]);

    let plan = plan(&tree(&[]), &manifest, &observations, DriftPolicy::Overwrite);

    assert_eq!(
        action(&plan, "current"),
        &Action::Remove {
            expected: Some(signature(&drifted)),
        }
    );
}

#[test]
fn a_missing_orphan_still_plans_removal() {
    let entry = file("old\n", false);
    let manifest = manifest_of(&[("old.txt", recorded(&entry, &[OWNER]))]);
    let observations = observed(&[("old.txt", Observation::Absent)]);

    let plan = plan(&tree(&[]), &manifest, &observations, DriftPolicy::Refuse);

    assert_eq!(action(&plan, "old.txt"), &Action::Remove { expected: None });
}

#[test]
fn a_shared_orphan_releases_the_departing_owner() {
    let entry = file("shared\n", false);
    let manifest = manifest_of(&[(".zshrc", recorded(&entry, &[OWNER, "other"]))]);
    let observations = observed(&[(".zshrc", on_disk(&entry))]);

    let plan = plan(&tree(&[]), &manifest, &observations, DriftPolicy::Refuse);

    assert_eq!(action(&plan, ".zshrc"), &Action::Release);
}

// --- writes ---

#[test]
fn a_new_path_writes() {
    let entry = file("fresh\n", false);

    let plan = plan(
        &tree(&[("new.txt", &entry)]),
        &Manifest::new(),
        &observed(&[]),
        DriftPolicy::Refuse,
    );

    assert_eq!(action(&plan, "new.txt"), &Action::Write { entry });
}

#[test]
fn write_heals_a_missing_path() {
    let entry = file("alpha\n", false);
    let manifest = manifest_of(&[("a.txt", recorded(&entry, &[OWNER]))]);
    let observations = observed(&[("a.txt", Observation::Absent)]);

    let plan = plan(
        &tree(&[("a.txt", &entry)]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(action(&plan, "a.txt"), &Action::Write { entry });
}

#[test]
fn an_executable_bit_change_alone_overwrites() {
    let old = file("#!/bin/sh\n", false);
    let new = file("#!/bin/sh\n", true);
    let manifest = manifest_of(&[("run.sh", recorded(&old, &[OWNER]))]);
    let observations = observed(&[("run.sh", on_disk(&old))]);

    let plan = plan(
        &tree(&[("run.sh", &new)]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "run.sh"),
        &Action::Overwrite {
            entry: new,
            expected: signature(&old),
            reason: OverwriteReason::ExecutableChanged,
        }
    );
}

#[test]
fn a_content_and_executable_bit_change_together_reads_as_content_changed() {
    let old = file("#!/bin/sh\n", false);
    let new = file("#!/bin/bash\n", true);
    let manifest = manifest_of(&[("run.sh", recorded(&old, &[OWNER]))]);
    let observations = observed(&[("run.sh", on_disk(&old))]);

    let plan = plan(
        &tree(&[("run.sh", &new)]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "run.sh"),
        &Action::Overwrite {
            entry: new,
            expected: signature(&old),
            reason: OverwriteReason::ContentChanged,
        }
    );
}

// --- owners ---

#[test]
fn skip_lets_an_owner_join_a_path_another_owner_holds_identically() {
    let entry = file("shared\n", false);
    let manifest = manifest_of(&[(".zshrc", recorded(&entry, &["other"]))]);
    let observations = observed(&[(".zshrc", on_disk(&entry))]);

    let plan = plan(
        &tree(&[(".zshrc", &entry)]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, ".zshrc"),
        &Action::Skip {
            entry: entry.clone(),
            expected: signature(&entry),
        }
    );
}

#[test]
fn a_desired_entry_differing_from_a_shared_recorded_one_refuses() {
    let held = file("theirs\n", false);
    let manifest = manifest_of(&[(".zshrc", recorded(&held, &["other", "third"]))]);
    let observations = observed(&[(".zshrc", on_disk(&held))]);

    let plan = plan(
        &tree(&[(".zshrc", &file("mine\n", false))]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, ".zshrc"),
        &Action::Refuse {
            refusal: Refusal::OwnerConflict {
                owners: BTreeSet::from(["other".to_owned(), "third".to_owned()]),
            },
        }
    );
}

#[test]
fn a_sharing_owner_changing_a_shared_path_refuses_with_the_other_owners() {
    let held = file("agreed\n", false);
    let manifest = manifest_of(&[(".zshrc", recorded(&held, &[OWNER, "other"]))]);
    let observations = observed(&[(".zshrc", on_disk(&held))]);

    let plan = plan(
        &tree(&[(".zshrc", &file("changed\n", false))]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, ".zshrc"),
        &Action::Refuse {
            refusal: Refusal::OwnerConflict {
                owners: BTreeSet::from(["other".to_owned()]),
            },
        }
    );
}

#[test]
fn paths_held_only_by_other_owners_are_untouched() {
    let entry = file("theirs\n", false);
    let manifest = manifest_of(&[("theirs.txt", recorded(&entry, &["other"]))]);
    let observations = observed(&[("theirs.txt", on_disk(&entry))]);

    let plan = plan(&tree(&[]), &manifest, &observations, DriftPolicy::Refuse);

    assert!(plan.actions.is_empty());
}

#[test]
fn an_empty_desired_tree_plans_removal_and_release() {
    let sole = file("sole\n", false);
    let shared = file("shared\n", false);
    let manifest = manifest_of(&[
        ("sole.txt", recorded(&sole, &[OWNER])),
        ("shared.txt", recorded(&shared, &[OWNER, "other"])),
    ]);
    let observations = observed(&[
        ("sole.txt", on_disk(&sole)),
        ("shared.txt", on_disk(&shared)),
    ]);

    let plan = plan(&tree(&[]), &manifest, &observations, DriftPolicy::Refuse);

    assert_eq!(
        action(&plan, "sole.txt"),
        &Action::Remove {
            expected: Some(signature(&sole)),
        }
    );
    assert_eq!(action(&plan, "shared.txt"), &Action::Release);
    assert_eq!(plan.actions.len(), 2);
}

// --- containment and the state directory ---

#[test]
fn desired_paths_enter_through_contained_join() {
    let entry = file("x\n", false);

    let plan = plan(
        &tree(&[
            ("../escape", &entry),
            ("/absolute", &entry),
            ("a/./b", &entry),
            ("ok.txt", &entry),
        ]),
        &Manifest::new(),
        &observed(&[]),
        DriftPolicy::Refuse,
    );

    for refused in ["../escape", "/absolute", "a/./b"] {
        assert_eq!(
            action(&plan, refused),
            &Action::Refuse {
                refusal: Refusal::Containment { through: None },
            },
            "expected a containment refusal at {refused}"
        );
    }
    assert_eq!(action(&plan, "ok.txt"), &Action::Write { entry });
}

#[test]
fn a_denormalized_desired_key_unifies_with_its_recorded_path() {
    let entry = file("alpha\n", false);
    let manifest = manifest_of(&[("b", recorded(&entry, &[OWNER]))]);
    let observations = observed(&[("b", on_disk(&entry))]);

    let plan = plan(
        &tree(&[("a/../b", &entry)]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "b"),
        &Action::Skip {
            entry: entry.clone(),
            expected: signature(&entry),
        }
    );
    assert_eq!(plan.actions.len(), 1);
}

#[test]
fn two_desired_keys_normalizing_to_one_path_refuse_both() {
    let entry = file("x\n", false);
    let plan = plan(
        &tree(&[
            ("b", &file("one\n", false)),
            ("a/../b", &file("two\n", false)),
            ("ok.txt", &entry),
        ]),
        &Manifest::new(),
        &observed(&[]),
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "b"),
        &Action::Refuse {
            refusal: Refusal::TreeConflict {
                paths: BTreeSet::from([Utf8PathBuf::from("a/../b")]),
            },
        }
    );
    assert_eq!(
        action(&plan, "a/../b"),
        &Action::Refuse {
            refusal: Refusal::TreeConflict {
                paths: BTreeSet::from([Utf8PathBuf::from("b")]),
            },
        }
    );
    assert_eq!(action(&plan, "ok.txt"), &Action::Write { entry });
    assert_eq!(plan.actions.len(), 3);
}

#[test]
fn a_desired_path_beneath_another_refuses_both() {
    let plan = plan(
        &tree(&[
            ("a", &file("whole\n", false)),
            ("a/b", &file("nested\n", false)),
            ("c", &file("fine\n", false)),
        ]),
        &Manifest::new(),
        &observed(&[]),
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "a"),
        &Action::Refuse {
            refusal: Refusal::TreeConflict {
                paths: BTreeSet::from([Utf8PathBuf::from("a/b")]),
            },
        }
    );
    assert_eq!(
        action(&plan, "a/b"),
        &Action::Refuse {
            refusal: Refusal::TreeConflict {
                paths: BTreeSet::from([Utf8PathBuf::from("a")]),
            },
        }
    );
    assert!(matches!(action(&plan, "c"), Action::Write { .. }));
}

#[test]
fn a_chain_of_overlapping_desired_paths_refuses_every_claimant() {
    let plan = plan(
        &tree(&[
            ("a", &file("1\n", false)),
            ("a/b", &file("2\n", false)),
            ("a/b/c", &file("3\n", false)),
        ]),
        &Manifest::new(),
        &observed(&[]),
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "a/b"),
        &Action::Refuse {
            refusal: Refusal::TreeConflict {
                paths: BTreeSet::from([Utf8PathBuf::from("a"), Utf8PathBuf::from("a/b/c")]),
            },
        }
    );
    assert_eq!(
        action(&plan, "a"),
        &Action::Refuse {
            refusal: Refusal::TreeConflict {
                paths: BTreeSet::from([Utf8PathBuf::from("a/b"), Utf8PathBuf::from("a/b/c")]),
            },
        }
    );
    assert_eq!(
        action(&plan, "a/b/c"),
        &Action::Refuse {
            refusal: Refusal::TreeConflict {
                paths: BTreeSet::from([Utf8PathBuf::from("a"), Utf8PathBuf::from("a/b")]),
            },
        }
    );
}

#[test]
fn a_recorded_path_under_a_tree_conflict_is_not_an_orphan() {
    let entry = file("v1\n", false);
    let manifest = manifest_of(&[
        ("a", recorded(&entry, &[OWNER])),
        ("shared", recorded(&entry, &[OWNER, "other"])),
    ]);
    let observations = observed(&[("a", on_disk(&entry)), ("shared", on_disk(&entry))]);

    let plan = plan(
        &tree(&[
            ("a", &entry),
            ("a/b", &file("nested\n", false)),
            ("shared", &entry),
            ("shared/x", &file("nested\n", false)),
        ]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    for (refused, other) in [
        ("a", "a/b"),
        ("a/b", "a"),
        ("shared", "shared/x"),
        ("shared/x", "shared"),
    ] {
        assert_eq!(
            action(&plan, refused),
            &Action::Refuse {
                refusal: Refusal::TreeConflict {
                    paths: BTreeSet::from([Utf8PathBuf::from(other)]),
                },
            },
            "expected a tree-conflict refusal at {refused}"
        );
    }
    assert_eq!(plan.actions.len(), 4);
}

#[test]
fn a_desired_path_entering_the_state_dir_refuses_containment() {
    let entry = file("x\n", false);

    let plan = decide(
        OWNER,
        &tree(&[
            (".proiectio/manifest.json", &entry),
            (".proiectio", &entry),
            ("elsewhere/.proiectio", &entry),
        ]),
        &Manifest::new(),
        &observed(&[]),
        Some(Utf8Path::new(".proiectio")),
        PlanOptions::default(),
    )
    .expect("decide");

    for refused in [".proiectio/manifest.json", ".proiectio"] {
        assert_eq!(
            action(&plan, refused),
            &Action::Refuse {
                refusal: Refusal::Containment { through: None },
            },
            "expected a containment refusal at {refused}"
        );
    }
    assert_eq!(
        action(&plan, "elsewhere/.proiectio"),
        &Action::Write { entry }
    );
}

const NESTED_STATE: &str = ".local/state/proiectio";

#[test]
fn a_desired_path_the_state_dir_sits_beneath_refuses_containment() {
    let entry = file("x\n", false);

    let plan = decide(
        OWNER,
        &tree(&[
            (".local", &entry),
            (".local/state", &entry),
            (".local/state/proiectio/manifest.json", &entry),
            (".local/share/rc", &entry),
        ]),
        &Manifest::new(),
        &observed(&[]),
        Some(Utf8Path::new(NESTED_STATE)),
        PlanOptions::default(),
    )
    .expect("decide");

    for refused in [
        ".local",
        ".local/state",
        ".local/state/proiectio/manifest.json",
    ] {
        assert_eq!(
            action(&plan, refused),
            &Action::Refuse {
                refusal: Refusal::Containment { through: None },
            },
            "expected a containment refusal at {refused}"
        );
    }
    assert_eq!(action(&plan, ".local/share/rc"), &Action::Write { entry });
}

#[test]
fn a_recorded_path_the_state_dir_sits_beneath_is_refused_not_removed() {
    let entry = file("alpha\n", false);
    let manifest = manifest_of(&[(".local", recorded(&entry, &[OWNER]))]);
    let observations = observed(&[(".local", on_disk(&entry))]);
    let refused = Action::Refuse {
        refusal: Refusal::Containment { through: None },
    };

    let sweep = decide_removal(
        OWNER,
        RemovalScope::Everything,
        &manifest,
        &observations,
        Some(Utf8Path::new(NESTED_STATE)),
        DriftPolicy::Refuse,
    );
    assert_eq!(action(&sweep, ".local"), &refused);

    let by_name = decide_removal(
        OWNER,
        RemovalScope::Paths(&requested(&[".local"])),
        &manifest,
        &observations,
        Some(Utf8Path::new(NESTED_STATE)),
        DriftPolicy::Refuse,
    );
    assert_eq!(action(&by_name, ".local"), &refused);

    let plan = decide(
        OWNER,
        &tree(&[("d/../.local", &entry)]),
        &manifest,
        &observations,
        Some(Utf8Path::new(NESTED_STATE)),
        PlanOptions::default(),
    )
    .expect("decide");

    assert_eq!(
        plan.actions,
        BTreeMap::from([("d/../.local".into(), refused)])
    );
}

#[test]
fn a_path_the_state_dir_sits_beneath_still_classifies() {
    let entry = file("alpha\n", false);
    let manifest = manifest_of(&[(".local", recorded(&entry, &[OWNER]))]);
    let observations = observed(&[
        (".local", Observation::Directory),
        (".local/share", Observation::Directory),
        (".local/share/rc", on_disk(&entry)),
        (".local/state", Observation::Directory),
        (".local/state/proiectio", Observation::Directory),
        (
            ".local/state/proiectio/manifest.json",
            on_disk(&file("{\"version\":1}", false)),
        ),
    ]);

    let status = classify(&manifest, &observations, Some(Utf8Path::new(NESTED_STATE)));

    assert_eq!(
        status
            .iter()
            .map(|(path, row)| (Utf8PathBuf::from(path), row.verdict))
            .collect::<BTreeMap<_, _>>(),
        BTreeMap::from([
            (".local".into(), PathState::Drifted),
            (".local/share".into(), PathState::Foreign),
            (".local/share/rc".into(), PathState::Foreign),
            (".local/state".into(), PathState::Foreign),
        ])
    );
}

#[test]
fn the_state_subtree_is_invisible_to_planning() {
    let entry = file("alpha\n", false);
    let manifest = manifest_of(&[
        ("a.txt", recorded(&entry, &[OWNER])),
        (".proiectio/old", recorded(&entry, &[OWNER])),
    ]);
    let observations = observed(&[
        ("a.txt", on_disk(&entry)),
        (".proiectio", Observation::Directory),
        (
            ".proiectio/manifest.json",
            on_disk(&file("{\"version\":1}", false)),
        ),
    ]);

    let plan = decide(
        OWNER,
        &tree(&[("a.txt", &entry)]),
        &manifest,
        &observations,
        Some(Utf8Path::new(".proiectio")),
        PlanOptions::default(),
    )
    .expect("decide");

    assert_eq!(
        plan.actions.keys().collect::<Vec<_>>(),
        [Utf8Path::new("a.txt")]
    );

    let sweep = decide_removal(
        OWNER,
        RemovalScope::Everything,
        &manifest,
        &observations,
        Some(Utf8Path::new(".proiectio")),
        DriftPolicy::Refuse,
    );

    assert_eq!(
        sweep.actions.keys().collect::<Vec<_>>(),
        [Utf8Path::new("a.txt")]
    );

    let by_name = decide_removal(
        OWNER,
        RemovalScope::Paths(&requested(&[".proiectio/old"])),
        &manifest,
        &observations,
        Some(Utf8Path::new(".proiectio")),
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&by_name, ".proiectio/old"),
        &Action::Refuse {
            refusal: Refusal::Containment { through: None },
        }
    );
}

// --- symlink target grading (`docs/security.lex` section 3) ---

fn allowing_external() -> PlanOptions {
    PlanOptions {
        external_targets: ExternalTargetPolicy::Allow,
        ..PlanOptions::default()
    }
}

#[test]
fn target_grading_admits_in_dest_targets_and_refuses_escaping_ones() {
    let table = [
        ("rc", "shared/rc", true),
        ("rc", "./shared/rc", true),
        ("nested/rc", "../shared/rc", true),
        ("nested/deep/rc", "../../shared/rc", true),
        ("rc", "sub/../shared/rc", true),
        ("rc", "shared//rc", true),
        ("rc", "shared/", true),
        ("rc", ".", true),
        ("rc", "not-there/yet", true),
        ("rc", "..", false),
        ("rc", "../outside", false),
        ("nested/rc", "../../outside", false),
        ("rc", "/etc/passwd", false),
        ("rc", "/", false),
        ("rc", "C:/escape", false),
        ("rc", "C:escape", false),
        ("rc", "..\\..\\escape", false),
        // A colon that is a name, not a drive.
        ("rc", "victim:stream", true),
    ];

    for (path, target, in_dest) in table {
        let entry = link(target);
        let desired = tree(&[(path, &entry)]);

        let refusing = plan(
            &desired,
            &Manifest::new(),
            &observed(&[]),
            DriftPolicy::Refuse,
        );
        let allowing = plan_with(
            &desired,
            &Manifest::new(),
            &observed(&[]),
            allowing_external(),
        );

        let expected = if in_dest {
            Action::Write {
                entry: entry.clone(),
            }
        } else {
            Action::Refuse {
                refusal: Refusal::ExternalTarget {
                    target: target.to_owned(),
                },
            }
        };
        assert_eq!(
            action(&refusing, path),
            &expected,
            "grading {path} -> {target}"
        );
        assert_eq!(
            action(&allowing, path),
            &Action::Write {
                entry: entry.clone()
            },
            "permitted {path} -> {target}"
        );
    }
}

#[test]
fn a_target_that_is_not_a_pathname_refuses_under_either_policy() {
    for target in ["", "\0", "shared/\0rc"] {
        let entry = link(target);
        let desired = tree(&[("rc", &entry)]);
        let expected = Action::Refuse {
            refusal: Refusal::InvalidTarget {
                target: target.to_owned(),
            },
        };

        for options in [PlanOptions::default(), allowing_external()] {
            let plan = plan_with(&desired, &Manifest::new(), &observed(&[]), options);
            assert_eq!(
                action(&plan, "rc"),
                &expected,
                "{target:?} under {options:?}"
            );
        }
    }
}

#[test]
fn an_external_target_refuses_even_where_the_link_is_already_recorded_and_clean() {
    let entry = link("/opt/toolchain");
    let manifest = manifest_of(&[("toolchain", recorded(&entry, &[OWNER]))]);
    let observations = observed(&[("toolchain", on_disk(&entry))]);
    let desired = tree(&[("toolchain", &entry)]);

    let refusing = plan(&desired, &manifest, &observations, DriftPolicy::Refuse);
    let forced = plan(&desired, &manifest, &observations, DriftPolicy::Overwrite);
    let allowing = plan_with(&desired, &manifest, &observations, allowing_external());

    for plan in [&refusing, &forced] {
        assert_eq!(
            action(plan, "toolchain"),
            &Action::Refuse {
                refusal: Refusal::ExternalTarget {
                    target: "/opt/toolchain".to_owned(),
                },
            }
        );
    }
    assert_eq!(
        action(&allowing, "toolchain"),
        &Action::Skip {
            entry: entry.clone(),
            expected: signature(&entry),
        }
    );
}

#[test]
fn a_recorded_external_link_the_tree_dropped_is_removed_without_permission() {
    let entry = link("/opt/toolchain");
    let manifest = manifest_of(&[("toolchain", recorded(&entry, &[OWNER]))]);
    let observations = observed(&[("toolchain", on_disk(&entry))]);

    let plan = plan(&tree(&[]), &manifest, &observations, DriftPolicy::Refuse);

    assert_eq!(
        action(&plan, "toolchain"),
        &Action::Remove {
            expected: Some(signature(&entry)),
        }
    );
}

// --- grading through the destination's own links ---

#[test]
fn a_target_reaching_outside_through_a_link_in_the_destination_grades_external() {
    let evil = link("pivot/passwd");
    let observations = observed(&[("pivot", on_disk(&link("/etc")))]);
    let desired = tree(&[("evil", &evil)]);

    let refusing = plan(
        &desired,
        &Manifest::new(),
        &observations,
        DriftPolicy::Refuse,
    );
    let allowing = plan_with(
        &desired,
        &Manifest::new(),
        &observations,
        allowing_external(),
    );

    assert_eq!(
        action(&refusing, "evil"),
        &Action::Refuse {
            refusal: Refusal::ExternalTarget {
                target: "pivot/passwd".to_owned(),
            },
        }
    );
    assert_eq!(
        action(&allowing, "evil"),
        &Action::Write {
            entry: evil.clone()
        }
    );
}

#[test]
fn an_ordinary_in_dest_chain_needs_no_permission() {
    let rc = link("shared/rc");
    let observations = observed(&[
        ("real", Observation::Directory),
        ("shared", on_disk(&link("real"))),
    ]);

    let plan = plan(
        &tree(&[("rc", &rc)]),
        &Manifest::new(),
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(action(&plan, "rc"), &Action::Write { entry: rc.clone() });
}

#[test]
fn a_hop_pointing_at_nothing_keeps_the_chain_in_dest() {
    let rc = link("shared/rc");
    let observations = observed(&[("shared", on_disk(&link("gone")))]);

    let plan = plan(
        &tree(&[("rc", &rc)]),
        &Manifest::new(),
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(action(&plan, "rc"), &Action::Write { entry: rc.clone() });
}

#[test]
fn a_target_chaining_into_a_cycle_refuses_rather_than_looping() {
    let rc = link("l1");
    let observations = observed(&[("l1", on_disk(&link("l2"))), ("l2", on_disk(&link("l1")))]);

    let plan = plan(
        &tree(&[("rc", &rc)]),
        &Manifest::new(),
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "rc"),
        &Action::Refuse {
            refusal: Refusal::ExternalTarget {
                target: "l1".to_owned(),
            },
        }
    );
}

#[test]
fn a_hop_whose_on_disk_target_is_not_utf8_grades_the_chain_external() {
    let rc = link("pivot/rc");
    let observations = observed(&[(
        "pivot",
        Observation::Symlink {
            hash: sha256_hex(&[0xff]),
            target: None,
        },
    )]);

    let plan = plan(
        &tree(&[("rc", &rc)]),
        &Manifest::new(),
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "rc"),
        &Action::Refuse {
            refusal: Refusal::ExternalTarget {
                target: "pivot/rc".to_owned(),
            },
        }
    );
}

#[test]
fn a_link_this_plan_removes_is_not_a_hop_the_chain_resolves_through() {
    let pivot = link("/etc");
    let evil = link("pivot/passwd");
    let manifest = manifest_of(&[("pivot", recorded(&pivot, &[OWNER]))]);
    let observations = observed(&[("pivot", on_disk(&pivot))]);

    let plan = plan(
        &tree(&[("evil", &evil)]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "pivot"),
        &Action::Remove {
            expected: Some(signature(&pivot)),
        }
    );
    assert_eq!(
        action(&plan, "evil"),
        &Action::Write {
            entry: evil.clone()
        }
    );
}

#[test]
fn a_plan_carries_the_external_target_permission_it_was_decided_under() {
    let desired = tree(&[]);
    let refusing = plan(
        &desired,
        &Manifest::new(),
        &observed(&[]),
        DriftPolicy::Refuse,
    );
    let allowing = plan_with(
        &desired,
        &Manifest::new(),
        &observed(&[]),
        allowing_external(),
    );

    assert_eq!(refusing.external_targets, ExternalTargetPolicy::Refuse);
    assert_eq!(allowing.external_targets, ExternalTargetPolicy::Allow);
}

#[test]
fn a_target_escaping_through_a_link_the_same_tree_projects_grades_external() {
    let root = link(".");
    let out = link("b/../escape");

    let plan = plan(
        &tree(&[("b", &root), ("a", &out)]),
        &Manifest::new(),
        &observed(&[]),
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "b"),
        &Action::Write {
            entry: root.clone()
        }
    );
    assert_eq!(
        action(&plan, "a"),
        &Action::Refuse {
            refusal: Refusal::ExternalTarget {
                target: "b/../escape".to_owned(),
            },
        }
    );
}

#[test]
fn a_cycle_among_the_links_a_tree_projects_grades_external_on_the_first_run() {
    let itself = link("self");
    let there = link("l2");
    let back = link("l1");

    let plan = plan(
        &tree(&[("self", &itself), ("l1", &there), ("l2", &back)]),
        &Manifest::new(),
        &observed(&[]),
        DriftPolicy::Refuse,
    );

    for (path, target) in [("self", "self"), ("l1", "l2"), ("l2", "l1")] {
        assert_eq!(
            action(&plan, path),
            &Action::Refuse {
                refusal: Refusal::ExternalTarget {
                    target: target.to_owned(),
                },
            },
            "{path}"
        );
    }
}

#[test]
fn an_ordinary_chain_through_a_link_the_same_tree_projects_needs_no_permission() {
    let shared = link("real");
    let rc = link("shared/rc");

    let plan = plan(
        &tree(&[("shared", &shared), ("rc", &rc)]),
        &Manifest::new(),
        &observed(&[]),
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "shared"),
        &Action::Write {
            entry: shared.clone()
        }
    );
    assert_eq!(action(&plan, "rc"), &Action::Write { entry: rc.clone() });
}

#[test]
fn a_pivot_this_run_replaces_is_graded_as_the_link_it_becomes() {
    let escaping = link("/etc");
    let landing = link("real");
    let through = link("pivot/x");
    let manifest = manifest_of(&[("pivot", recorded(&escaping, &[OWNER]))]);

    let plan = plan(
        &tree(&[("pivot", &landing), ("evil", &through)]),
        &manifest,
        &observed(&[
            ("pivot", on_disk(&escaping)),
            ("real", Observation::Directory),
        ]),
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "pivot"),
        &Action::Overwrite {
            entry: landing.clone(),
            expected: signature(&escaping),
            reason: OverwriteReason::ContentChanged,
        }
    );
    assert_eq!(
        action(&plan, "evil"),
        &Action::Write {
            entry: through.clone()
        }
    );
}

// --- the no-alias rule: no projected path resolves through a link ---

#[test]
fn a_desired_path_beneath_a_desired_link_refuses_both_as_a_tree_conflict() {
    let plan = plan(
        &tree(&[
            ("logs", &link("real")),
            ("logs/x.txt", &file("nested\n", false)),
        ]),
        &Manifest::new(),
        &observed(&[]),
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "logs"),
        &Action::Refuse {
            refusal: Refusal::TreeConflict {
                paths: BTreeSet::from([Utf8PathBuf::from("logs/x.txt")]),
            },
        }
    );
    assert_eq!(
        action(&plan, "logs/x.txt"),
        &Action::Refuse {
            refusal: Refusal::TreeConflict {
                paths: BTreeSet::from([Utf8PathBuf::from("logs")]),
            },
        }
    );
}

#[test]
fn a_desired_path_beneath_a_surviving_on_disk_link_refuses_containment() {
    let held_elsewhere = manifest_of(&[("logs", recorded(&link("real"), &["other"]))]);
    let observations = observed(&[
        ("logs", on_disk(&link("real"))),
        ("real", Observation::Directory),
    ]);
    let desired = tree(&[("logs/x.txt", &file("aliased\n", false))]);

    for manifest in [&held_elsewhere, &Manifest::new()] {
        let plan = plan(&desired, manifest, &observations, DriftPolicy::Refuse);

        assert_eq!(
            action(&plan, "logs/x.txt"),
            &Action::Refuse {
                refusal: Refusal::Containment {
                    through: Some(Utf8PathBuf::from("logs")),
                },
            }
        );
    }
}

#[test]
fn a_desired_path_beneath_a_link_this_plan_removes_is_written() {
    let entry = file("a real file\n", false);
    let manifest = manifest_of(&[("logs", recorded(&link("real"), &[OWNER]))]);
    let observations = observed(&[
        ("logs", on_disk(&link("real"))),
        ("real", Observation::Directory),
    ]);

    let plan = plan(
        &tree(&[("logs/x.txt", &entry)]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "logs"),
        &Action::Remove {
            expected: Some(signature(&link("real"))),
        }
    );
    assert_eq!(action(&plan, "logs/x.txt"), &Action::Write { entry });
}

#[test]
fn a_desired_path_beneath_a_link_the_plan_only_releases_still_refuses() {
    let manifest = manifest_of(&[("logs", recorded(&link("real"), &[OWNER, "other"]))]);
    let observations = observed(&[
        ("logs", on_disk(&link("real"))),
        ("real", Observation::Directory),
    ]);

    let plan = plan(
        &tree(&[("logs/x.txt", &file("aliased\n", false))]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(action(&plan, "logs"), &Action::Release);
    assert_eq!(
        action(&plan, "logs/x.txt"),
        &Action::Refuse {
            refusal: Refusal::Containment {
                through: Some(Utf8PathBuf::from("logs")),
            },
        }
    );
}

// --- grading recorded ancestry: the arms act's no-follow walk grades, read off the snapshot ---

#[test]
fn a_removal_beneath_a_hand_made_link_refuses_containment() {
    let manifest = manifest_of(&[(
        "logs/deep/file.txt",
        recorded(&file("kept\n", false), &[OWNER]),
    )]);
    let observations = observed(&[("logs", on_disk(&link("real/missing")))]);

    let plan = removal(
        RemovalScope::Everything,
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "logs/deep/file.txt"),
        &Action::Refuse {
            refusal: Refusal::Containment {
                through: Some(Utf8PathBuf::from("logs")),
            },
        }
    );
}

#[test]
fn a_removal_beneath_a_recorded_link_whose_target_moved_refuses_drift() {
    let manifest = manifest_of(&[
        ("logs", recorded(&link("real"), &["other"])),
        ("logs/x.txt", recorded(&file("kept\n", false), &[OWNER])),
    ]);
    let observations = observed(&[
        ("logs", on_disk(&link("elsewhere"))),
        ("elsewhere", Observation::Directory),
    ]);

    let plan = removal(
        RemovalScope::Everything,
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "logs/x.txt"),
        &Action::Refuse {
            refusal: Refusal::Drift,
        }
    );
}

#[test]
fn a_removal_beneath_a_recorded_link_expects_the_node_the_walk_resolves_to() {
    let kept = file("kept\n", false);
    let manifest = manifest_of(&[
        ("logs", recorded(&link("real"), &["other"])),
        ("logs/x.txt", recorded(&kept, &[OWNER])),
    ]);
    let observations = observed(&[
        ("logs", on_disk(&link("real"))),
        ("real/x.txt", on_disk(&kept)),
    ]);

    let plan = removal(
        RemovalScope::Everything,
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "logs/x.txt"),
        &Action::Remove {
            expected: Some(signature(&kept)),
        }
    );
}

#[test]
fn a_removal_whose_ancestry_is_gone_still_forgets_the_path() {
    let manifest = manifest_of(&[("gone/x.txt", recorded(&file("kept\n", false), &[OWNER]))]);

    let plan = removal(
        RemovalScope::Everything,
        &manifest,
        &observed(&[]),
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "gone/x.txt"),
        &Action::Remove { expected: None }
    );
}

#[test]
fn a_release_beneath_a_link_walks_nothing_and_is_not_refused() {
    let manifest = manifest_of(&[(
        "logs/x.txt",
        recorded(&file("kept\n", false), &[OWNER, "other"]),
    )]);
    let observations = observed(&[("logs", on_disk(&link("real/missing")))]);

    let plan = removal(
        RemovalScope::Everything,
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(action(&plan, "logs/x.txt"), &Action::Release);
}

#[test]
fn a_recorded_link_the_walk_meets_twice_refuses_containment() {
    let looping = link("a");
    let manifest = manifest_of(&[
        ("a", recorded(&looping, &["other"])),
        ("a/x.txt", recorded(&file("kept\n", false), &[OWNER])),
    ]);
    let observations = observed(&[("a", on_disk(&looping))]);

    let plan = removal(
        RemovalScope::Everything,
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "a/x.txt"),
        &Action::Refuse {
            refusal: Refusal::Containment {
                through: Some(Utf8PathBuf::from("a")),
            },
        }
    );
}

#[test]
fn a_desired_path_beneath_a_node_that_is_not_a_directory_refuses() {
    let entry = file("settings\n", false);
    let theirs = manifest_of(&[("conf", recorded(&file("theirs\n", false), &["other"]))]);
    let observations = observed(&[("conf", on_disk(&file("theirs\n", false)))]);

    for (manifest, expected) in [
        (&theirs, Refusal::Drift),
        (&Manifest::new(), Refusal::Foreign),
    ] {
        let plan = plan(
            &tree(&[("conf/rc", &entry)]),
            manifest,
            &observations,
            DriftPolicy::Refuse,
        );

        assert_eq!(
            action(&plan, "conf/rc"),
            &Action::Refuse { refusal: expected }
        );
    }
}

#[test]
fn a_removal_whose_link_lands_in_the_state_subtree_refuses_containment() {
    let secret = file("secret\n", false);
    let manifest = manifest_of(&[
        ("logs", recorded(&link(".proiectio"), &["other"])),
        ("logs/private-state", recorded(&secret, &[OWNER])),
    ]);
    let observations = observed(&[
        ("logs", on_disk(&link(".proiectio"))),
        (".proiectio/private-state", on_disk(&secret)),
    ]);

    let plan = decide_removal(
        OWNER,
        RemovalScope::Everything,
        &manifest,
        &observations,
        Some(Utf8Path::new(".proiectio")),
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "logs/private-state"),
        &Action::Refuse {
            refusal: Refusal::Containment {
                through: Some(Utf8PathBuf::from("logs")),
            },
        }
    );
}

#[test]
fn a_removal_landing_where_a_desired_path_stands_refuses_both() {
    let kept = file("kept\n", false);
    let manifest = manifest_of(&[
        ("logs", recorded(&link("real"), &["other"])),
        ("logs/x.txt", recorded(&kept, &[OWNER])),
        ("real/x.txt", recorded(&kept, &[OWNER])),
    ]);
    let observations = observed(&[
        ("logs", on_disk(&link("real"))),
        ("real/x.txt", on_disk(&kept)),
    ]);

    let plan = plan(
        &tree(&[("real/x.txt", &kept)]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "logs/x.txt"),
        &Action::Refuse {
            refusal: Refusal::TreeConflict {
                paths: BTreeSet::from([Utf8PathBuf::from("real/x.txt")]),
            },
        }
    );
    assert_eq!(
        action(&plan, "real/x.txt"),
        &Action::Refuse {
            refusal: Refusal::TreeConflict {
                paths: BTreeSet::from([Utf8PathBuf::from("logs/x.txt")]),
            },
        }
    );
}

#[test]
fn two_removals_landing_on_one_node_refuse_the_conflict() {
    let kept = file("kept\n", false);
    let manifest = manifest_of(&[
        ("logs", recorded(&link("real"), &["other"])),
        ("logs/x.txt", recorded(&kept, &[OWNER])),
        ("real/x.txt", recorded(&kept, &[OWNER])),
    ]);
    let observations = observed(&[
        ("logs", on_disk(&link("real"))),
        ("real/x.txt", on_disk(&kept)),
    ]);

    let plan = removal(
        RemovalScope::Everything,
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    for (path, other) in [("logs/x.txt", "real/x.txt"), ("real/x.txt", "logs/x.txt")] {
        assert_eq!(
            action(&plan, path),
            &Action::Refuse {
                refusal: Refusal::TreeConflict {
                    paths: BTreeSet::from([Utf8PathBuf::from(other)]),
                },
            }
        );
    }
}

#[test]
fn a_removal_landing_on_another_owners_record_refuses() {
    let kept = file("kept\n", false);
    let manifest = manifest_of(&[
        ("a", recorded(&link("real"), &["p"])),
        ("a/x.txt", recorded(&kept, &[OWNER])),
        ("real/x.txt", recorded(&kept, &["p"])),
    ]);
    let observations = observed(&[
        ("a", on_disk(&link("real"))),
        ("real/x.txt", on_disk(&kept)),
    ]);

    let plan = removal(
        RemovalScope::Everything,
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "a/x.txt"),
        &Action::Refuse {
            refusal: Refusal::RecordedLanding {
                through: Utf8PathBuf::from("a"),
                at: Utf8PathBuf::from("real/x.txt"),
                owners: BTreeSet::from(["p".to_owned()]),
            },
        }
    );
}

#[test]
fn a_scoped_removal_landing_on_a_record_outside_its_scope_refuses() {
    let kept = file("kept\n", false);
    let manifest = manifest_of(&[
        ("a", recorded(&link("real"), &["other"])),
        ("a/x.txt", recorded(&kept, &[OWNER])),
        ("real/x.txt", recorded(&kept, &[OWNER])),
    ]);
    let observations = observed(&[
        ("a", on_disk(&link("real"))),
        ("real/x.txt", on_disk(&kept)),
    ]);
    let scope = requested(&["a/x.txt"]);

    let plan = removal(
        RemovalScope::Paths(&scope),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "a/x.txt"),
        &Action::Refuse {
            refusal: Refusal::RecordedLanding {
                through: Utf8PathBuf::from("a"),
                at: Utf8PathBuf::from("real/x.txt"),
                owners: BTreeSet::from([OWNER.to_owned()]),
            },
        }
    );
}

#[test]
fn a_removal_landing_on_a_record_this_plan_only_releases_refuses() {
    // A release leaves the node standing, so it does not vacate the landing.
    let kept = file("kept\n", false);
    let manifest = manifest_of(&[
        ("a", recorded(&link("real"), &["other"])),
        ("a/x.txt", recorded(&kept, &[OWNER])),
        ("real/x.txt", recorded(&kept, &[OWNER, "p"])),
    ]);
    let observations = observed(&[
        ("a", on_disk(&link("real"))),
        ("real/x.txt", on_disk(&kept)),
    ]);

    let plan = removal(
        RemovalScope::Everything,
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(action(&plan, "real/x.txt"), &Action::Release);
    assert_eq!(
        action(&plan, "a/x.txt"),
        &Action::Refuse {
            refusal: Refusal::RecordedLanding {
                through: Utf8PathBuf::from("a"),
                at: Utf8PathBuf::from("real/x.txt"),
                owners: BTreeSet::from(["p".to_owned(), OWNER.to_owned()]),
            },
        }
    );
}

#[test]
fn a_removal_landing_on_a_record_with_nothing_on_disk_forgets_it() {
    // Refusing an absence-only removal would strand a stale record.
    let kept = file("kept\n", false);
    let manifest = manifest_of(&[
        ("a", recorded(&link("real"), &["p"])),
        ("a/x.txt", recorded(&kept, &[OWNER])),
        ("real/x.txt", recorded(&kept, &["p"])),
    ]);
    let observations = observed(&[
        ("a", on_disk(&link("real"))),
        ("real", Observation::Directory),
    ]);

    let plan = removal(
        RemovalScope::Everything,
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(action(&plan, "a/x.txt"), &Action::Remove { expected: None });
}

#[test]
fn a_removal_landing_on_a_record_whose_own_removal_refuses_refuses_too() {
    // A refusal takes no node, so it claims the landing for nothing.
    let kept = file("kept\n", false);
    let old = file("old\n", false);
    let manifest = manifest_of(&[
        ("a", recorded(&link("real"), &["other"])),
        ("a/x.txt", recorded(&kept, &[OWNER])),
        ("real/x.txt", recorded(&old, &[OWNER])),
    ]);
    let observations = observed(&[
        ("a", on_disk(&link("real"))),
        ("real/x.txt", on_disk(&kept)),
    ]);

    let plan = removal(
        RemovalScope::Everything,
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "real/x.txt"),
        &Action::Refuse {
            refusal: Refusal::Drift,
        }
    );
    assert_eq!(
        action(&plan, "a/x.txt"),
        &Action::Refuse {
            refusal: Refusal::RecordedLanding {
                through: Utf8PathBuf::from("a"),
                at: Utf8PathBuf::from("real/x.txt"),
                owners: BTreeSet::from([OWNER.to_owned()]),
            },
        }
    );
}

#[test]
fn a_write_walks_through_the_location_a_removal_vacates() {
    let old = file("old\n", false);
    let fresh = file("fresh\n", false);
    let manifest = manifest_of(&[
        ("logs", recorded(&link("real"), &["other"])),
        ("logs/x", recorded(&old, &[OWNER])),
    ]);
    let observations = observed(&[("logs", on_disk(&link("real"))), ("real/x", on_disk(&old))]);

    let plan = plan(
        &tree(&[("real/x/child.txt", &fresh)]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "logs/x"),
        &Action::Remove {
            expected: Some(signature(&old)),
        }
    );
    assert_eq!(
        action(&plan, "real/x/child.txt"),
        &Action::Write {
            entry: fresh.clone(),
        }
    );
}

#[test]
fn an_absence_only_removal_claims_no_node() {
    let old = file("old\n", false);
    let fresh = file("fresh\n", false);
    let manifest = manifest_of(&[
        ("logs", recorded(&link("real"), &["other"])),
        ("logs/x", recorded(&old, &[OWNER])),
    ]);
    let observations = observed(&[
        ("logs", on_disk(&link("real"))),
        ("real", Observation::Directory),
    ]);

    let plan = plan(
        &tree(&[("real/x", &fresh)]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(action(&plan, "logs/x"), &Action::Remove { expected: None });
    assert_eq!(
        action(&plan, "real/x"),
        &Action::Write {
            entry: fresh.clone(),
        }
    );
}

#[test]
fn a_removal_landing_on_its_own_key_conflicts_with_nothing() {
    let kept = file("kept\n", false);
    let manifest = manifest_of(&[
        ("a.txt", recorded(&kept, &[OWNER])),
        ("deep/b.txt", recorded(&kept, &[OWNER])),
    ]);
    let observations = observed(&[("a.txt", on_disk(&kept)), ("deep/b.txt", on_disk(&kept))]);

    let plan = removal(
        RemovalScope::Everything,
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    for path in ["a.txt", "deep/b.txt"] {
        assert_eq!(
            action(&plan, path),
            &Action::Remove {
                expected: Some(signature(&kept)),
            }
        );
    }
}

// Lexical and resolved-parent grading disagree only under a symlink ancestor,
// refused from both directions; each test asserts the divergent verdict beside
// the refusal so removing either guard fails here.

#[test]
fn a_desired_link_beneath_a_desired_link_refuses_before_the_verdicts_diverge() {
    let pivot = link("real");
    let escaping = link("../../escape");

    let plan = plan(
        &tree(&[("b/c", &pivot), ("b/c/x", &escaping)]),
        &Manifest::new(),
        &observed(&[("real", Observation::Directory)]),
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "b/c"),
        &Action::Refuse {
            refusal: Refusal::TreeConflict {
                paths: BTreeSet::from([Utf8PathBuf::from("b/c/x")]),
            },
        }
    );
    assert_eq!(
        action(&plan, "b/c/x"),
        &Action::Refuse {
            refusal: Refusal::TreeConflict {
                paths: BTreeSet::from([Utf8PathBuf::from("b/c")]),
            },
        }
    );
}

#[test]
fn a_desired_link_beneath_an_observed_link_refuses_before_the_verdicts_diverge() {
    let escaping = link("../../escape");
    let observations = observed(&[
        ("b/c", on_disk(&link("real"))),
        ("real", Observation::Directory),
    ]);

    let plan = plan(
        &tree(&[("b/c/x", &escaping), ("real/x", &escaping)]),
        &Manifest::new(),
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "b/c/x"),
        &Action::Refuse {
            refusal: Refusal::Containment {
                through: Some(Utf8PathBuf::from("b/c")),
            },
        }
    );
    assert_eq!(
        action(&plan, "real/x"),
        &Action::Refuse {
            refusal: Refusal::ExternalTarget {
                target: "../../escape".to_owned(),
            },
        }
    );
}

// --- kind-agnostic comparison: symlinks through the generic table ---

#[test]
fn a_desired_link_matching_the_disk_target_skips() {
    let entry = link("shared/rc");
    let manifest = manifest_of(&[("rc", recorded(&entry, &[OWNER]))]);
    let observations = observed(&[("rc", on_disk(&entry))]);

    let plan = plan(
        &tree(&[("rc", &entry)]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "rc"),
        &Action::Skip {
            entry: entry.clone(),
            expected: signature(&entry),
        }
    );
}

#[test]
fn a_changed_desired_link_target_overwrites_a_clean_link() {
    let old = link("v1");
    let new = link("v2");
    let manifest = manifest_of(&[("rc", recorded(&old, &[OWNER]))]);
    let observations = observed(&[("rc", on_disk(&old))]);

    let plan = plan(
        &tree(&[("rc", &new)]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "rc"),
        &Action::Overwrite {
            entry: new,
            expected: signature(&old),
            reason: OverwriteReason::ContentChanged,
        }
    );
}

#[test]
fn a_desired_file_over_a_recorded_link_is_drift_of_kind_when_the_link_moved() {
    let manifest = manifest_of(&[("rc", recorded(&link("v1"), &[OWNER]))]);
    let observations = observed(&[("rc", on_disk(&link("moved")))]);

    let plan = plan(
        &tree(&[("rc", &file("now a file\n", false))]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "rc"),
        &Action::Refuse {
            refusal: Refusal::Drift,
        }
    );
}

// --- unhashable drift is never lifted ---

#[test]
fn drift_to_an_empty_directory_is_replaced_under_overwrite_policy() {
    let manifest = manifest_of(&[("a", recorded(&file("v1\n", false), &[OWNER]))]);
    let observations = observed(&[("a", Observation::Directory)]);

    let plan = plan(
        &tree(&[("a", &file("v2\n", false))]),
        &manifest,
        &observations,
        DriftPolicy::Overwrite,
    );

    assert_eq!(
        action(&plan, "a"),
        &Action::OverwriteDirectory {
            entry: file("v2\n", false),
        }
    );
}

#[test]
fn a_drifted_orphan_now_a_directory_stays_refused_under_overwrite_policy() {
    let manifest = manifest_of(&[("old", recorded(&file("v1\n", false), &[OWNER]))]);
    let observations = observed(&[("old", Observation::Other)]);

    let plan = plan(&tree(&[]), &manifest, &observations, DriftPolicy::Overwrite);

    assert_eq!(
        action(&plan, "old"),
        &Action::Refuse {
            refusal: Refusal::Drift,
        }
    );

    assert_eq!(
        plan.refused().expect("a refused path").to_string(),
        "refusing to touch drifted paths (edited on disk): old; pass --force to touch them \
         anyway, where the projection can still tell what it would replace"
    );
}

// --- a directory standing where a file or a link belongs ---

#[test]
fn a_directory_the_runs_own_removals_empty_becomes_the_desired_file() {
    let inside = file("scaffolding\n", false);
    let manifest = manifest_of(&[("build.sh/main", recorded(&inside, &[OWNER]))]);
    let observations = observed(&[
        ("build.sh", Observation::Directory),
        ("build.sh/main", on_disk(&inside)),
    ]);

    let plan = plan(
        &tree(&[("build.sh", &file("#!/bin/sh\n", true))]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "build.sh/main"),
        &Action::Remove {
            expected: Some(signature(&inside)),
        }
    );
    assert_eq!(
        action(&plan, "build.sh"),
        &Action::Write {
            entry: file("#!/bin/sh\n", true),
        }
    );
}

#[test]
fn a_node_nothing_records_holds_the_directory_and_the_refusal_names_it() {
    let inside = file("scaffolding\n", false);
    let manifest = manifest_of(&[("build.sh/main", recorded(&inside, &[OWNER]))]);
    let observations = observed(&[
        ("build.sh", Observation::Directory),
        ("build.sh/main", on_disk(&inside)),
        ("build.sh/notes.md", on_disk(&file("mine\n", false))),
    ]);

    let plan = plan(
        &tree(&[("build.sh", &file("#!/bin/sh\n", true))]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "build.sh"),
        &Action::Refuse {
            refusal: Refusal::DirectoryInTheWay {
                holding: BTreeMap::from([("build.sh/notes.md".into(), BTreeSet::new())]),
                unreadable: BTreeSet::new(),
            },
        }
    );
}

#[test]
fn another_owners_record_beneath_holds_the_directory_and_names_the_owner() {
    let mine = file("scaffolding\n", false);
    let theirs = file("site\n", false);
    let manifest = manifest_of(&[
        ("build.sh/main", recorded(&mine, &[OWNER])),
        ("build.sh/theirs", recorded(&theirs, &["other"])),
    ]);
    let observations = observed(&[
        ("build.sh", Observation::Directory),
        ("build.sh/main", on_disk(&mine)),
        ("build.sh/theirs", on_disk(&theirs)),
    ]);

    let plan = plan(
        &tree(&[("build.sh", &file("#!/bin/sh\n", true))]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "build.sh"),
        &Action::Refuse {
            refusal: Refusal::DirectoryInTheWay {
                holding: BTreeMap::from([(
                    "build.sh/theirs".into(),
                    BTreeSet::from(["other".to_owned()]),
                )]),
                unreadable: BTreeSet::new(),
            },
        }
    );
}

#[test]
fn an_empty_directory_nested_in_the_scaffolding_holds_it() {
    let inside = file("scaffolding\n", false);
    let manifest = manifest_of(&[("build.sh/main", recorded(&inside, &[OWNER]))]);
    let observations = observed(&[
        ("build.sh", Observation::Directory),
        ("build.sh/main", on_disk(&inside)),
        ("build.sh/scratch", Observation::Directory),
    ]);

    let plan = plan(
        &tree(&[("build.sh", &file("#!/bin/sh\n", true))]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "build.sh"),
        &Action::Refuse {
            refusal: Refusal::DirectoryInTheWay {
                holding: BTreeMap::from([("build.sh/scratch".into(), BTreeSet::new())]),
                unreadable: BTreeSet::new(),
            },
        }
    );
}

#[test]
fn a_block_beneath_the_directory_holds_it_because_its_container_survives() {
    let region = block("managed\n", Placement::Append);
    let manifest = manifest_of(&[("build.sh/conf", recorded(&region, &[OWNER]))]);
    let observations = observed(&[
        ("build.sh", Observation::Directory),
        ("build.sh/conf", on_disk(&region)),
    ]);

    let plan = plan(
        &tree(&[("build.sh", &file("#!/bin/sh\n", true))]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "build.sh/conf"),
        &Action::Remove {
            expected: Some(signature(&region)),
        }
    );
    assert_eq!(
        action(&plan, "build.sh"),
        &Action::Refuse {
            refusal: Refusal::DirectoryInTheWay {
                holding: BTreeMap::from([(
                    "build.sh/conf".into(),
                    BTreeSet::from([OWNER.to_owned()]),
                )]),
                unreadable: BTreeSet::new(),
            },
        }
    );
}

#[test]
fn a_drifted_node_beneath_the_directory_is_the_refusal_the_run_states() {
    let recorded_inside = file("v1\n", false);
    let manifest = manifest_of(&[("build.sh/main", recorded(&recorded_inside, &[OWNER]))]);
    let observations = observed(&[
        ("build.sh", Observation::Directory),
        ("build.sh/main", on_disk(&file("edited\n", false))),
    ]);
    let desired = tree(&[("build.sh", &file("#!/bin/sh\n", true))]);

    let refusing = plan(&desired, &manifest, &observations, DriftPolicy::Refuse);
    assert_eq!(
        refusing.refused().expect("refusals").kind(),
        RefusalKind::Drift
    );

    let forced = plan(&desired, &manifest, &observations, DriftPolicy::Overwrite);
    assert_eq!(
        action(&forced, "build.sh"),
        &Action::Write {
            entry: file("#!/bin/sh\n", true),
        }
    );
}

#[test]
fn a_desired_block_over_a_directory_stays_foreign() {
    let plan = plan(
        &tree(&[("conf", &block("managed\n", Placement::Append))]),
        &Manifest::new(),
        &observed(&[("conf", Observation::Directory)]),
        DriftPolicy::Overwrite,
    );

    assert_eq!(
        action(&plan, "conf"),
        &Action::Refuse {
            refusal: Refusal::Foreign,
        }
    );
}

#[test]
fn a_foreign_block_container_is_refused_and_removing_it_refuses_too() {
    let desired = tree(&[("rc", &block("ours\n", Placement::Append))]);
    let theirs = Observation::Block {
        hash: None,
        newline_terminated: true,
        occurrences: 1,
        desired: Some(DesiredRegion {
            occurrences: 1,
            hash: Some(sha256_hex(b"theirs\n")),
            author_newline_terminated: true,
        }),
    };

    let standing = plan(
        &desired,
        &Manifest::new(),
        &observed(&[("rc", theirs)]),
        DriftPolicy::Overwrite,
    );

    assert_eq!(
        action(&standing, "rc"),
        &Action::Refuse {
            refusal: Refusal::Foreign,
        }
    );

    let removed = plan(
        &desired,
        &Manifest::new(),
        &observed(&[]),
        DriftPolicy::Overwrite,
    );

    assert_eq!(
        action(&removed, "rc"),
        &Action::Refuse {
            refusal: Refusal::Block {
                fault: BlockFault::ContainerMissing,
            },
        },
        "removing a block's container does not let the projection write it"
    );

    let region_gone = plan(
        &desired,
        &Manifest::new(),
        &observed(&[("rc", no_region(true))]),
        DriftPolicy::Overwrite,
    );

    assert_eq!(
        action(&region_gone, "rc"),
        &Action::Write {
            entry: block("ours\n", Placement::Append),
        },
        "clearing the region rather than the container is what lets it write"
    );
}

#[test]
fn drift_to_a_directory_holding_anything_refuses_under_either_policy() {
    let manifest = manifest_of(&[("a", recorded(&file("v1\n", false), &[OWNER]))]);
    let observations = observed(&[
        ("a", Observation::Directory),
        ("a/inside", on_disk(&file("theirs\n", false))),
    ]);
    let refusal = Refusal::DirectoryInTheWay {
        holding: BTreeMap::from([("a/inside".into(), BTreeSet::new())]),
        unreadable: BTreeSet::new(),
    };

    for policy in [DriftPolicy::Refuse, DriftPolicy::Overwrite] {
        let desired = plan(
            &tree(&[("a", &file("v2\n", false))]),
            &manifest,
            &observations,
            policy,
        );
        assert_eq!(
            action(&desired, "a"),
            &Action::Refuse {
                refusal: refusal.clone(),
            },
            "{policy:?}"
        );

        let orphaned = plan(&tree(&[]), &manifest, &observations, policy);
        assert_eq!(
            action(&orphaned, "a"),
            &Action::Refuse {
                refusal: refusal.clone(),
            },
            "{policy:?}"
        );
    }
}

#[test]
fn a_drifted_directory_names_what_holds_it_rather_than_the_directory_between() {
    let manifest = manifest_of(&[("a", recorded(&file("v1\n", false), &[OWNER]))]);
    let observations = observed(&[
        ("a", Observation::Directory),
        ("a/sub", Observation::Directory),
        ("a/sub/note.md", on_disk(&file("theirs\n", false))),
    ]);

    let plan = plan(
        &tree(&[("a", &file("v2\n", false))]),
        &manifest,
        &observations,
        DriftPolicy::Overwrite,
    );

    assert_eq!(
        action(&plan, "a"),
        &Action::Refuse {
            refusal: Refusal::DirectoryInTheWay {
                holding: BTreeMap::from([("a/sub/note.md".into(), BTreeSet::new())]),
                unreadable: BTreeSet::new(),
            },
        }
    );
}

#[test]
fn drift_to_an_empty_directory_refuses_as_drift_until_forced() {
    let manifest = manifest_of(&[("a", recorded(&file("v1\n", false), &[OWNER]))]);
    let observations = observed(&[("a", Observation::Directory)]);

    let plan = plan(
        &tree(&[("a", &file("v2\n", false))]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "a"),
        &Action::Refuse {
            refusal: Refusal::Drift,
        }
    );
}

#[test]
fn an_orphan_drifted_to_an_empty_directory_is_removed_under_overwrite_policy() {
    let manifest = manifest_of(&[("old", recorded(&file("v1\n", false), &[OWNER]))]);
    let observations = observed(&[("old", Observation::Directory)]);

    let refusing = plan(&tree(&[]), &manifest, &observations, DriftPolicy::Refuse);
    assert_eq!(
        action(&refusing, "old"),
        &Action::Refuse {
            refusal: Refusal::Drift,
        }
    );

    let forced = plan(&tree(&[]), &manifest, &observations, DriftPolicy::Overwrite);
    assert_eq!(action(&forced, "old"), &Action::RemoveDirectory);
}

#[test]
fn a_directory_drifted_over_another_owners_record_is_an_owner_conflict() {
    let manifest = manifest_of(&[("a", recorded(&file("theirs\n", false), &["other"]))]);
    let observations = observed(&[("a", Observation::Directory)]);
    let desired = tree(&[("a", &file("mine\n", false))]);

    for policy in [DriftPolicy::Refuse, DriftPolicy::Overwrite] {
        assert_eq!(
            action(&plan(&desired, &manifest, &observations, policy), "a"),
            &Action::Refuse {
                refusal: Refusal::OwnerConflict {
                    owners: BTreeSet::from(["other".to_owned()]),
                },
            }
        );
    }
}

#[test]
fn a_directory_drifted_over_a_shared_record_is_an_owner_conflict() {
    let manifest = manifest_of(&[("a", recorded(&file("agreed\n", false), &[OWNER, "other"]))]);
    let observations = observed(&[("a", Observation::Directory)]);
    let desired = tree(&[("a", &file("changed\n", false))]);

    for policy in [DriftPolicy::Refuse, DriftPolicy::Overwrite] {
        assert_eq!(
            action(&plan(&desired, &manifest, &observations, policy), "a"),
            &Action::Refuse {
                refusal: Refusal::OwnerConflict {
                    owners: BTreeSet::from(["other".to_owned()]),
                },
            }
        );
    }
}

#[test]
fn a_child_directory_the_run_removes_does_not_hold_its_parent() {
    let manifest = manifest_of(&[("build.sh/main", recorded(&file("v1\n", false), &[OWNER]))]);
    let observations = observed(&[
        ("build.sh", Observation::Directory),
        ("build.sh/main", Observation::Directory),
    ]);

    let plan = plan(
        &tree(&[("build.sh", &file("#!/bin/sh\n", true))]),
        &manifest,
        &observations,
        DriftPolicy::Overwrite,
    );

    assert_eq!(action(&plan, "build.sh/main"), &Action::RemoveDirectory);
    assert_eq!(
        action(&plan, "build.sh"),
        &Action::Write {
            entry: file("#!/bin/sh\n", true),
        }
    );
}

// --- a directory holding more than observation can state ---

#[test]
fn a_name_the_walk_cannot_read_keeps_the_directory_from_clearing() {
    let inside = file("scaffolding\n", false);
    let manifest = manifest_of(&[("build.sh/main", recorded(&inside, &[OWNER]))]);
    let observations = observed_with_unreadable(
        &[
            ("build.sh", Observation::Directory),
            ("build.sh/main", on_disk(&inside)),
        ],
        &["build.sh"],
    );

    let plan = plan(
        &tree(&[("build.sh", &file("#!/bin/sh\n", true))]),
        &manifest,
        &observations,
        DriftPolicy::Overwrite,
    );

    assert_eq!(
        action(&plan, "build.sh"),
        &Action::Refuse {
            refusal: Refusal::DirectoryInTheWay {
                holding: BTreeMap::new(),
                unreadable: BTreeSet::from(["build.sh".into()]),
            },
        }
    );
}

#[test]
fn a_name_the_walk_cannot_read_below_the_directory_holds_it_too() {
    let inside = file("scaffolding\n", false);
    let manifest = manifest_of(&[("build.sh/nested/main", recorded(&inside, &[OWNER]))]);
    let observations = observed_with_unreadable(
        &[
            ("build.sh", Observation::Directory),
            ("build.sh/nested", Observation::Directory),
            ("build.sh/nested/main", on_disk(&inside)),
        ],
        &["build.sh/nested"],
    );

    let plan = plan(
        &tree(&[("build.sh", &file("#!/bin/sh\n", true))]),
        &manifest,
        &observations,
        DriftPolicy::Overwrite,
    );

    assert_eq!(
        action(&plan, "build.sh"),
        &Action::Refuse {
            refusal: Refusal::DirectoryInTheWay {
                holding: BTreeMap::new(),
                unreadable: BTreeSet::from(["build.sh/nested".into()]),
            },
        }
    );
}

#[test]
fn a_name_the_walk_cannot_read_refuses_the_drifted_directory_under_either_policy() {
    let manifest = manifest_of(&[("a", recorded(&file("v1\n", false), &[OWNER]))]);
    let observations = observed_with_unreadable(&[("a", Observation::Directory)], &["a"]);
    let expected = Action::Refuse {
        refusal: Refusal::DirectoryInTheWay {
            holding: BTreeMap::new(),
            unreadable: BTreeSet::from(["a".into()]),
        },
    };

    for policy in [DriftPolicy::Refuse, DriftPolicy::Overwrite] {
        let writing = plan(
            &tree(&[("a", &file("v2\n", false))]),
            &manifest,
            &observations,
            policy,
        );
        assert_eq!(action(&writing, "a"), &expected);

        let removing = plan(&tree(&[]), &manifest, &observations, policy);
        assert_eq!(action(&removing, "a"), &expected);
    }
}

#[test]
fn a_name_the_walk_cannot_read_elsewhere_leaves_the_directory_clearable() {
    let inside = file("scaffolding\n", false);
    let manifest = manifest_of(&[("build.sh/main", recorded(&inside, &[OWNER]))]);
    let observations = observed_with_unreadable(
        &[
            ("build.sh", Observation::Directory),
            ("build.sh/main", on_disk(&inside)),
            ("elsewhere", Observation::Directory),
        ],
        &["", "elsewhere"],
    );

    let plan = plan(
        &tree(&[("build.sh", &file("#!/bin/sh\n", true))]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "build.sh"),
        &Action::Write {
            entry: file("#!/bin/sh\n", true),
        }
    );
}

// --- blocks: the region, not the container ---

#[test]
fn a_desired_block_over_an_unrecorded_container_writes() {
    let entry = block("managed\n", Placement::Append);
    let container = file("author\n", false);

    let plan = plan(
        &tree(&[("conf", &entry)]),
        &Manifest::new(),
        &observed(&[("conf", on_disk(&container))]),
        DriftPolicy::Refuse,
    );

    assert_eq!(action(&plan, "conf"), &Action::Write { entry });
}

#[test]
fn a_desired_block_over_an_unrecorded_non_file_refuses_as_foreign() {
    let entry = block("managed\n", Placement::Append);
    for observation in [
        on_disk(&link("elsewhere")),
        Observation::Directory,
        Observation::Other,
    ] {
        let plan = plan(
            &tree(&[("conf", &entry)]),
            &Manifest::new(),
            &observed(&[("conf", observation.clone())]),
            DriftPolicy::Refuse,
        );

        assert_eq!(
            action(&plan, "conf"),
            &Action::Refuse {
                refusal: Refusal::Foreign,
            },
            "{observation:?}"
        );
    }
}

#[test]
fn a_block_never_creates_its_container() {
    let entry = block("managed\n", Placement::Append);
    let recorded_block = manifest_of(&[("conf", recorded(&entry, &[OWNER]))]);
    let cases: &[(Manifest, Observations)] = &[
        (Manifest::new(), observed(&[])),
        (recorded_block, observed(&[("conf", Observation::Absent)])),
    ];

    for (manifest, observations) in cases {
        let plan = plan(
            &tree(&[("conf", &entry)]),
            manifest,
            observations,
            DriftPolicy::Refuse,
        );

        assert_eq!(
            action(&plan, "conf"),
            &Action::Refuse {
                refusal: Refusal::Block {
                    fault: BlockFault::ContainerMissing,
                },
            }
        );
    }
}

#[test]
fn a_region_gone_from_a_standing_container_is_written_again() {
    let entry = block("managed\n", Placement::Append);
    let manifest = manifest_of(&[("conf", recorded(&entry, &[OWNER]))]);

    let plan = plan(
        &tree(&[("conf", &entry)]),
        &manifest,
        &observed(&[("conf", no_region(true))]),
        DriftPolicy::Refuse,
    );

    assert_eq!(action(&plan, "conf"), &Action::Write { entry });
}

#[test]
fn the_marker_and_body_rules_refuse_before_any_classification() {
    let cases: &[(Entry, BlockFault)] = &[
        (
            Entry::Block {
                body: b"managed\n".to_vec(),
                marker: String::new(),
                placement: Placement::Append,
            },
            BlockFault::MarkerEmpty,
        ),
        (
            Entry::Block {
                body: b"managed\n".to_vec(),
                marker: "# a\n# b".to_owned(),
                placement: Placement::Append,
            },
            BlockFault::MarkerNotOneLine,
        ),
        (
            Entry::Block {
                body: b"managed\n".to_vec(),
                marker: "# proiectio ".to_owned(),
                placement: Placement::Append,
            },
            BlockFault::MarkerEdgeWhitespace,
        ),
        (
            block("a\n# proiectio\nb\n", Placement::Append),
            BlockFault::BodyCarriesMarker,
        ),
        (
            block("managed", Placement::Prepend),
            BlockFault::BodyNotNewlineTerminated,
        ),
    ];

    for (entry, fault) in cases {
        let plan = plan(
            &tree(&[("conf", entry)]),
            &Manifest::new(),
            &observed(&[("conf", on_disk(&file("author\n", false)))]),
            DriftPolicy::Refuse,
        );

        assert_eq!(
            action(&plan, "conf"),
            &Action::Refuse {
                refusal: Refusal::Block { fault: *fault },
            },
            "{entry:?}"
        );
    }
}

#[test]
fn appending_needs_an_author_side_that_ends_with_a_newline() {
    let entry = block("managed\n", Placement::Append);
    let manifest = manifest_of(&[("conf", recorded(&entry, &[OWNER]))]);

    let refused = plan(
        &tree(&[("conf", &entry)]),
        &manifest,
        &observed(&[("conf", no_region(false))]),
        DriftPolicy::Refuse,
    );
    let prepended = plan(
        &tree(&[("conf", &block("managed\n", Placement::Prepend))]),
        &Manifest::new(),
        &observed(&[("conf", on_disk(&file("author", false)))]),
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&refused, "conf"),
        &Action::Refuse {
            refusal: Refusal::Block {
                fault: BlockFault::ContainerNotNewlineTerminated,
            },
        }
    );
    assert!(matches!(action(&prepended, "conf"), Action::Write { .. }));
}

#[test]
fn a_changed_marker_overwrites_the_recorded_region() {
    let was = block("managed\n", Placement::Append);
    let now = Entry::Block {
        body: b"managed\n".to_vec(),
        marker: "# renamed".to_owned(),
        placement: Placement::Append,
    };
    let manifest = manifest_of(&[("conf", recorded(&was, &[OWNER]))]);

    let plan = plan(
        &tree(&[("conf", &now)]),
        &manifest,
        &observed(&[("conf", on_disk(&was))]),
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "conf"),
        &Action::Overwrite {
            entry: now,
            expected: signature(&was),
            reason: OverwriteReason::ContentChanged,
        }
    );
}

#[test]
fn a_region_matching_desired_skips() {
    let entry = block("managed\n", Placement::Append);
    let manifest = manifest_of(&[("conf", recorded(&entry, &[OWNER]))]);

    let plan = plan(
        &tree(&[("conf", &entry)]),
        &manifest,
        &observed(&[("conf", on_disk(&entry))]),
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "conf"),
        &Action::Skip {
            entry: entry.clone(),
            expected: signature(&entry),
        }
    );
}

#[test]
fn a_drifted_region_lifts_under_force_and_a_lost_container_does_not() {
    let entry = block("v2\n", Placement::Append);
    let manifest = manifest_of(&[(
        "conf",
        recorded(&block("v1\n", Placement::Append), &[OWNER]),
    )]);
    let edited = edited_region("edited\n", 1);

    let lifted = plan(
        &tree(&[("conf", &entry)]),
        &manifest,
        &observed(&[("conf", edited)]),
        DriftPolicy::Overwrite,
    );
    let refused = plan(
        &tree(&[("conf", &entry)]),
        &manifest,
        &observed(&[("conf", Observation::Directory)]),
        DriftPolicy::Overwrite,
    );

    assert_eq!(
        action(&lifted, "conf"),
        &Action::Overwrite {
            entry,
            expected: NodeSignature {
                kind: EntryKind::Block {
                    marker: MARKER.to_owned(),
                    placement: Placement::Append,
                },
                hash: sha256_hex(b"edited\n"),
                executable: false,
                target: None,
            },
            reason: OverwriteReason::ForcedDrift,
        }
    );
    assert_eq!(
        action(&refused, "conf"),
        &Action::Refuse {
            refusal: Refusal::Drift,
        }
    );
}

#[test]
fn a_second_marker_line_costs_the_region_its_identity_and_every_action_refuses() {
    let entry = block("v2\n", Placement::Append);
    let manifest = manifest_of(&[(
        "conf",
        recorded(&block("v1\n", Placement::Append), &[OWNER]),
    )]);
    let ambiguous = [
        // The author's own bytes down there.
        edited_region("theirs\n", 2),
        // A copy of the recorded region, which would otherwise read clean.
        edited_region("v1\n", 2),
        // A copy of what this run wants, which would otherwise skip.
        edited_region("v2\n", 2),
    ];

    for observation in ambiguous {
        let observations = observed(&[("conf", observation.clone())]);
        for policy in [DriftPolicy::Refuse, DriftPolicy::Overwrite] {
            let overwrite = plan(&tree(&[("conf", &entry)]), &manifest, &observations, policy);
            let removal = removal(RemovalScope::Everything, &manifest, &observations, policy);
            for plan in [&overwrite, &removal] {
                assert_eq!(
                    action(plan, "conf"),
                    &Action::Refuse {
                        refusal: Refusal::Drift,
                    },
                    "{observation:?} {policy:?}"
                );
            }
        }
        assert_eq!(
            classify(&manifest, &observations, None).rows[Utf8Path::new("conf")].verdict,
            PathState::Drifted,
            "{observation:?}"
        );
    }
}

#[test]
fn a_path_never_changes_between_a_whole_node_and_a_block() {
    let as_file = file("whole\n", false);
    let as_block = block("managed\n", Placement::Append);
    let cases: &[(&Entry, &Entry)] = &[(&as_file, &as_block), (&as_block, &as_file)];

    for (was, now) in cases {
        let manifest = manifest_of(&[("conf", recorded(was, &[OWNER]))]);
        let plan = plan(
            &tree(&[("conf", now)]),
            &manifest,
            &observed(&[("conf", on_disk(was))]),
            DriftPolicy::Refuse,
        );

        assert_eq!(
            action(&plan, "conf"),
            &Action::Refuse {
                refusal: Refusal::Block {
                    fault: BlockFault::KindChange,
                },
            },
            "{was:?} -> {now:?}"
        );
    }
}

#[test]
fn an_orphaned_region_is_removed_and_a_vanished_one_expects_nothing() {
    let entry = block("managed\n", Placement::Append);
    let manifest = manifest_of(&[("conf", recorded(&entry, &[OWNER]))]);

    let present = plan(
        &tree(&[]),
        &manifest,
        &observed(&[("conf", on_disk(&entry))]),
        DriftPolicy::Refuse,
    );
    let vanished = plan(
        &tree(&[]),
        &manifest,
        &observed(&[("conf", no_region(true))]),
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&present, "conf"),
        &Action::Remove {
            expected: Some(signature(&entry)),
        }
    );
    assert_eq!(
        action(&vanished, "conf"),
        &Action::Remove { expected: None }
    );
}

// --- removal: whole owner, or a subset by path ---

fn removal(
    scope: RemovalScope<'_>,
    manifest: &Manifest,
    observations: &Observations,
    policy: DriftPolicy,
) -> Plan {
    decide_removal(OWNER, scope, manifest, observations, None, policy)
}

fn requested(paths: &[&str]) -> BTreeSet<Utf8PathBuf> {
    paths.iter().map(Utf8PathBuf::from).collect()
}

#[test]
fn removing_a_whole_owner_is_deciding_against_an_empty_tree() {
    let entry = file("alpha\n", false);
    let manifest = manifest_of(&[
        ("a.txt", recorded(&entry, &[OWNER])),
        ("b/c.txt", recorded(&entry, &[OWNER])),
        ("shared.txt", recorded(&entry, &[OWNER, "other"])),
        ("theirs.txt", recorded(&entry, &["other"])),
    ]);
    let observations = observed(&[
        ("a.txt", on_disk(&entry)),
        ("b/c.txt", on_disk(&entry)),
        ("shared.txt", on_disk(&entry)),
        ("theirs.txt", on_disk(&entry)),
    ]);

    let removal = removal(
        RemovalScope::Everything,
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        removal,
        plan(&tree(&[]), &manifest, &observations, DriftPolicy::Refuse)
    );
    assert_eq!(
        removal.actions,
        BTreeMap::from([
            (
                "a.txt".into(),
                Action::Remove {
                    expected: Some(signature(&entry)),
                },
            ),
            (
                "b/c.txt".into(),
                Action::Remove {
                    expected: Some(signature(&entry)),
                },
            ),
            ("shared.txt".into(), Action::Release),
        ])
    );
}

#[test]
fn a_subset_removal_names_the_only_paths_it_judges() {
    let entry = file("alpha\n", false);
    let manifest = manifest_of(&[
        ("a.txt", recorded(&entry, &[OWNER])),
        ("b/c.txt", recorded(&entry, &[OWNER])),
        ("shared.txt", recorded(&entry, &[OWNER, "other"])),
    ]);
    let observations = observed(&[
        ("a.txt", on_disk(&entry)),
        ("b/c.txt", on_disk(&entry)),
        ("shared.txt", on_disk(&entry)),
    ]);

    let plan = removal(
        RemovalScope::Paths(&requested(&["b/c.txt", "shared.txt"])),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        plan.actions,
        BTreeMap::from([
            (
                "b/c.txt".into(),
                Action::Remove {
                    expected: Some(signature(&entry)),
                },
            ),
            ("shared.txt".into(), Action::Release),
        ])
    );
}

#[test]
fn a_subset_removal_naming_no_path_plans_nothing() {
    let entry = file("alpha\n", false);
    let manifest = manifest_of(&[("a.txt", recorded(&entry, &[OWNER]))]);
    let observations = observed(&[("a.txt", on_disk(&entry))]);

    let plan = removal(
        RemovalScope::Paths(&BTreeSet::new()),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(plan.actions, BTreeMap::new());
}

#[test]
fn a_subset_removal_matches_the_manifest_on_normalized_paths() {
    let entry = file("alpha\n", false);
    let manifest = manifest_of(&[("b/c.txt", recorded(&entry, &[OWNER]))]);
    let observations = observed(&[("b/c.txt", on_disk(&entry))]);

    let plan = removal(
        RemovalScope::Paths(&requested(&["b/x/../c.txt"])),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, "b/c.txt"),
        &Action::Remove {
            expected: Some(signature(&entry)),
        }
    );
}

#[test]
fn requested_paths_pass_the_same_containment_gateway_as_desired_keys() {
    let entry = file("alpha\n", false);
    let manifest = manifest_of(&[("a.txt", recorded(&entry, &[OWNER]))]);
    let observations = observed(&[("a.txt", on_disk(&entry))]);

    let plan = decide_removal(
        OWNER,
        RemovalScope::Paths(&requested(&[
            "../escape",
            "/etc/passwd",
            "a\\b",
            ".proiectio/manifest.json",
            "a.txt",
        ])),
        &manifest,
        &observations,
        Some(Utf8Path::new(".proiectio")),
        DriftPolicy::Refuse,
    );

    for path in [
        "../escape",
        "/etc/passwd",
        "a\\b",
        ".proiectio/manifest.json",
    ] {
        assert_eq!(
            action(&plan, path),
            &Action::Refuse {
                refusal: Refusal::Containment { through: None },
            },
            "expected {path} refused"
        );
    }
    assert_eq!(
        action(&plan, "a.txt"),
        &Action::Remove {
            expected: Some(signature(&entry)),
        }
    );
}

#[test]
fn a_removal_request_the_state_dir_sits_beneath_refuses_containment() {
    let entry = file("alpha\n", false);
    let manifest = manifest_of(&[(".local/share/rc", recorded(&entry, &[OWNER]))]);
    let observations = observed(&[(".local/share/rc", on_disk(&entry))]);

    let plan = decide_removal(
        OWNER,
        RemovalScope::Paths(&requested(&[".local", ".local/share/rc"])),
        &manifest,
        &observations,
        Some(Utf8Path::new(NESTED_STATE)),
        DriftPolicy::Refuse,
    );

    assert_eq!(
        action(&plan, ".local"),
        &Action::Refuse {
            refusal: Refusal::Containment { through: None },
        }
    );
    assert_eq!(
        action(&plan, ".local/share/rc"),
        &Action::Remove {
            expected: Some(signature(&entry)),
        }
    );
}

#[test]
fn a_removal_request_that_escapes_the_destination_refuses_rather_than_reading_as_unheld() {
    let manifest = manifest_of(&[("a.txt", recorded(&file("alpha\n", false), &[OWNER]))]);
    let observations = observed(&[("a.txt", on_disk(&file("alpha\n", false)))]);
    let escaping = [
        "../ESCAPE/x",
        "/etc/passwd",
        "a\\b",
        "a/../../ESCAPE",
        "./a.txt",
    ];

    let plan = removal(
        RemovalScope::Paths(&requested(&escaping)),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    for path in escaping {
        assert_eq!(
            action(&plan, path),
            &Action::Refuse {
                refusal: Refusal::Containment { through: None },
            },
            "expected {path} refused"
        );
    }
    assert_eq!(plan.actions.len(), escaping.len());
}

#[test]
fn naming_a_path_this_owner_does_not_hold_says_so_and_changes_nothing() {
    let entry = file("alpha\n", false);
    let manifest = manifest_of(&[("theirs.txt", recorded(&entry, &["other"]))]);
    let observations = observed(&[
        ("theirs.txt", on_disk(&entry)),
        ("foreign.txt", on_disk(&entry)),
    ]);

    let plan = removal(
        RemovalScope::Paths(&requested(&["gone.txt", "foreign.txt", "theirs.txt", "b"])),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(
        plan.actions,
        BTreeMap::from([
            ("b".into(), Action::NotRecorded),
            ("foreign.txt".into(), Action::NotRecorded),
            ("gone.txt".into(), Action::NotRecorded),
            ("theirs.txt".into(), Action::NotRecorded),
        ])
    );
}

#[test]
fn a_manifest_key_that_escapes_the_destination_refuses_rather_than_planning_a_removal() {
    let entry = file("alpha\n", false);
    let manifest = manifest_of(&[
        ("../ESCAPE/x", recorded(&entry, &[OWNER])),
        ("/etc/passwd", recorded(&entry, &[OWNER])),
        ("a\\b", recorded(&entry, &[OWNER])),
        ("a.txt", recorded(&entry, &[OWNER])),
    ]);
    let observations = observed(&[("a.txt", on_disk(&entry))]);

    let swept = removal(
        RemovalScope::Everything,
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    for path in ["../ESCAPE/x", "/etc/passwd", "a\\b"] {
        assert_eq!(
            action(&swept, path),
            &Action::Refuse {
                refusal: Refusal::Containment { through: None },
            },
            "expected {path} refused"
        );
    }
    assert_eq!(
        action(&swept, "a.txt"),
        &Action::Remove {
            expected: Some(signature(&entry)),
        }
    );
    let projected = plan(
        &tree(&[("a.txt", &entry)]),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );
    for path in ["../ESCAPE/x", "/etc/passwd", "a\\b"] {
        assert_eq!(
            action(&projected, path),
            &Action::Refuse {
                refusal: Refusal::Containment { through: None },
            },
            "expected {path} refused"
        );
    }
}

#[test]
fn removing_a_drifted_path_refuses_and_the_policy_lifts_it() {
    let entry = file("alpha\n", false);
    let drifted = file("edited by hand\n", false);
    let manifest = manifest_of(&[("a.txt", recorded(&entry, &[OWNER]))]);
    let observations = observed(&[("a.txt", on_disk(&drifted))]);
    let named = requested(&["a.txt"]);

    for scope in [RemovalScope::Everything, RemovalScope::Paths(&named)] {
        assert_eq!(
            action(
                &removal(scope, &manifest, &observations, DriftPolicy::Refuse),
                "a.txt"
            ),
            &Action::Refuse {
                refusal: Refusal::Drift,
            }
        );
        assert_eq!(
            action(
                &removal(scope, &manifest, &observations, DriftPolicy::Overwrite),
                "a.txt"
            ),
            &Action::Remove {
                expected: Some(signature(&drifted)),
            }
        );
    }
}

#[test]
fn removing_an_already_gone_path_drops_the_entry_alone() {
    let entry = file("alpha\n", false);
    let manifest = manifest_of(&[("a.txt", recorded(&entry, &[OWNER]))]);
    let observations = observed(&[("a.txt", Observation::Absent)]);

    let plan = removal(
        RemovalScope::Paths(&requested(&["a.txt"])),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(action(&plan, "a.txt"), &Action::Remove { expected: None });
}

// --- determinism ---

#[test]
fn plans_are_byte_identical_for_identical_inputs() {
    let entry = file("alpha\n", false);
    let drifted = file("edited\n", true);
    let desired = tree(&[
        ("a.txt", &entry),
        ("b/c.txt", &file("nested\n", true)),
        ("../escape", &entry),
        ("rc", &link("../shared/rc")),
    ]);
    let manifest = manifest_of(&[
        ("a.txt", recorded(&entry, &[OWNER])),
        ("old.txt", recorded(&file("old\n", false), &[OWNER])),
        (
            "shared.txt",
            recorded(&file("s\n", false), &[OWNER, "other"]),
        ),
    ]);
    let observations = observed(&[
        ("a.txt", on_disk(&drifted)),
        ("old.txt", Observation::Absent),
        ("shared.txt", on_disk(&file("s\n", false))),
        ("foreign.txt", on_disk(&file("f\n", false))),
    ]);

    let first = plan(&desired, &manifest, &observations, DriftPolicy::Overwrite);
    let second = plan(&desired, &manifest, &observations, DriftPolicy::Overwrite);

    assert_eq!(format!("{first:?}"), format!("{second:?}"));
}

#[test]
fn a_plan_carries_the_source_of_each_path_a_source_named() {
    let entry = file("x\n", false);
    let mapping = Origin::Mapping {
        path: "/maps/deploy.toml".into(),
    };
    let walked = Origin::Tree {
        path: "/srv/skeleton".into(),
    };
    let mut desired = Desired::new();
    desired.insert("a.txt".into(), entry.clone(), Origin::Caller);
    desired.insert("b.txt".into(), entry.clone(), mapping.clone());
    desired.insert("c.txt".into(), entry, walked.clone());

    let plan = plan(
        &desired,
        &Manifest::new(),
        &observed(&[]),
        DriftPolicy::Refuse,
    );

    assert_eq!(
        plan.origins,
        BTreeMap::from([
            (Utf8PathBuf::from("b.txt"), mapping),
            (Utf8PathBuf::from("c.txt"), walked),
        ])
    );
}
