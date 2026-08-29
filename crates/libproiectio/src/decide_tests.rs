use super::*;

/// The owner every test plans for unless it says otherwise.
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

/// The signature of a disk node holding exactly `entry`.
fn signature(entry: &Entry) -> NodeSignature {
    NodeSignature {
        kind: entry.kind(),
        hash: desired_hash(entry),
        executable: desired_executable(entry),
    }
}

/// The manifest entry recording exactly `entry`, held by `owners`.
fn recorded(entry: &Entry, owners: &[&str]) -> ManifestEntry {
    ManifestEntry {
        kind: entry.kind(),
        hash: desired_hash(entry),
        executable: desired_executable(entry),
        owners: owners.iter().map(|owner| (*owner).to_owned()).collect(),
    }
}

/// The observation of a disk node holding exactly `entry`.
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
        // A region on disk: the author's side ends at the marker's line
        // start, which is newline-terminated by construction.
        Entry::Block { body, .. } => Observation::Block {
            hash: Some(sha256_hex(body)),
            newline_terminated: true,
        },
    }
}

/// The marker every block test uses.
const MARKER: &str = "# proiectio";

fn block(body: &str, placement: Placement) -> Entry {
    Entry::Block {
        body: body.as_bytes().to_vec(),
        marker: MARKER.to_owned(),
        placement,
    }
}

/// The observation of a container the region is gone from, whose author's
/// side is newline-terminated or not.
fn no_region(newline_terminated: bool) -> Observation {
    Observation::Block {
        hash: None,
        newline_terminated,
    }
}

