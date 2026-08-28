use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};

use crate::containment::contained_normalize;
use crate::{
    Action, DriftPolicy, Entry, EntryKind, Manifest, ManifestEntry, Observation, Observations,
    PathState, Plan, Refusal, Status, sha256_hex,
};

/// The pure classification: one [`PathState`] per path in the union of the
/// manifest and the observed destination (`docs/design.lex` section 2).
///
/// No filesystem — the disk is consumed as the [`Observations`] snapshot,
/// so the same inputs always classify identically. [`decide`] runs this and
/// compares the result against the desired tree; status is this
/// classification with nothing written, over observations taken fresh.
///
/// Per path:
///
/// - recorded and observed [`Absent`](Observation::Absent) — or, defensively,
///   recorded but missing from the snapshot — [`Missing`](PathState::Missing);
/// - recorded and the observation matches the recorded entry — hash and
///   kind, plus the executable bit for files — [`Clean`](PathState::Clean);
/// - recorded and the observation differs in any of those —
///   [`Drifted`](PathState::Drifted). A recorded path whose node is now a
///   directory, or a kind the projection never writes
///   ([`Other`](Observation::Other)), is drift of kind;
/// - on disk and unrecorded — [`Foreign`](PathState::Foreign). Directories
///   included: the manifest records no directories, so every observed
///   directory is unrecorded and classifies foreign — planning refuses it
///   only where the desired tree names that exact path, and writing files
///   *inside* an existing directory is the merge `docs/design.lex` promises.
///
/// `state_prefix` is the projection's state directory as a path relative to
/// the destination, when it lies inside it
/// ([`Projection::state_prefix`](crate::Projection::state_prefix)); the
/// subtree under it — the prefix itself included — is the projection's own
/// state and never classifies.
///
/// Seam: a recorded [`Block`](EntryKind::Block) entry hashes its delimited
/// body, which no whole-node observation reproduces, so until block-region
/// classification lands a recorded block always classifies
/// [`Drifted`](PathState::Drifted) — conservative, since drift refuses.
pub fn classify(
    manifest: &Manifest,
    observations: &Observations,
    state_prefix: Option<&Utf8Path>,
) -> Status {
    let mut paths = BTreeMap::new();
    for (path, observation) in &observations.paths {
        if in_state(path, state_prefix) {
            continue;
        }
        let state = match (manifest.entries.get(path), observation) {
            (Some(_), Observation::Absent) => PathState::Missing,
            (Some(recorded), observation) => {
                if observation_matches_recorded(recorded, observation) {
                    PathState::Clean
                } else {
                    PathState::Drifted
                }
            }
            // Only recorded paths observe Absent; an unrecorded one is
            // neither on disk nor in the manifest — nothing to classify.
            (None, Observation::Absent) => continue,
            (None, _) => PathState::Foreign,
        };
        paths.insert(path.clone(), state);
    }
    for path in manifest.entries.keys() {
        if in_state(path, state_prefix) {
            continue;
        }
        paths.entry(path.clone()).or_insert(PathState::Missing);
    }
    Status { paths }
}

