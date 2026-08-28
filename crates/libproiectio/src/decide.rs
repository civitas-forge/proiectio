use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;

use camino::{Utf8Path, Utf8PathBuf};

use crate::containment::{Hop, contained_normalize, contained_target_chain, is_pathname};
use crate::{
    Action, DriftPolicy, Entry, EntryKind, ExternalTargetPolicy, Manifest, ManifestEntry,
    NodeSignature, Observation, Observations, PathState, Plan, PlanOptions, Refusal, Status,
    sha256_hex,
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
/// Observation is lstat-only and never descends a symlink, so a path
/// *beneath* a link on disk is observed [`Absent`](Observation::Absent) —
/// which is exactly why [`decide`] refuses to plan one (its no-alias rule):
/// nothing here could tell such a path's real node from a missing one.
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
/// on-disk location has one action — and a tree claiming one location
/// twice (two keys normalizing to the same path, or one desired path lying
/// beneath another — a nesting no file or block permits, and none the
/// no-alias rule below permits beneath a symlink either) has both claims
/// refused as [`Refusal::TreeConflict`], keyed verbatim, each naming the
/// keys it collides with. [`load_mapping`](crate::load_mapping) rejects
/// same-path duplicates at parse time as
/// [`MappingDuplicate`](crate::Error::MappingDuplicate); the refusal here
/// is the deciding stage's own verdict on any tree, however built.
///
/// # Symlinks
///
/// Two rules, both judged per admitted path.
///
/// *Target grading* (`docs/security.lex` section 3). Every desired
/// symlink's target is resolved once, here, from the link's parent
/// directory and purely to classify it: a target landing inside the
/// destination is in-dest and always allowed — whether or not anything
/// exists there, since a dangling pointer is a legal link — while one
/// landing outside is external and refused as [`Refusal::ExternalTarget`]
/// carrying the target string, unless `options` sets
/// [`ExternalTargetPolicy::Allow`]. Grading never rewrites anything: what
/// apply writes is the target string verbatim, permitted or not. Only
/// *desired* links are graded — removing a recorded external link unlinks
/// the pointer and reads nothing through it.
///
/// Resolution follows the destination's own links, out of `observations`:
/// a target lands outside when it is absolute, climbs out, carries one of
/// the spellings graded external on every host, or reaches outside through
/// a link the destination already holds — where dest holds
/// `pivot -> /etc`, a tree projecting `evil -> pivot/passwd` needs the
/// permission, while an ordinary in-dest chain (`shared -> real` with
/// `rc -> shared/rc`) does not. [`contained_target_chain`] carries the
/// whole rule, cycle guard included. Two consequences worth naming: a
/// target's verdict depends on the destination, so the same tree may need
/// the permission in one destination and not in another (tree *paths* keep
/// the host-independent lexical verdict [`contained_join`](crate::contained_join)
/// gives them); and the chain runs over what the destination holds at plan
/// time, not over what this plan writes — the projection still cannot
/// create an escaping hop of its own, since a desired link is itself
/// graded before it may be written. Apply re-grades against the disk
/// before publishing a link, so a pivot swapped after the plan refuses
/// rather than publishing an escaping pointer.
///
/// One question comes before grading: a target that is not a pathname on
/// any host — the empty string, or one carrying a NUL byte — lands nowhere
/// to grade, and is refused as [`Refusal::InvalidTarget`] under either
/// policy, since the permission is about where a pointer points and there
/// is no pointer.
///
/// *No aliases.* A plan's key is the location on disk: the projection
/// never writes a path that resolves through a symlink, so what the
/// manifest records at a path is what a later run observes there.
/// Refused as [`Refusal::Containment`], therefore, is any admitted path
/// with a symlink ancestor that outlives this plan — a link on disk, owned
/// or foreign, that no action in this plan removes; a desired path nesting
/// beneath a *desired* link is the [`Refusal::TreeConflict`] above. The
/// rule is what keeps the three stages agreeing: observation never
/// descends a link, so a path beneath one observes
/// [`Absent`](Observation::Absent) forever, and a projection that wrote
/// through the link would re-plan the write on every run and then refuse
/// its own file as changed. Apply's walk still follows an owned in-dest
/// link, but this stage cannot aim a removal through one for the same
/// reason: a path recorded beneath a link classifies
/// [`Missing`](PathState::Missing), so its [`Remove`](Action::Remove)
/// expects nothing, and apply refuses as drift on finding a node there.
/// Under this rule the projection writes no such path, so the shape
/// survives only in a manifest predating it; removing the link first
/// clears it.
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
///   [`Overwrite`](Action::Overwrite) expecting the recorded signature;
/// - [`Drifted`](PathState::Drifted) with desired differing —
///   [`Refusal::Drift`], unless `options.drift` is
///   [`DriftPolicy::Overwrite`], which plans an
///   [`Overwrite`](Action::Overwrite) expecting the *drifted* node's
///   signature observed at plan time;
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
///   expecting the recorded signature when [`Clean`](PathState::Clean),
///   expecting nothing when [`Missing`](PathState::Missing) — the path was
///   already gone, so apply drops the manifest entry alone and refuses if
///   a node has appeared since the plan — and [`Refusal::Drift`] when
///   [`Drifted`](PathState::Drifted) unless `options.drift` lifts it to a
///   [`Remove`](Action::Remove) expecting the drifted node's signature;
/// - held only by other owners — not this plan's business: no action.
///
/// [`DriftPolicy::Overwrite`] lifts a drift refusal only where the drifted
/// node carries a [`NodeSignature`] for apply's changed-since-plan
/// re-check — a file or a symlink. A path whose kind drifted to a
/// directory or to a node the projection never writes stays refused under
/// either policy: no signature could express what apply must re-verify.
///
/// An empty desired tree plans a removal: everything this owner alone holds
/// removes, everything it shares releases.
///
/// Kinds compare through the one hash convention ([`sha256_hex`]): a file
/// hashes its contents, a symlink its target string — so a desired symlink
/// whose target equals the on-disk link skips, a changed target overwrites
/// or drifts, and a file replacing a link (or a link replacing a file)
/// rides the same rules as changed bytes, exactly like file content. One
/// seam remains: [`Block`](EntryKind::Block) entries flow through the same
/// generic table, where their body hash matches no whole-node observation
/// — a desired block over nothing plans a [`Write`](Action::Write), a
/// pre-existing container refuses as foreign, a recorded block never
/// classifies clean, and a recorded block's drift is never lifted, since no
/// whole-node signature can express the region a lifted removal would
/// strip. Conservative until block-region classification and
/// container-tolerant writes land.
pub fn decide(
    owner: &str,
    desired: &BTreeMap<Utf8PathBuf, Entry>,
    manifest: &Manifest,
    observations: &Observations,
    state_prefix: Option<&Utf8Path>,
    options: PlanOptions,
) -> Plan {
    let states = classify(manifest, observations, state_prefix);
    let mut actions: BTreeMap<Utf8PathBuf, Action> = BTreeMap::new();

    // Admission: every desired key passes the containment gateway, none
    // may enter the projection's own state subtree, and no two admitted
    // keys may claim overlapping on-disk locations.
    let mut claims: BTreeMap<Utf8PathBuf, BTreeMap<&Utf8PathBuf, &Entry>> = BTreeMap::new();
    for (key, entry) in desired {
        let Ok(normalized) = contained_normalize(key) else {
            actions.insert(key.clone(), refuse(Refusal::Containment));
            continue;
        };
        if in_state(&normalized, state_prefix) {
            actions.insert(key.clone(), refuse(Refusal::Containment));
            continue;
        }
        claims.entry(normalized).or_default().insert(key, entry);
    }

    // Overlaps between distinct claimed locations: one normalized path
    // lying beneath another. Each path checks its proper ancestors, so
    // every overlapping pair is recorded once, on both sides.
    let mut overlaps: BTreeMap<Utf8PathBuf, BTreeSet<Utf8PathBuf>> = BTreeMap::new();
    for (normalized, keys) in &claims {
        for ancestor in normalized.ancestors().skip(1) {
            let Some(above) = claims.get(ancestor) else {
                continue;
            };
            overlaps
                .entry(normalized.clone())
                .or_default()
                .extend(above.keys().map(|&key| key.clone()));
            overlaps
                .entry(ancestor.to_owned())
                .or_default()
                .extend(keys.keys().map(|&key| key.clone()));
        }
    }

    let mut admitted: BTreeMap<Utf8PathBuf, &Entry> = BTreeMap::new();
    for (normalized, keys) in &claims {
        let overlapping = overlaps.get(normalized);
        if keys.len() == 1 && overlapping.is_none() {
            let (_, &entry) = keys.first_key_value().expect("one claim");
            admitted.insert(normalized.clone(), entry);
            continue;
        }
        // The location is claimed twice — by two keys normalizing to it,
        // by a key above it, or by one below it. Refuse every claimant,
        // each naming the keys it collides with.
        for &key in keys.keys() {
            let mut paths: BTreeSet<Utf8PathBuf> = keys
                .keys()
                .filter(|&&other| other != key)
                .map(|&other| other.clone())
                .collect();
            paths.extend(overlapping.iter().flat_map(|set| set.iter().cloned()));
            actions.insert(key.clone(), refuse(Refusal::TreeConflict { paths }));
        }
    }

    // Recorded paths the desired tree no longer names. Named means
    // *claimed*, not admitted: a recorded location under a tree-conflict
    // refusal is still named by the desired tree, and planning its removal
    // would overwrite the refusal.
    //
    // Judged before the admitted paths because a removal *vacates* its
    // path: a link this plan unlinks is no longer an ancestor the writes
    // below would resolve through, and act runs removals first.
    let mut vacated: BTreeSet<Utf8PathBuf> = BTreeSet::new();
    for (path, recorded) in &manifest.entries {
        if claims.contains_key(path)
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
            orphan_action(
                recorded,
                observations.paths.get(path),
                *state,
                options.drift,
            )
        };
        if matches!(action, Action::Remove { .. }) {
            vacated.insert(path.clone());
        }
        actions.insert(path.clone(), action);
    }

    for (path, entry) in &admitted {
        let action = match link_refusal(path, entry, observations, &vacated, options) {
            Some(refusal) => refuse(refusal),
            None => desired_action(
                owner,
                entry,
                states.paths.get(path),
                manifest.entries.get(path),
                observations.paths.get(path),
                options.drift,
            ),
        };
        actions.insert(path.clone(), action);
    }

    Plan {
        owner: owner.to_owned(),
        external_targets: options.external_targets,
        actions,
    }
}