fn tree(entries: &[(&str, &Entry)]) -> BTreeMap<Utf8PathBuf, Entry> {
    entries
        .iter()
        .map(|(path, entry)| (Utf8PathBuf::from(*path), (*entry).clone()))
        .collect()
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

fn observed(paths: &[(&str, Observation)]) -> Observations {
    Observations {
        paths: paths
            .iter()
            .map(|(path, observation)| (Utf8PathBuf::from(*path), observation.clone()))
            .collect(),
    }
}

/// [`decide`] for [`OWNER`] with no in-dest state prefix, under `policy`
/// and the default (refusing) external-target policy.
fn plan(
    desired: &BTreeMap<Utf8PathBuf, Entry>,
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

/// [`plan`] under options the test chooses whole.
fn plan_with(
    desired: &BTreeMap<Utf8PathBuf, Entry>,
    manifest: &Manifest,
    observations: &Observations,
    options: PlanOptions,
) -> Plan {
    decide(OWNER, desired, manifest, observations, None, options)
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

    assert_eq!(status.paths[Utf8Path::new("a.txt")], PathState::Clean);
}

#[test]
fn drifted_when_bytes_differ() {
    let manifest = manifest_of(&[("a.txt", recorded(&file("alpha\n", false), &[OWNER]))]);
    let observations = observed(&[("a.txt", on_disk(&file("edited\n", false)))]);

    let status = classify(&manifest, &observations, None);

    assert_eq!(status.paths[Utf8Path::new("a.txt")], PathState::Drifted);
}

#[test]
fn drifted_when_the_executable_bit_differs() {
    let manifest = manifest_of(&[("run.sh", recorded(&file("#!/bin/sh\n", false), &[OWNER]))]);
    let observations = observed(&[("run.sh", on_disk(&file("#!/bin/sh\n", true)))]);

    let status = classify(&manifest, &observations, None);

    assert_eq!(status.paths[Utf8Path::new("run.sh")], PathState::Drifted);
}

#[test]
fn drifted_when_the_kind_differs() {
    let manifest = manifest_of(&[("a", recorded(&file("alpha\n", false), &[OWNER]))]);
    let observations = observed(&[("a", on_disk(&link("alpha\n")))]);

    let status = classify(&manifest, &observations, None);

    assert_eq!(status.paths[Utf8Path::new("a")], PathState::Drifted);
}

#[test]
fn drifted_when_a_recorded_path_is_now_a_directory() {
    let manifest = manifest_of(&[("a", recorded(&file("alpha\n", false), &[OWNER]))]);
    let observations = observed(&[("a", Observation::Directory)]);

    let status = classify(&manifest, &observations, None);

    assert_eq!(status.paths[Utf8Path::new("a")], PathState::Drifted);
}

#[test]
fn missing_when_a_recorded_path_is_gone() {
    let manifest = manifest_of(&[("a.txt", recorded(&file("alpha\n", false), &[OWNER]))]);
    let observations = observed(&[("a.txt", Observation::Absent)]);

    let status = classify(&manifest, &observations, None);

    assert_eq!(status.paths[Utf8Path::new("a.txt")], PathState::Missing);
}

#[test]
fn missing_when_the_snapshot_lacks_a_recorded_path() {
    // observe completes the union with Absent; classify does not depend
    // on it and treats an unmentioned recorded path the same way.
    let manifest = manifest_of(&[("a.txt", recorded(&file("alpha\n", false), &[OWNER]))]);
    let observations = observed(&[]);

    let status = classify(&manifest, &observations, None);

    assert_eq!(status.paths[Utf8Path::new("a.txt")], PathState::Missing);
}

#[test]
fn foreign_when_on_disk_and_unrecorded() {
    let manifest = Manifest::new();
    let observations = observed(&[("notes.txt", on_disk(&file("mine\n", false)))]);

    let status = classify(&manifest, &observations, None);

    assert_eq!(status.paths[Utf8Path::new("notes.txt")], PathState::Foreign);
}

#[test]
fn an_unrecorded_directory_classifies_foreign() {
    // The manifest records no directories, so every observed directory is
    // unrecorded; planning refuses it only where the desired tree names
    // that exact path.
    let status = classify(
        &Manifest::new(),
        &observed(&[("existing", Observation::Directory)]),
        None,
    );

    assert_eq!(status.paths[Utf8Path::new("existing")], PathState::Foreign);
}

#[test]
fn an_unrecorded_node_of_another_kind_classifies_foreign() {
    let status = classify(
        &Manifest::new(),
        &observed(&[("pipe", Observation::Other)]),
        None,
    );

    assert_eq!(status.paths[Utf8Path::new("pipe")], PathState::Foreign);
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

    assert_eq!(status.paths[Utf8Path::new("kept")], PathState::Clean);
    assert_eq!(status.paths[Utf8Path::new("moved")], PathState::Drifted);
}

#[test]
fn a_link_target_edited_to_non_utf8_classifies_drifted() {
    let entry = link("ok");
    let manifest = manifest_of(&[("l", recorded(&entry, &[OWNER]))]);
    // observe hashes the raw target bytes and reports `target: None`; the
    // hash can match no recorded UTF-8 target, so classification is drift.
    let observations = observed(&[(
        "l",
        Observation::Symlink {
            hash: sha256_hex(b"\xff\xfe"),
            target: None,
        },
    )]);

    let status = classify(&manifest, &observations, None);

    assert_eq!(status.paths[Utf8Path::new("l")], PathState::Drifted);
}

#[test]
fn a_recorded_block_classifies_over_its_region() {
    // The region is the node: the container's other bytes enter no
    // comparison, and a container the marker line is gone from reads as a
    // node gone from disk.
    let entry = block("managed\n", Placement::Append);
    let manifest = manifest_of(&[("conf", recorded(&entry, &[OWNER]))]);
    let cases: &[(Observation, PathState)] = &[
        (on_disk(&entry), PathState::Clean),
        (
            Observation::Block {
                hash: Some(sha256_hex(b"edited\n")),
                newline_terminated: true,
            },
            PathState::Drifted,
        ),
        (no_region(true), PathState::Missing),
        (Observation::Absent, PathState::Missing),
        // The container is no longer a file at all: drift of kind.
        (Observation::Directory, PathState::Drifted),
    ];
    for (observation, want) in cases {
        let status = classify(&manifest, &observed(&[("conf", observation.clone())]), None);
        assert_eq!(
            status.paths[Utf8Path::new("conf")],
            *want,
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
        status.paths.keys().collect::<Vec<_>>(),
        [Utf8Path::new("a.txt")]
    );
}

// --- the action table (`docs/design.lex` section 2), row by row ---

#[test]
fn disk_already_equal_to_desired_skips() {
    // Row 1: disk already equals desired — skip, so re-applying is a
    // no-op and mtimes survive.
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
            expected: signature(&entry),
        }
    );
    assert_eq!(plan.owner, OWNER);
}

#[test]
fn clean_disk_with_changed_desired_overwrites() {
    // Row 2: disk equals recorded and desired differs — overwrite,
    // expecting the recorded hash at apply time.
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
        }
    );
}