/// The pure deciding stage: `(desired, manifest, observations) -> Plan`
/// (`docs/implementation.lex` section 1). No filesystem — identical inputs
/// produce byte-identical plans.
///
/// Every desired-tree key is admitted through the containment gateway
/// ([`contained_join`](crate::contained_join)'s lexical contract): a key the
/// gateway refuses, or one entering the state-directory subtree named by
/// `state_prefix` ([`Projection::state_prefix`](crate::Projection::state_prefix)),
/// gets [`Refusal::Containment`] keyed by the key verbatim. Admitted keys
/// land in the plan lexically normalized (`a/../b` plans as `b`), so one
/// on-disk location has one action.
///
/// Each admitted path is then judged against its classification
/// ([`classify`]) per the `docs/design.lex` section 2 action table:
///
/// - never recorded and not on disk — [`Write`](Action::Write); a
///   [`Missing`](PathState::Missing) recorded path is written again — write
///   heals;
/// - on disk equal to desired — [`Skip`](Action::Skip), whether the path is
///   clean or was edited into agreement; this is also how an owner joins a
///   path another owner holds identically;
/// - [`Clean`](PathState::Clean) with desired differing —
///   [`Overwrite`](Action::Overwrite) expecting the recorded hash;
/// - [`Drifted`](PathState::Drifted) with desired differing —
///   [`Refusal::Drift`], unless `policy` is [`DriftPolicy::Overwrite`],
///   which plans an [`Overwrite`](Action::Overwrite) expecting the hash of
///   the *drifted* bytes observed at plan time;
/// - [`Foreign`](PathState::Foreign) — [`Refusal::Foreign`], always: no
///   policy lifts it, identical bytes included — adopting a file the
///   projection did not write would put it on the removal path;
/// - recorded by other owners with a desired entry differing from the
///   recorded one — [`Refusal::OwnerConflict`] naming the other owners: two
///   owners hold one path only while writing identical entries.
///
/// Recorded paths absent from the desired tree:
///
/// - held by other owners too — [`Release`](Action::Release): the departing
///   owner drops from the entry, the disk is untouched;
/// - held by this owner alone — an orphan: [`Remove`](Action::Remove)
///   expecting the recorded hash when [`Clean`](PathState::Clean) or
///   [`Missing`](PathState::Missing) (apply drops an already-gone path from
///   the manifest alone), [`Refusal::Drift`] when
///   [`Drifted`](PathState::Drifted) unless `policy` lifts it to a
///   [`Remove`](Action::Remove) expecting the drifted hash;
/// - held only by other owners — not this plan's business: no action.
///
/// [`DriftPolicy::Overwrite`] lifts a drift refusal only where the drifted
/// node carries a hash for apply's changed-since-plan re-check — a file or
/// a symlink. A path whose kind drifted to a directory or to a node the
/// projection never writes stays refused under either policy: no
/// `expected_hash` could express what apply must re-verify.
///
/// An empty desired tree plans a removal: everything this owner alone holds
/// removes, everything it shares releases.
///
/// Kinds compare through the one hash convention ([`sha256_hex`]): a file
/// hashes its contents, a symlink its target string — so a desired symlink
/// whose target equals the on-disk link skips, and a changed target
/// overwrites or drifts, exactly like file bytes. Two seams remain:
///
/// - Symlink target *grading* — in-dest or external, `docs/security.lex`
///   section 3 — is not judged here yet; when it lands it joins the
///   per-path admission alongside the containment gateway (grading is
///   lexical, per link, resolved from the link's parent) and produces
///   [`Refusal::ExternalTarget`]. Until then no plan carries that refusal.
/// - [`Block`](EntryKind::Block) entries flow through the same generic
///   table, where their body hash matches no whole-node observation: a
///   desired block over nothing plans a [`Write`](Action::Write), a
///   pre-existing container refuses as foreign, and a recorded block never
///   classifies clean. Conservative until block-region classification and
///   container-tolerant writes land.
///
/// # Panics
///
/// Panics when two desired keys normalize to the same path (`b` and
/// `a/../b`): the tree claims one location twice and there is no
/// deterministic entry to prefer. [`load_mapping`](crate::load_mapping)
/// rejects such trees at parse time as
/// [`MappingDuplicate`](crate::Error::MappingDuplicate); a hand-built tree
/// carrying one is a caller bug, like a relative path handed to
/// [`Projection::new`](crate::Projection::new).
pub fn decide(
    owner: &str,
    desired: &BTreeMap<Utf8PathBuf, Entry>,
    manifest: &Manifest,
    observations: &Observations,
    state_prefix: Option<&Utf8Path>,
    policy: DriftPolicy,
) -> Plan {
    let states = classify(manifest, observations, state_prefix);
    let mut actions: BTreeMap<Utf8PathBuf, Action> = BTreeMap::new();

    // Admission: every desired key passes the containment gateway, and none
    // may enter the projection's own state subtree.
    let mut admitted: BTreeMap<Utf8PathBuf, &Entry> = BTreeMap::new();
    for (key, entry) in desired {
        let Ok(normalized) = contained_normalize(key) else {
            actions.insert(key.clone(), refuse(Refusal::Containment));
            continue;
        };
        if in_state(&normalized, state_prefix) {
            actions.insert(key.clone(), refuse(Refusal::Containment));
            continue;
        }
        assert!(
            admitted.insert(normalized.clone(), entry).is_none(),
            "two desired keys normalize to the same path {normalized}"
        );
    }

    for (path, entry) in &admitted {
        let action = desired_action(
            owner,
            entry,
            states.paths.get(path),
            manifest.entries.get(path),
            observations.paths.get(path),
            policy,
        );
        actions.insert(path.clone(), action);
    }

    // Recorded paths the desired tree no longer names.
    for (path, recorded) in &manifest.entries {
        if admitted.contains_key(path)
            || in_state(path, state_prefix)
            || !recorded.owners.contains(owner)
        {
            continue;
        }
        let action = if recorded.owners.len() > 1 {
            Action::Release
        } else {
            let state = states
                .paths
                .get(path)
                .expect("recorded paths outside the state subtree always classify");
            orphan_action(recorded, observations.paths.get(path), *state, policy)
        };
        actions.insert(path.clone(), action);
    }

    Plan {
        owner: owner.to_owned(),
        actions,
    }
}

