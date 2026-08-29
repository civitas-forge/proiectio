use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;

use camino::{Utf8Path, Utf8PathBuf};

use crate::block;
use crate::containment::{Hop, contained_normalize, contained_target_chain, is_pathname};
use crate::{
    Action, BlockFault, DriftPolicy, Entry, EntryKind, ExternalTargetPolicy, Manifest,
    ManifestEntry, NodeSignature, Observation, Observations, Origin, PathState, Placement, Plan,
    PlanOptions, Refusal, Status, sha256_hex,
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
/// state and never classifies (`in_state`, whose rustdoc says why this reading is
/// narrower than the one admission applies).
///
/// A recorded [`Block`](EntryKind::Block) classifies over its *region*, not
/// its container: [`observe`](crate::observe) locates the region with the
/// recorded marker and placement and hashes the body alone, so a container
/// edited outside the region reads [`Clean`](PathState::Clean), and a
/// container whose marker line is gone reads [`Missing`](PathState::Missing)
/// exactly as a deleted file does (`docs/design.lex` section 2). One holding
/// the marker on more than one whole line identifies no region at all and
/// reads [`Drifted`](PathState::Drifted) whatever its extreme occurrence
/// holds, which is what stops every later stage acting on a guess
/// ([`EntryKind::Block`]).
pub(crate) fn classify(
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
            // A region whose marker line is no longer in the container is a
            // node gone from disk, which is what Missing means — but only
            // where the record is the block that region belongs to. This
            // stage takes the manifest and the observations as two separate
            // inputs, so a region observed at a path recorded as a whole node
            // is the kind mismatch below, not an absence.
            (Some(recorded), Observation::Block { hash: None, .. }) if recorded.kind.is_block() => {
                PathState::Missing
            }
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
/// gateway refuses, or one overlapping the state subtree named by
/// `state_prefix` (`overlaps_state`), gets [`Refusal::Containment`] keyed
/// by the key verbatim. Admitted keys
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
/// symlink's target is classified here, from the link's parent directory
/// and purely to decide whether the plan may carry it — apply re-grades the
/// same target against the live disk before publishing the link, since the
/// destination this verdict was taken against can move underneath it. A
/// target landing inside the
/// destination is in-dest and always allowed — whether or not anything
/// exists there, since a dangling pointer is a legal link — while one
/// landing outside is external and refused as [`Refusal::ExternalTarget`]
/// carrying the target string, unless `options` sets
/// [`ExternalTargetPolicy::Allow`]. Grading never rewrites anything: what
/// apply writes is the target string verbatim, permitted or not. Only
/// *desired* links are graded — removing a recorded external link unlinks
/// the pointer and reads nothing through it.
///
/// Resolution follows the destination's own links: a target lands outside
/// when it is absolute, climbs out, carries one of the spellings graded
/// external on every host, or reaches outside through a link the
/// destination holds — where dest holds `pivot -> /etc`, a tree projecting
/// `evil -> pivot/passwd` needs the permission, while an ordinary in-dest
/// chain (`shared -> real` with `rc -> shared/rc`) does not.
/// `contained_target_chain` carries the whole rule, cycle guard included,
/// and `planned_hop` supplies the destination it resolves against: the
/// links this run *leaves* — the desired tree's own, then `observations`
/// for every path the run does not touch. A pointer graded against the
/// destination the run leaves is graded against the destination it will
/// live in, which is what keeps the projection from planting an escaping
/// pointer out of two links that each land in-dest alone, and what makes
/// the verdict the same on the run that writes a link and every run after
/// it. One consequence worth naming: a target's verdict depends on the
/// destination, so the same tree may need the permission in one
/// destination and not in another (tree *paths* keep the host-independent
/// lexical verdict [`contained_join`](crate::contained_join) gives them).
/// Apply re-grades before publishing a link, so a pivot swapped after the
/// plan refuses rather than publishing an escaping pointer.
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
/// - overlapping the state subtree (`overlaps_state`) —
///   [`Refusal::Containment`];
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
/// re-check — a file, a symlink, or a region the recorded marker still
/// identifies. A path whose kind drifted to a directory or to a node the
/// projection never writes stays refused under either policy, as does a
/// container holding the marker on more than one whole line: no signature
/// could express what apply must re-verify.
///
/// An empty desired tree plans a removal: everything this owner alone holds
/// removes, everything it shares releases. [`decide_removal`] is that call
/// by name, and takes the path subset an empty tree cannot express.
///
/// Kinds compare through the one hash convention ([`sha256_hex`]): a file
/// hashes its contents, a symlink its target string, a block its region's
/// body — so a desired symlink whose target equals the on-disk link skips, a
/// changed target overwrites or drifts, and a file replacing a link (or a
/// link replacing a file) rides the same rules as changed bytes, exactly like
/// file content.
///
/// # Blocks
///
/// [`Block`](EntryKind::Block) entries ride that same table, over the region
/// rather than the container (`docs/design.lex` section 2). Three places are
/// where the table alone does not settle it, and each follows from a rule
/// [`EntryKind::Block`] states:
///
/// - a desired block over an unrecorded regular file plans a
///   [`Write`](Action::Write) rather than refusing as foreign: writing into a
///   file it does not own whole is what a block is for, and only apply's read
///   can tell an untouched container from one already carrying a region. An
///   unrecorded container that is *not* a regular file still refuses as
///   [`Refusal::Foreign`], a symlink included;
/// - a container that is not there refuses as [`Refusal::Block`] carrying
///   [`ContainerMissing`](crate::BlockFault::ContainerMissing) — a block
///   never creates one, so nothing here heals a deleted container the way a
///   write heals a deleted file;
/// - a path recorded as a block and desired as a whole node, or the other
///   way round, refuses as [`Refusal::Block`] carrying
///   [`KindChange`](crate::BlockFault::KindChange).
///
/// The rest is the ordinary table. A desired marker or placement differing
/// from the recorded pair makes the desired kind differ, so it plans an
/// [`Overwrite`](Action::Overwrite) expecting the recorded region — apply
/// strips that region and splices the new one in a single publish. A drifted
/// region lifts under [`DriftPolicy::Overwrite`] like any other node whose
/// signature apply can re-verify, which for a block is the body the recorded
/// marker locates: a container that became a directory carries no such
/// signature and stays refused under either policy. Neither does a container
/// holding the marker on more than one whole line — it identifies no region,
/// so it never reads clean, never skips, and never lifts, until the extra
/// line is gone ([`EntryKind::Block`]).
///
/// `origin` says where `desired` came from and lands on the [`Plan`], so
/// every refusal this call produces — and every refusal applying the plan
/// raises — names it ([`Origin`]).
pub(crate) fn decide(
    owner: &str,
    desired: &BTreeMap<Utf8PathBuf, Entry>,
    origin: Origin,
    manifest: &Manifest,
    observations: &Observations,
    state_prefix: Option<&Utf8Path>,
    options: PlanOptions,
) -> Plan {
    Plan {
        owner: owner.to_owned(),
        origin,
        external_targets: options.external_targets,
        actions: plan_actions(
            owner,
            desired,
            manifest,
            observations,
            state_prefix,
            options,
            &Judged::Everything,
        ),
    }
}

/// The removal stage: the plan that clears what `owner` holds, either
/// whole or at the paths named — [`decide`] against an empty desired tree,
/// with the recorded paths it judges narrowed by `scope`
/// (`docs/design.lex` section 2).
///
/// - [`RemovalScope::Everything`] — every path recorded under `owner` is
///   judged. This is exactly `decide(owner, &BTreeMap::new(), ..)`, which is
///   the definition of a removal, spelled as its own call so a caller need
///   not build an empty tree to say it.
/// - [`RemovalScope::Paths`] — only the recorded paths the set names are
///   judged; every other recorded path keeps its entry and its node, absent
///   from the plan entirely. The set names *locations*, one recorded path
///   each: the manifest records no directories, so naming one names nothing,
///   and a subtree is spelled by naming its paths.
///
/// Each requested path is admitted through the same containment gateway
/// every desired key passes ([`contained_join`](crate::contained_join)'s
/// lexical contract): a path the gateway refuses, or one overlapping the
/// state subtree (`overlaps_state`), gets [`Refusal::Containment`] keyed
/// by the request verbatim and is judged no further. Admitted requests are matched against the manifest lexically
/// normalized, so `a/../b` names the recorded `b`.
///
/// A requested path the manifest does not record under `owner` yields no
/// action: there is nothing of this owner's at that location to remove, and
/// a removal must stay repeatable — re-running one that succeeded names
/// paths that are already gone. That is also why nothing here refuses a
/// path as foreign: the projection does not adopt what it never wrote, and
/// a location it does not own is not a location it declines to remove.
///
/// The refusals are every other plan's, produced by the same code paths:
/// a recorded path overlapping the state subtree refuses as
/// [`Refusal::Containment`] (`overlaps_state`); a recorded path drifted on
/// disk refuses as [`Refusal::Drift`] carrying
/// it, unless `options.drift` is [`DriftPolicy::Overwrite`]; a path other
/// owners hold too plans a [`Release`](Action::Release) and leaves the disk
/// alone; a path already gone plans a [`Remove`](Action::Remove) expecting
/// nothing, so the manifest entry drops and a node that appeared since the
/// plan refuses at apply time. [`apply`](crate::apply) then removes in
/// reverse order and prunes the directories the removals emptied, keeping
/// any that still holds anything.
///
/// The plan carries [`Origin::Caller`]: a removal is decided from the
/// manifest, so there is no source tree for a refusal to name.
pub(crate) fn decide_removal(
    owner: &str,
    scope: RemovalScope<'_>,
    manifest: &Manifest,
    observations: &Observations,
    state_prefix: Option<&Utf8Path>,
    options: PlanOptions,
) -> Plan {
    let (judged, refused) = match scope {
        RemovalScope::Everything => (Judged::Everything, BTreeMap::new()),
        RemovalScope::Paths(requested) => {
            let mut admitted = BTreeSet::new();
            let mut refused: BTreeMap<Utf8PathBuf, Action> = BTreeMap::new();
            for request in requested {
                match contained_normalize(request) {
                    Some(normalized) if !overlaps_state(&normalized, state_prefix) => {
                        admitted.insert(normalized);
                    }
                    _ => {
                        refused.insert(request.clone(), refuse(Refusal::Containment));
                    }
                }
            }
            (Judged::Paths(admitted), refused)
        }
    };
    let mut actions = plan_actions(
        owner,
        &BTreeMap::new(),
        manifest,
        observations,
        state_prefix,
        options,
        &judged,
    );
    // A refused request was never admitted, so its key names no planned
    // action: the two maps are disjoint.
    actions.extend(refused);
    Plan {
        owner: owner.to_owned(),
        origin: Origin::Caller,
        external_targets: options.external_targets,
        actions,
    }
}

/// What a removal clears: everything the owner holds, or the paths a caller
/// names ([`Projection::plan_removal`](crate::Projection::plan_removal)).
///
/// The two are separate spellings rather than a full list and an empty one,
/// so clearing an owner cannot be said by accident: `Paths` over an empty
/// set names no location and plans nothing, and only `Everything` reaches
/// every recorded path. A caller passing a path list it collected — a
/// command line's arguments, say — therefore removes nothing when the list
/// comes up empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalScope<'a> {
    /// Every path recorded under the owner.
    Everything,
    /// The recorded paths this set names, and no others. Each passes the
    /// containment gateway first, and matches the manifest lexically
    /// normalized.
    Paths(&'a BTreeSet<Utf8PathBuf>),
}

/// Which of the owner's recorded paths a plan judges: everything it holds,
/// or the admitted subset a [`RemovalScope::Paths`] request named.
enum Judged {
    /// Every path recorded under the owner — what planning against a
    /// desired tree always judges, since any recorded path the tree no
    /// longer names is an orphan.
    Everything,
    /// The recorded paths in this set alone; the rest keep their entries
    /// and their nodes. The paths are normalized, matching the manifest's
    /// own keys.
    Paths(BTreeSet<Utf8PathBuf>),
}

impl Judged {
    /// Whether a recorded path is this plan's business.
    fn covers(&self, path: &Utf8Path) -> bool {
        match self {
            Judged::Everything => true,
            Judged::Paths(paths) => paths.contains(path),
        }
    }
}

/// The body behind [`decide`] and [`decide_removal`]: the action table of
/// [`decide`]'s rustdoc, with `judged` narrowing which of the owner's
/// recorded paths it judges. The callers wrap the actions in the [`Plan`]
/// that carries the owner, the origin, and the policy.
fn plan_actions(
    owner: &str,
    desired: &BTreeMap<Utf8PathBuf, Entry>,
    manifest: &Manifest,
    observations: &Observations,
    state_prefix: Option<&Utf8Path>,
    options: PlanOptions,
    judged: &Judged,
) -> BTreeMap<Utf8PathBuf, Action> {
    let states = classify(manifest, observations, state_prefix);
    let mut actions: BTreeMap<Utf8PathBuf, Action> = BTreeMap::new();

    // Admission: every desired key passes the containment gateway, none
    // may claim a location overlapping the projection's own state subtree,
    // and no two admitted keys may claim overlapping on-disk locations.
    //
    // `named` is every location the desired tree names, refused or not — the
    // orphan loop below reads it rather than `claims`, because a location the
    // tree names is not an orphan whatever verdict admission gave it.
    let mut claims: BTreeMap<Utf8PathBuf, BTreeMap<&Utf8PathBuf, &Entry>> = BTreeMap::new();
    let mut named: BTreeSet<Utf8PathBuf> = BTreeSet::new();
    for (key, entry) in desired {
        let Some(normalized) = contained_normalize(key) else {
            // No location to name: the gateway refused the key outright, and
            // a manifest key is always one the gateway admits.
            actions.insert(key.clone(), refuse(Refusal::Containment));
            continue;
        };
        named.insert(normalized.clone());
        if overlaps_state(&normalized, state_prefix) {
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

    // Recorded paths the desired tree no longer names. Named means the
    // location appeared in the tree, not that admission took it: a recorded
    // location refused as a tree conflict or for overlapping the state
    // subtree is still one the tree names, and planning its removal would
    // overwrite the refusal — with a `Remove` that deletes the very file the
    // tree asked for, since a plan carrying no refusal applies in full.
    //
    // Judged before the admitted paths because a removal *vacates* its
    // path: a link this plan unlinks is no longer an ancestor the writes
    // below would resolve through, and act runs removals first.
    let mut vacated: BTreeSet<Utf8PathBuf> = BTreeSet::new();
    for (path, recorded) in &manifest.entries {
        if named.contains(path)
            || in_state(path, state_prefix)
            || !recorded.owners.contains(owner)
            || !judged.covers(path)
        {
            continue;
        }
        // A recorded location the state directory sits beneath — `in_state`
        // above has already taken the ones inside it. Classified, so status
        // still reports it, but never acted on: removing it is removing the
        // node the state directory hangs from, and unlinking a recorded
        // `.local` the manifest lives under leaves the next run reading no
        // manifest and calling every projected file foreign. It refuses as
        // the same Containment a request naming it gets, so the verdict does
        // not turn on how the removal was scoped.
        if overlaps_state(path, state_prefix) {
            actions.insert(path.clone(), refuse(Refusal::Containment));
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
        let action = match link_refusal(path, entry, &admitted, observations, &vacated, options) {
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

    actions
}

/// Whether acting at `path` would touch the projection's own state subtree —
/// the question every location a plan would write or remove is admitted
/// through. Symmetric: the two overlap when either is a prefix of the other,
/// because both directions are the same collision. `.proiectio/manifest.json`
/// sits inside the state directory; `.local`, where the state directory is
/// `.local/state/proiectio`, is a location it sits beneath, and writing a
/// file there would stand where that directory stands while removing it would
/// unlink the node the manifest hangs from.
///
/// Refusing only the inside case would let the other reach apply and refuse
/// there under a different rule, against a plan a dry run had already reported
/// as what apply would execute (`docs/implementation.lex` section 1). All
/// callers refuse as [`Refusal::Containment`], keyed by whatever named the
/// path.
fn overlaps_state(path: &Utf8Path, state_prefix: Option<&Utf8Path>) -> bool {
    state_prefix.is_some_and(|prefix| path.starts_with(prefix) || prefix.starts_with(path))
}

/// Whether `path` is itself in the projection's own state subtree, the prefix
/// included — the exclusion [`classify`] applies, and deliberately not
/// `overlaps_state`.
///
/// Asymmetric on purpose: a path the state directory sits *beneath* is not
/// the projection's state. Where the state directory is
/// `.local/state/proiectio`, `.local` holds whatever else the destination
/// puts under it, so it classifies like any other unrecorded path. Excluding
/// it would hide it from [`status`](crate::status), which reports what the
/// destination holds and has no business hiding a path the projection merely
/// refuses to touch — while the state subtree proper is excluded because the
/// projection's own files are not its output.
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
    admitted: &BTreeMap<Utf8PathBuf, &Entry>,
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
        && !target_resolves_in_dest(path, target, admitted, observations, vacated)
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
/// through the links the destination will hold, lands inside the
/// destination.
///
/// [`contained_target_chain`] is the rule; this supplies the destination it
/// resolves against, out of the plan-time observations and the desired tree
/// itself, so the stage stays filesystem-free.
fn target_resolves_in_dest(
    link: &Utf8Path,
    target: &str,
    admitted: &BTreeMap<Utf8PathBuf, &Entry>,
    observations: &Observations,
    vacated: &BTreeSet<Utf8PathBuf>,
) -> bool {
    let parent = link.parent().unwrap_or_else(|| Utf8Path::new(""));
    let landing = contained_target_chain(parent, target, |path| {
        Ok::<Hop, Infallible>(planned_hop(admitted, observations, path, vacated))
    });
    match landing {
        Ok(landing) => landing.is_some(),
        Err(never) => match never {},
    }
}

/// What will stand at one destination-relative path once this run finishes,
/// in the terms chain resolution asks in — the destination the pointer being
/// graded will actually live in.
///
/// Three sources, in that order:
///
/// - the desired tree, for a path this run writes. A pointer is graded
///   against the destination the run leaves, so a link the tree projects is
///   a hop like any other: without this a tree carrying both `b -> .` and
///   `a -> b/../escape` grades both in-dest and publishes a pointer that
///   dereferences outside the destination without the permission, and a tree
///   carrying a symlink cycle is written on the first run and refused on the
///   second. Reading the desired entry rather than the snapshot is sound
///   because a plan holding any [`Refuse`](Action::Refuse) applies nothing:
///   either every admitted entry lands, or none does.
/// - `vacated`, the paths this run unlinks. A link the run removes is not a
///   hop the pointer will resolve through — act executes every removal
///   first — the same reading [`resolves_through_link`] gives an ancestor
///   the plan removes.
/// - the observation snapshot, for everything the run leaves alone.
///
/// Only a symlink continues a chain: an absent path, a file — a block's
/// container included — a directory, and a kind the projection never writes
/// all end it, and so does a path the snapshot never mentions. A link whose on-disk target is not UTF-8 is
/// [`Unresolvable`](Hop::Unresolvable): nothing can say where it points, so
/// nothing can vouch for a chain through it (apply's walk refuses to follow
/// such a link for the same reason).
fn planned_hop(
    admitted: &BTreeMap<Utf8PathBuf, &Entry>,
    observations: &Observations,
    path: &Utf8Path,
    vacated: &BTreeSet<Utf8PathBuf>,
) -> Hop {
    match admitted.get(path) {
        Some(Entry::Symlink { target }) => return Hop::Link(target.clone()),
        Some(Entry::File { .. } | Entry::Block { .. }) => return Hop::Terminal,
        None => {}
    }
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
            | Observation::Block { .. }
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
    if let Some(refusal) = block_refusal(entry, observation) {
        return refuse(refusal);
    }
    let Some(state) = state else {
        // Nothing recorded and nothing on disk.
        return match entry {
            // A block never creates its container, so there is nothing here
            // for its region to sit in.
            Entry::Block { .. } => refuse(Refusal::Block {
                fault: BlockFault::ContainerMissing,
            }),
            Entry::File { .. } | Entry::Symlink { .. } => Action::Write {
                entry: entry.clone(),
            },
        };
    };
    if *state == PathState::Foreign {
        // A block owns the region, not the container around it: an
        // unrecorded regular file is exactly what a block is for, so it plans
        // a write and apply's read of the bytes tells splicing a region in
        // from adopting one already there from refusing one it did not write.
        if matches!(entry, Entry::Block { .. })
            && matches!(observation, Some(Observation::File { .. }))
        {
            return Action::Write {
                entry: entry.clone(),
            };
        }
        return refuse(Refusal::Foreign);
    }
    let recorded = recorded.expect("Clean, Drifted, and Missing paths are recorded");
    if recorded.kind.is_block() != matches!(entry, Entry::Block { .. }) {
        return refuse(Refusal::Block {
            fault: BlockFault::KindChange,
        });
    }
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
        PathState::Missing => match (entry, observation) {
            // The region is gone but the container still stands: splice it
            // back in — write heals. A container that is gone too does not,
            // since a block never creates one.
            (Entry::Block { .. }, Some(Observation::Block { .. })) => Action::Write {
                entry: entry.clone(),
            },
            (Entry::Block { .. }, _) => refuse(Refusal::Block {
                fault: BlockFault::ContainerMissing,
            }),
            _ => Action::Write {
                entry: entry.clone(),
            },
        },
        PathState::Clean => {
            let observation = observation.expect("a clean path was observed");
            if observation_matches_desired(entry, recorded, observation) {
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
            if observation_matches_desired(entry, recorded, observation) {
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

/// The refusals a desired [`Block`](Entry::Block) earns before its
/// classification is consulted: the marker and body rules
/// [`EntryKind::Block`] states, and — for
/// [`Append`](Placement::Append) — an author's side that does not end
/// with a newline.
///
/// The newline question is asked of the observation because it is about the
/// container the region would go into. Where a region was observed, the
/// author's side ends at the marker's line start and is newline-terminated by
/// construction, so this refuses only a container with no region in it or one
/// whose caller just moved the region to the other end. An *unrecorded*
/// container carries no region observation at all, so apply asks the same
/// question of the bytes it reads.
fn block_refusal(entry: &Entry, observation: Option<&Observation>) -> Option<Refusal> {
    let Entry::Block {
        body,
        marker,
        placement,
    } = entry
    else {
        return None;
    };
    if let Some(fault) = block::entry_fault(marker, *placement, body) {
        return Some(Refusal::Block { fault });
    }
    let author_ready = !matches!(
        observation,
        Some(Observation::Block {
            newline_terminated: false,
            ..
        })
    );
    if *placement == Placement::Append && !author_ready {
        return Some(Refusal::Block {
            fault: BlockFault::ContainerNotNewlineTerminated,
        });
    }
    None
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
/// expecting the drifted node. A node without a signature — a directory, a
/// kind the projection never writes, or a block whose container no longer
/// holds exactly one region the recorded marker identifies — is refused under
/// either policy.
fn lift_or_refuse_drift(
    recorded: &ManifestEntry,
    observation: &Observation,
    policy: DriftPolicy,
    lift: impl FnOnce(NodeSignature) -> Action,
) -> Action {
    match policy {
        DriftPolicy::Refuse => refuse(Refusal::Drift),
        DriftPolicy::Overwrite => match observed_signature(recorded, observation) {
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
        kind: recorded.kind.clone(),
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

/// The observed node's signature, where it has one: files, symlinks, and a
/// region the container still holds. `None` for absent paths, directories,
/// nodes the projection never writes, and a container whose marker line is
/// gone — nothing apply could re-check before a destructive action.
///
/// `recorded` says which node the observation is about. Where it is a block
/// the node is the region, so only a container still holding one has a
/// signature — the body the recorded marker and placement locate, under that
/// same recorded kind. A container swapped for a symlink or a directory
/// carries no region, so a lifted drift has nothing to re-verify and the
/// refusal holds under either policy.
///
/// Neither does a container holding the marker on more than one line, which
/// identifies no region at all ([`identified_region`]): lifting there would
/// strip a range nobody can say is the recorded one, and the region it
/// guessed wrong about would stay in the container with the manifest no
/// longer recording it.
fn observed_signature(
    recorded: &ManifestEntry,
    observation: &Observation,
) -> Option<NodeSignature> {
    if recorded.kind.is_block() {
        return identified_region(observation).map(|hash| NodeSignature {
            kind: recorded.kind.clone(),
            hash: hash.clone(),
            executable: false,
        });
    }
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
        Observation::Block { .. }
        | Observation::Absent
        | Observation::Directory
        | Observation::Other => None,
    }
}

/// Whether the observed node is exactly the recorded entry — kind and hash,
/// plus the executable bit for files. For a block that is the region's body:
/// the container's other bytes are the author's and enter no comparison.
///
/// A container holding the marker on more than one whole line matches
/// nothing, however its extreme occurrence hashes ([`identified_region`]).
fn observation_matches_recorded(recorded: &ManifestEntry, observation: &Observation) -> bool {
    match (&recorded.kind, observation) {
        (EntryKind::File, Observation::File { hash, executable }) => {
            *hash == recorded.hash && *executable == recorded.executable
        }
        (EntryKind::Symlink, Observation::Symlink { hash, .. }) => *hash == recorded.hash,
        (EntryKind::Block { .. }, observation) => {
            identified_region(observation) == Some(&recorded.hash)
        }
        _ => false,
    }
}

/// The hash of the region an observation identifies: `None` where the
/// container holds no marker occurrence, and `None` too where it holds more
/// than one.
///
/// The region is found by taking an extreme occurrence — the last for
/// [`Append`](Placement::Append), the first for
/// [`Prepend`](Placement::Prepend) — which is the projection's own only while
/// every other occurrence is a line outside the region. The body may carry
/// none, so a container the projection alone has written has exactly one; a
/// second bare marker line is somebody else's, and the marker is the whole of
/// a region's identity, so nothing says which of the two bounds the recorded
/// region. An author line above an `Append` region and a duplicate of the
/// region below it are the same picture from here, and the safe one cannot be
/// told from the ruinous one.
///
/// So such a container identifies no region. It matches neither the record
/// nor the desired entry, which classifies it [`Drifted`](PathState::Drifted)
/// and leaves nothing to skip, and it carries no signature
/// ([`observed_signature`]), which is what refuses the lift. Every action on
/// it refuses under either policy until the extra line is gone.
/// [`EntryKind::Block`]'s rustdoc states the rule and the indented or quoted
/// spelling that lets a container mention its marker without breaking it.
fn identified_region(observation: &Observation) -> Option<&String> {
    match observation {
        Observation::Block {
            hash: Some(hash),
            occurrences: 1,
            ..
        } => Some(hash),
        _ => None,
    }
}

/// Whether the observed node is exactly the desired entry — same comparison
/// as [`observation_matches_recorded`], against the desired side.
///
/// `recorded` is the entry the observation was taken against. A region is
/// located with the *recorded* marker and placement, so an observation
/// answers about a desired block only where the caller still names that same
/// pair; changing either asks about a region that is not the one on disk, and
/// the answer is no.
fn observation_matches_desired(
    entry: &Entry,
    recorded: &ManifestEntry,
    observation: &Observation,
) -> bool {
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
        (Entry::Block { body, .. }, observation) => {
            entry.kind() == recorded.kind
                && identified_region(observation) == Some(&sha256_hex(body))
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
        Entry::Block { body, .. } => sha256_hex(body),
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