#[test]
fn a_drifted_path_refuses_and_names_it() {
    // Row 3: drifted — refuse and name the path.
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
    // Row 3, lifted: the overwrite expects the hash of the *drifted*
    // bytes observed at plan time, not the recorded ones, so apply's
    // re-check runs against what the lift was granted for.
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
        }
    );
}

#[test]
fn a_path_edited_into_agreement_with_desired_skips() {
    // Row 1 beats row 3: the user edited the file to exactly the desired
    // bytes — disk already equals desired, so nothing needs writing.
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
            expected: signature(&entry),
        }
    );
}

#[test]
fn an_agreement_skip_carries_the_desired_signature() {
    // The recorded entry differs from the desired one (here in the
    // executable bit alone), so the skip must carry the full desired
    // signature for apply to record — the hash by itself could not tell
    // apply the bit changed.
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
            expected: NodeSignature {
                kind: EntryKind::File,
                hash: desired_hash(&agreed),
                executable: true,
            },
        }
    );
}

#[test]
fn a_foreign_path_refuses_always() {
    // Row 4: foreign — refuse; no policy lifts it.
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
    // Skipping would adopt the file — record it, and put it on the
    // removal path of a later plan the user never signed it up for.
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
fn a_desired_path_over_a_foreign_directory_refuses() {
    let plan = plan(
        &tree(&[("existing", &file("now a file\n", false))]),
        &Manifest::new(),
        &observed(&[("existing", Observation::Directory)]),
        DriftPolicy::Overwrite,
    );

    assert_eq!(
        action(&plan, "existing"),
        &Action::Refuse {
            refusal: Refusal::Foreign,
        }
    );
}

#[test]
fn an_orphan_removes_when_disk_matches_the_recorded_hash() {
    // Row 5: recorded under this owner, absent from the desired tree.
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
    // Row 5: refused as drifted when the disk no longer matches.
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
fn a_missing_orphan_still_plans_removal() {
    // Already gone from disk: the Remove expects nothing, so apply drops
    // the manifest entry alone — and refuses if a node has appeared at
    // the path since the plan, even one matching the recorded signature.
    let entry = file("old\n", false);
    let manifest = manifest_of(&[("old.txt", recorded(&entry, &[OWNER]))]);
    let observations = observed(&[("old.txt", Observation::Absent)]);

    let plan = plan(&tree(&[]), &manifest, &observations, DriftPolicy::Refuse);

    assert_eq!(action(&plan, "old.txt"), &Action::Remove { expected: None });
}

#[test]
fn a_shared_orphan_releases_the_departing_owner() {
    // Row 5: when another owner still holds the path, the departing owner
    // is released and the disk is left alone.
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
    // This owner holds the path too; changing it still needs the other
    // owner's tree to agree first.
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

    // Refusals are keyed by the desired key verbatim.
    for refused in ["../escape", "/absolute", "a/./b"] {
        assert_eq!(
            action(&plan, refused),
            &Action::Refuse {
                refusal: Refusal::Containment,
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
            expected: signature(&entry),
        }
    );
    assert_eq!(plan.actions.len(), 1);
}

#[test]
fn two_desired_keys_normalizing_to_one_path_refuse_both() {
    // The tree claims one location twice; there is no deterministic entry
    // to prefer, so both claims refuse, keyed verbatim, each naming the
    // other. An unrelated key still plans.
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
    // Every desired entry is a non-directory, so a tree naming both `a`
    // and `a/b` cannot be applied in any order: whichever lands first
    // makes the other impossible. Both claims refuse.
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
    // `a`, `a/b`, and `a/b/c` overlap pairwise; each refusal names every
    // key it collides with, ancestors and descendants alike.
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
    // The desired tree still names the location — conflictedly — so the
    // orphan pass must not treat it as dropped and overwrite the refusal
    // with a removal (or a release, for a shared entry).
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
    );

    for refused in [".proiectio/manifest.json", ".proiectio"] {
        assert_eq!(
            action(&plan, refused),
            &Action::Refuse {
                refusal: Refusal::Containment,
            },
            "expected a containment refusal at {refused}"
        );
    }
    // The prefix confines a subtree, not a name: the same name elsewhere
    // is an ordinary path.
    assert_eq!(
        action(&plan, "elsewhere/.proiectio"),
        &Action::Write { entry }
    );
}

#[test]
fn the_state_subtree_is_invisible_to_planning() {
    // The state directory's own files observe as on-disk nodes, but never
    // classify — so they are not foreign and get no action.
    let entry = file("alpha\n", false);
    let manifest = manifest_of(&[("a.txt", recorded(&entry, &[OWNER]))]);
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
    );

    assert_eq!(
        plan.actions.keys().collect::<Vec<_>>(),
        [Utf8Path::new("a.txt")]
    );
}

// --- symlink target grading (`docs/security.lex` section 3) ---

/// The options permitting external targets, drift refused as usual.
fn allowing_external() -> PlanOptions {
    PlanOptions {
        external_targets: ExternalTargetPolicy::Allow,
        ..PlanOptions::default()
    }
}

#[test]
fn target_grading_admits_in_dest_targets_and_refuses_escaping_ones() {
    // (link path, target, resolves in-dest). The destination here holds
    // nothing, so the chain has no hop to follow and every verdict is the
    // one pure lexical resolution from the link's parent gives.
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
        // A colon that is a name, not a drive: an NTFS stream of a sibling
        // under the destination.
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
        // The permission lifts exactly the external refusal; an in-dest
        // target never needed it.
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
    // Judged before grading, and not lifted by the external-target
    // permission: the empty string names nothing and a NUL cannot appear
    // in a pathname, so there is no pointer for a policy to permit.
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
    // Nothing about the disk lifts the refusal: the permission is the
    // invoker's, and it is the same link either way.
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
            expected: signature(&entry),
        }
    );
}