/// Whether `path` lies in the projection's own state subtree — the prefix
/// itself included.
fn in_state(path: &Utf8Path, state_prefix: Option<&Utf8Path>) -> bool {
    state_prefix.is_some_and(|prefix| path.starts_with(prefix))
}

/// The action for a path the desired tree names, given its classification
/// (`None` when the path is neither recorded nor on disk).
fn desired_action(
    owner: &str,
    entry: &Entry,
    state: Option<&PathState>,
    recorded: Option<&ManifestEntry>,
    observation: Option<&Observation>,
    policy: DriftPolicy,
) -> Action {
    let Some(state) = state else {
        return Action::Write {
            entry: entry.clone(),
        };
    };
    if *state == PathState::Foreign {
        return refuse(Refusal::Foreign);
    }
    let recorded = recorded.expect("Clean, Drifted, and Missing paths are recorded");
    let others: BTreeSet<String> = recorded
        .owners
        .iter()
        .filter(|other| *other != owner)
        .cloned()
        .collect();
    if !others.is_empty() && !desired_matches_recorded(entry, recorded) {
        return refuse(Refusal::OwnerConflict { owners: others });
    }
    match state {
        PathState::Missing => Action::Write {
            entry: entry.clone(),
        },
        PathState::Clean => {
            let observation = observation.expect("a clean path was observed");
            if observation_matches_desired(entry, observation) {
                // Clean: the bytes on disk are the recorded bytes.
                Action::Skip {
                    expected_hash: recorded.hash.clone(),
                }
            } else {
                Action::Overwrite {
                    entry: entry.clone(),
                    expected_hash: recorded.hash.clone(),
                }
            }
        }
        PathState::Drifted => {
            let observation = observation.expect("a drifted path was observed");
            if observation_matches_desired(entry, observation) {
                // Edited into agreement: disk already equals desired.
                Action::Skip {
                    expected_hash: observed_hash(observation)
                        .expect("a node matching a desired entry carries a hash")
                        .to_owned(),
                }
            } else {
                lift_or_refuse_drift(observation, policy, |drifted_hash| Action::Overwrite {
                    entry: entry.clone(),
                    expected_hash: drifted_hash,
                })
            }
        }
        PathState::Foreign => unreachable!("handled above"),
    }
}

/// The action for an orphan: a path recorded under this owner alone and
/// absent from the desired tree.
fn orphan_action(
    recorded: &ManifestEntry,
    observation: Option<&Observation>,
    state: PathState,
    policy: DriftPolicy,
) -> Action {
    match state {
        // Missing removes too: apply drops an already-gone path from the
        // manifest alone (`Action::Remove`).
        PathState::Clean | PathState::Missing => Action::Remove {
            expected_hash: recorded.hash.clone(),
        },
        PathState::Drifted => {
            let observation = observation.expect("a drifted path was observed");
            lift_or_refuse_drift(observation, policy, |drifted_hash| Action::Remove {
                expected_hash: drifted_hash,
            })
        }
        PathState::Foreign => unreachable!("recorded paths are never foreign"),
    }
}