/// Whether `path` lies in the projection's own state subtree — the prefix
/// itself included.
fn in_state(path: &Utf8Path, state_prefix: Option<&Utf8Path>) -> bool {
    state_prefix.is_some_and(|prefix| path.starts_with(prefix))
}

/// The two symlink rules of [`decide`]'s rustdoc, judged over an admitted
/// path before its classification is consulted; `None` admits the path to
/// the ordinary action table.
///
/// - the path resolves through a link that outlives this plan —
///   [`Refusal::Containment`], the no-alias rule;
/// - the entry is a symlink whose target is not a pathname on any host —
///   [`Refusal::InvalidTarget`] carrying the target verbatim, under either
///   policy;
/// - the entry is a symlink whose target grades external and `options`
///   does not permit external targets — [`Refusal::ExternalTarget`]
///   carrying the target verbatim.
///
/// In that order. Containment first, because where a path would resolve is
/// not a question its own target answers; then the pathname check, because
/// a string naming no path lands nowhere for grading to judge.
fn link_refusal(
    path: &Utf8Path,
    entry: &Entry,
    observations: &Observations,
    vacated: &BTreeSet<Utf8PathBuf>,
    options: PlanOptions,
) -> Option<Refusal> {
    if resolves_through_link(path, observations, vacated) {
        return Some(Refusal::Containment);
    }
    let Entry::Symlink { target } = entry else {
        return None;
    };
    if !is_pathname(target) {
        return Some(Refusal::InvalidTarget {
            target: target.clone(),
        });
    }
    if options.external_targets == ExternalTargetPolicy::Refuse
        && !target_resolves_in_dest(path, target, observations, vacated)
    {
        return Some(Refusal::ExternalTarget {
            target: target.clone(),
        });
    }
    None
}