#[test]
fn a_recorded_external_link_the_tree_dropped_is_removed_without_permission() {
    // Grading judges what the tree asks for. An orphan asks for nothing —
    // removal unlinks the pointer and reads nothing through it.
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

// --- grading through the destination's own links (issue #29) ---

#[test]
fn a_target_reaching_outside_through_a_link_in_the_destination_grades_external() {
    // The pivot case: the destination already holds `pivot -> /etc`, so
    // the pointer `evil -> pivot/passwd` dereferences to /etc/passwd. The
    // projection could not have created that hop — `pivot` itself would
    // have needed the permission — but it may not plant a pointer through
    // one either.
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
    // `shared -> real` is an in-dest link like any other, so `rc`
    // pointing through it lands in-dest and is written under the default
    // policy. Refusing every target with a symlink ancestor would break
    // exactly this shape.
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
    // The chain runs out at a link pointing nowhere. A pointer to nothing
    // is still a pointer inside the destination, so no permission is
    // needed — the same reading that makes a dangling target legal.
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
    // Deciding terminates: the visited set ends the resolution at the
    // second visit to a link, and a chain that never lands grades
    // external.
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
    // Nothing can say where such a link points, so nothing can say the
    // chain through it stays inside — the same conservatism apply's walk
    // applies when it refuses to follow one.
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
    // Removals run before anything is written, so by the time the pointer
    // is published the pivot is gone and the chain ends at an absent
    // path. Grading reads the destination the run will leave, exactly as
    // the no-alias rule does for an ancestor the plan unlinks.
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
    // Apply reads it to know whether a re-graded target has a plan-time
    // verdict to be held to.
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
    // Both links grade in-dest read one at a time: "." lands on the
    // destination itself, and "b/../escape" lands on "escape" where "b" is
    // an ordinary name. Together they are a pointer to the destination's
    // *parent*, because "b/.." pops the directory "b" resolved to. Grading
    // reads the destination the run leaves, so the second link is the first
    // one's hop and the pointer grades external.
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
    // A tree the run has not written yet is still the destination the run
    // leaves, so a cycle among its own links is graded like one already on
    // disk. Reading only the snapshot would write the cycle on the first
    // run and refuse the identical tree on the second, once the links it
    // wrote were observable.
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
    // The shape the issue names as the one that must not start needing the
    // permission, with the pivot projected by this run rather than already
    // on disk.
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
    // The destination holds an escaping pivot and the tree replaces it with
    // an in-dest link. The pointer through it is graded against the link
    // the run leaves, not the one it displaces, so nothing about the run's
    // finished destination reaches outside and the permission is not needed.
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
    // The nesting is expressible on disk — apply would follow the link —
    // but the write would land at a path the plan never names.
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
    // The link is on disk and stays there — recorded under another owner
    // here, but an unowned one refuses the same way, since observation
    // never descends either.
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
                refusal: Refusal::Containment,
            }
        );
    }
}

