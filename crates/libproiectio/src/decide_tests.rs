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
        Entry::Block { .. } => panic!("no whole-node observation holds a block"),
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

/// [`decide`] for [`OWNER`] with no in-dest state prefix.
fn plan(
    desired: &BTreeMap<Utf8PathBuf, Entry>,
    manifest: &Manifest,
    observations: &Observations,
    policy: DriftPolicy,
) -> Plan {
    decide(OWNER, desired, manifest, observations, None, policy)
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
fn a_recorded_block_classifies_drifted_until_region_classification() {
    // Seam: a block's hash covers its delimited body, which no whole-node
    // observation reproduces — conservative drift until the block issue
    // replaces this with region classification.
    let body = b"managed\n".to_vec();
    let container = "before\nmanaged\nafter\n";
    let manifest = manifest_of(&[(
        "conf",
        ManifestEntry {
            kind: EntryKind::Block,
            hash: sha256_hex(&body),
            executable: false,
            owners: BTreeSet::from([OWNER.to_owned()]),
        },
    )]);
    let observations = observed(&[("conf", on_disk(&file(container, false)))]);

    let status = classify(&manifest, &observations, None);

    assert_eq!(status.paths[Utf8Path::new("conf")], PathState::Drifted);
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
        DriftPolicy::Refuse,
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
        DriftPolicy::Refuse,
    );

    assert_eq!(
        plan.actions.keys().collect::<Vec<_>>(),
        [Utf8Path::new("a.txt")]
    );
}

// --- kind-agnostic comparison: symlinks through the generic table ---

#[test]
fn a_desired_link_matching_the_disk_target_skips() {
    let entry = link("../shared/rc");
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

// --- blocks: the generic-table seam ---

#[test]
fn a_drifted_recorded_block_is_never_lifted() {
    // A recorded block owns only its delimited region, which no
    // whole-node signature expresses: lifting would plan whole-file
    // actions against a container the projection does not own whole, so
    // the drift refusal holds under either policy — for a changed desired
    // block and for an orphaned one alike.
    let recorded_block = Entry::Block {
        body: b"v1\n".to_vec(),
    };
    let desired_block = Entry::Block {
        body: b"v2\n".to_vec(),
    };
    let container = file("# config\nmanaged v1\n", false);
    let manifest = manifest_of(&[("conf", recorded(&recorded_block, &[OWNER]))]);
    let observations = observed(&[("conf", on_disk(&container))]);

    let changed = plan(
        &tree(&[("conf", &desired_block)]),
        &manifest,
        &observations,
        DriftPolicy::Overwrite,
    );
    let orphaned = plan(&tree(&[]), &manifest, &observations, DriftPolicy::Overwrite);

    for plan in [&changed, &orphaned] {
        assert_eq!(
            action(plan, "conf"),
            &Action::Refuse {
                refusal: Refusal::Drift,
            }
        );
    }
}

#[test]
fn a_desired_block_over_nothing_writes() {
    let entry = Entry::Block {
        body: b"managed\n".to_vec(),
    };

    let plan = plan(
        &tree(&[("conf", &entry)]),
        &Manifest::new(),
        &observed(&[]),
        DriftPolicy::Refuse,
    );

    assert_eq!(action(&plan, "conf"), &Action::Write { entry });
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