/// Whether some ancestor of `path` is observed as a symlink that no action
/// in this plan removes — `vacated` naming the paths the run unlinks
/// before it writes anything.
///
/// A link the plan removes is not an ancestor the write will meet: act
/// executes every removal first, children before parents, and only then
/// creates what the writes need.
fn resolves_through_link(
    path: &Utf8Path,
    observations: &Observations,
    vacated: &BTreeSet<Utf8PathBuf>,
) -> bool {
    path.ancestors().skip(1).any(|ancestor| {
        !ancestor.as_str().is_empty()
            && matches!(
                observations.paths.get(ancestor),
                Some(Observation::Symlink { .. })
            )
            && !vacated.contains(ancestor)
    })
}

/// Grades a desired symlink's target (`docs/security.lex` section 3):
/// `true` where the target, resolved from the link's parent directory
/// through the destination's own links, lands inside the destination.
///
/// [`contained_target_chain`] is the rule; this supplies the destination it
/// resolves against, out of the plan-time observations, so the stage stays
/// filesystem-free. `vacated` names the paths this plan unlinks before it
/// writes anything: a link the run removes is not a hop the pointer will
/// resolve through, the same reading [`resolves_through_link`] gives an
/// ancestor the plan removes.
fn target_resolves_in_dest(
    link: &Utf8Path,
    target: &str,
    observations: &Observations,
    vacated: &BTreeSet<Utf8PathBuf>,
) -> bool {
    let parent = link.parent().unwrap_or_else(|| Utf8Path::new(""));
    let landing = contained_target_chain(parent, target, |path| {
        Ok::<Hop, Infallible>(observed_hop(observations, path, vacated))
    });
    match landing {
        Ok(landing) => landing.is_some(),
        Err(never) => match never {},
    }
}