#[test]
fn a_desired_path_beneath_a_link_this_plan_removes_is_written() {
    // The orphan removal runs first, so the link is not an ancestor the
    // write will meet: the path plans as the ordinary directory write it
    // becomes.
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
    // A release leaves the link on disk for its other owner, so the path
    // beneath it still resolves through a link.
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
            refusal: Refusal::Containment,
        }
    );
}

// The two tests below are why deciding may grade a target from the lexical
// `link.parent()` while apply grades it from the parent its walk resolved
// to. The two disagree only for a link with a symlink ancestor, and the
// tree that would exhibit the disagreement is refused from both directions:
// a desired ancestor link by the overlap check, an observed one by the
// no-alias rule. Each test states the divergent verdict alongside the
// refusal, so removing either guard turns the refusal into a `Write` here
// and fails, rather than leaving apply to catch the escape.

#[test]
fn a_desired_link_beneath_a_desired_link_refuses_before_the_verdicts_diverge() {
    // `b/c/x` spells two climbs. From `b/c`, where the tree writes it, they
    // pop `c` and `b` and land on `escape` inside the destination. From
    // `real`, where apply's walk would follow `b/c` to, the second climb
    // pops past the destination root.
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
    // The same target and the same divergence, with the ancestor link
    // already on disk instead of in the tree. The second assertion is the
    // verdict apply would reach: written at the location `b/c` resolves to,
    // the identical target grades external.
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
            refusal: Refusal::Containment,
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
        }
    );
}