/// Resolves a drifted path under `policy`: refuse, or — when the policy
/// overwrites and the drifted node carries a hash for apply's
/// changed-since-plan re-check — the destructive action built by `lift`
/// expecting the drifted hash. A node without a hash (a directory, or a
/// kind the projection never writes) stays refused under either policy.
fn lift_or_refuse_drift(
    observation: &Observation,
    policy: DriftPolicy,
    lift: impl FnOnce(String) -> Action,
) -> Action {
    match policy {
        DriftPolicy::Refuse => refuse(Refusal::Drift),
        DriftPolicy::Overwrite => match observed_hash(observation) {
            Some(drifted_hash) => lift(drifted_hash.to_owned()),
            None => refuse(Refusal::Drift),
        },
    }
}

fn refuse(refusal: Refusal) -> Action {
    Action::Refuse { refusal }
}

/// Whether the observed node is exactly the recorded entry — kind and hash,
/// plus the executable bit for files. A recorded block matches nothing
/// until block-region classification lands (its hash covers the delimited
/// body, which no whole-node observation reproduces).
fn observation_matches_recorded(recorded: &ManifestEntry, observation: &Observation) -> bool {
    match (recorded.kind, observation) {
        (EntryKind::File, Observation::File { hash, executable }) => {
            *hash == recorded.hash && *executable == recorded.executable
        }
        (EntryKind::Symlink, Observation::Symlink { hash, .. }) => *hash == recorded.hash,
        _ => false,
    }
}

/// Whether the observed node is exactly the desired entry — same comparison
/// as [`observation_matches_recorded`], against the desired side.
fn observation_matches_desired(entry: &Entry, observation: &Observation) -> bool {
    match (entry, observation) {
        (
            Entry::File {
                contents,
                executable,
            },
            Observation::File {
                hash,
                executable: on_disk,
            },
        ) => executable == on_disk && *hash == sha256_hex(contents),
        (Entry::Symlink { target }, Observation::Symlink { hash, .. }) => {
            *hash == sha256_hex(target.as_bytes())
        }
        _ => false,
    }
}

/// Whether the desired entry is exactly the recorded one — kind, hash, and
/// executable bit — the agreement two owners must reach to share a path.
fn desired_matches_recorded(entry: &Entry, recorded: &ManifestEntry) -> bool {
    entry.kind() == recorded.kind
        && desired_executable(entry) == recorded.executable
        && desired_hash(entry) == recorded.hash
}

/// The hash the manifest would record for a desired entry: contents for a
/// file, the target string for a symlink, the body for a block.
fn desired_hash(entry: &Entry) -> String {
    match entry {
        Entry::File { contents, .. } => sha256_hex(contents),
        Entry::Symlink { target } => sha256_hex(target.as_bytes()),
        Entry::Block { body } => sha256_hex(body),
    }
}

/// The executable bit the manifest would record: the file's own; always
/// `false` for symlinks and blocks ([`ManifestEntry::executable`]).
fn desired_executable(entry: &Entry) -> bool {
    match entry {
        Entry::File { executable, .. } => *executable,
        Entry::Symlink { .. } | Entry::Block { .. } => false,
    }
}

/// The observed node's hash, where it has one: files and symlinks. `None`
/// for absent paths, directories, and nodes the projection never writes —
/// nothing apply could re-check before a destructive action.
fn observed_hash(observation: &Observation) -> Option<&str> {
    match observation {
        Observation::File { hash, .. } | Observation::Symlink { hash, .. } => Some(hash),
        Observation::Absent | Observation::Directory | Observation::Other => None,
    }
}

#[cfg(test)]
#[path = "decide_tests.rs"]
mod tests;