/// What the observation snapshot says stands at one destination-relative
/// path, in the terms chain resolution asks in.
///
/// Only a symlink continues a chain: an absent path, a file, a directory,
/// and a kind the projection never writes all end it, and so does a path
/// the snapshot never mentions. A link this plan removes ends it too — by
/// the time apply publishes anything, the removal has already run. A link
/// whose on-disk target is not UTF-8 is [`Unresolvable`](Hop::Unresolvable):
/// nothing can say where it points, so nothing can vouch for a chain
/// through it (apply's walk refuses to follow such a link for the same
/// reason).
fn observed_hop(
    observations: &Observations,
    path: &Utf8Path,
    vacated: &BTreeSet<Utf8PathBuf>,
) -> Hop {
    if vacated.contains(path) {
        return Hop::Terminal;
    }
    match observations.paths.get(path) {
        Some(Observation::Symlink {
            target: Some(target),
            ..
        }) => Hop::Link(target.clone()),
        Some(Observation::Symlink { target: None, .. }) => Hop::Unresolvable,
        Some(
            Observation::Absent
            | Observation::File { .. }
            | Observation::Directory
            | Observation::Other,
        )
        | None => Hop::Terminal,
    }
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
                skip(entry)
            } else {
                Action::Overwrite {
                    entry: entry.clone(),
                    expected: recorded_signature(recorded),
                }
            }
        }
        PathState::Drifted => {
            let observation = observation.expect("a drifted path was observed");
            if observation_matches_desired(entry, observation) {
                // Edited into agreement: disk already equals desired.
                skip(entry)
            } else {
                lift_or_refuse_drift(recorded, observation, policy, |drifted| Action::Overwrite {
                    entry: entry.clone(),
                    expected: drifted,
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
        PathState::Clean => Action::Remove {
            expected: Some(recorded_signature(recorded)),
        },
        // Missing removes too, expecting nothing: the plan records the
        // absence, so apply drops the manifest entry alone — and a node
        // appearing at the path since the plan is a change apply refuses,
        // exactly like a present node changing.
        PathState::Missing => Action::Remove { expected: None },
        PathState::Drifted => {
            let observation = observation.expect("a drifted path was observed");
            lift_or_refuse_drift(recorded, observation, policy, |drifted| Action::Remove {
                expected: Some(drifted),
            })
        }
        PathState::Foreign => unreachable!("recorded paths are never foreign"),
    }
}

/// Resolves a drifted path under `policy`: refuse, or — when the policy
/// overwrites and the drifted node carries a signature for apply's
/// changed-since-plan re-check — the destructive action built by `lift`
/// expecting the drifted node. Refused under either policy:
///
/// - a node without a signature — a directory, or a kind the projection
///   never writes;
/// - a recorded [`Block`](EntryKind::Block): it owns only its delimited
///   region, which no whole-node signature expresses — a lifted removal
///   would read as whole-file deletion of a container the projection does
///   not own whole. Conservative until block-region classification lands
///   (the seam in [`decide`]'s rustdoc).
fn lift_or_refuse_drift(
    recorded: &ManifestEntry,
    observation: &Observation,
    policy: DriftPolicy,
    lift: impl FnOnce(NodeSignature) -> Action,
) -> Action {
    if recorded.kind == EntryKind::Block {
        return refuse(Refusal::Drift);
    }
    match policy {
        DriftPolicy::Refuse => refuse(Refusal::Drift),
        DriftPolicy::Overwrite => match observed_signature(observation) {
            Some(drifted) => lift(drifted),
            None => refuse(Refusal::Drift),
        },
    }
}

fn refuse(refusal: Refusal) -> Action {
    Action::Refuse { refusal }
}

/// The skip for a path whose disk node already equals the desired `entry`:
/// the action carries the full desired signature because apply re-checks
/// it against the disk and records it in the manifest, where the recorded
/// entry may differ in any field (a drifted path edited into agreement, or
/// an owner joining a shared path).
fn skip(entry: &Entry) -> Action {
    Action::Skip {
        expected: desired_signature(entry),
    }
}

/// The signature the manifest records for `recorded` — what the disk must
/// still hold for a clean path's destructive action to proceed at apply
/// time.
fn recorded_signature(recorded: &ManifestEntry) -> NodeSignature {
    NodeSignature {
        kind: recorded.kind,
        hash: recorded.hash.clone(),
        executable: recorded.executable,
    }
}

/// The signature the manifest would record for a desired entry.
fn desired_signature(entry: &Entry) -> NodeSignature {
    NodeSignature {
        kind: entry.kind(),
        hash: desired_hash(entry),
        executable: desired_executable(entry),
    }
}

/// The observed node's signature, where it has one: files and symlinks.
/// `None` for absent paths, directories, and nodes the projection never
/// writes — nothing apply could re-check before a destructive action.
fn observed_signature(observation: &Observation) -> Option<NodeSignature> {
    match observation {
        Observation::File { hash, executable } => Some(NodeSignature {
            kind: EntryKind::File,
            hash: hash.clone(),
            executable: *executable,
        }),
        Observation::Symlink { hash, .. } => Some(NodeSignature {
            kind: EntryKind::Symlink,
            hash: hash.clone(),
            executable: false,
        }),
        Observation::Absent | Observation::Directory | Observation::Other => None,
    }
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

#[cfg(test)]
#[path = "decide_tests.rs"]
mod tests;