#[test]
fn a_desired_file_over_a_recorded_link_is_drift_of_kind_when_the_link_moved() {
    // Recorded link, disk target edited: drifted; the desired kind change
    // rides the same drift rules as byte edits.
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
fn drift_to_a_directory_stays_refused_under_overwrite_policy() {
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
        &Action::Refuse {
            refusal: Refusal::Drift,
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
}

// --- blocks: the region, not the container ---

#[test]
fn a_desired_block_over_an_unrecorded_container_writes() {
    // Writing into a file it does not own whole is what a block is for, so
    // an unrecorded container is not a foreign refusal. Only apply's read of
    // the bytes can tell an untouched container from one already carrying a
    // region, so the plan is a write either way.
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
    // Neither over a path nothing was ever at, nor over a recorded region
    // whose container was deleted: a projection that made the file would own
    // it whole, which is what a `File` entry is for.
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
    // Missing, so write heals — the container is still there to splice into.
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
    // Neither side's bytes get normalized to make room, so a container whose
    // last line has no terminator refuses rather than gaining one.
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
    // Prepending puts the author's side last, so its terminator is theirs.
    assert!(matches!(action(&prepended, "conf"), Action::Write { .. }));
}

#[test]
fn a_changed_marker_overwrites_the_recorded_region() {
    // The desired kind carries the marker, so changing it makes the desired
    // entry differ from the recorded one: apply strips the region the old
    // marker locates and splices the new one in a single publish.
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
            expected: signature(&entry),
        }
    );
}

#[test]
fn a_drifted_region_lifts_under_force_and_a_lost_container_does_not() {
    // The crate's ordinary rule: a drift lifts where the observed node
    // carries a signature apply can re-verify, which for a block is the body
    // the recorded marker locates. A container that became a directory
    // carries no such signature, so `--force` still refuses it.
    let entry = block("v2\n", Placement::Append);
    let manifest = manifest_of(&[(
        "conf",
        recorded(&block("v1\n", Placement::Append), &[OWNER]),
    )]);
    let edited = Observation::Block {
        hash: Some(sha256_hex(b"edited\n")),
        newline_terminated: true,
    };

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
            },
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

/// [`decide_removal`] for [`OWNER`] with no in-dest state prefix, under
/// `policy` and the default (refusing) external-target policy.
fn removal(
    scope: RemovalScope<'_>,
    manifest: &Manifest,
    observations: &Observations,
    policy: DriftPolicy,
) -> Plan {
    decide_removal(
        OWNER,
        scope,
        manifest,
        observations,
        None,
        PlanOptions {
            drift: policy,
            ..PlanOptions::default()
        },
    )
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
            // Another owner still holds it: the disk is left alone.
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

    // a.txt is not named: no action at all, so its entry and its node
    // both survive the run.
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

    // Clearing the owner is `Everything`, never an empty list: a caller
    // passing along a path list that came up empty removes nothing.
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
        PlanOptions::default(),
    );

    // Refusals are keyed by the request verbatim, exactly as a refused
    // desired key is.
    for path in [
        "../escape",
        "/etc/passwd",
        "a\\b",
        ".proiectio/manifest.json",
    ] {
        assert_eq!(
            action(&plan, path),
            &Action::Refuse {
                refusal: Refusal::Containment,
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
fn naming_a_path_this_owner_does_not_hold_plans_nothing() {
    let entry = file("alpha\n", false);
    let manifest = manifest_of(&[("theirs.txt", recorded(&entry, &["other"]))]);
    let observations = observed(&[
        ("theirs.txt", on_disk(&entry)),
        ("foreign.txt", on_disk(&entry)),
    ]);

    // Never recorded, recorded under another owner alone, and a directory
    // (which the manifest never records): a removal owes nothing at any of
    // them, so re-running one that already succeeded stays a no-op.
    let plan = removal(
        RemovalScope::Paths(&requested(&["gone.txt", "foreign.txt", "theirs.txt", "b"])),
        &manifest,
        &observations,
        DriftPolicy::Refuse,
    );

    assert_eq!(plan.actions, BTreeMap::new());
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
        // Overwrite lifts it to a removal expecting the *drifted* node, so
        // apply still refuses if the file changes again after the plan.
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

    let first = serde_json::to_vec(&first).expect("serialize");
    let second = serde_json::to_vec(&second).expect("serialize");
    assert_eq!(first, second);
}
