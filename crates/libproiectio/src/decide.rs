use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;

use camino::{Utf8Path, Utf8PathBuf};

use crate::block;
use crate::containment::{Hop, contained_normalize, contained_target_chain, is_pathname};
use crate::{
    Action, BlockFault, Desired, DriftPolicy, Entry, EntryKind, Error, ExternalTargetPolicy,
    Landing, MAX_WALK_DEPTH, Manifest, ManifestEntry, NodeSignature, Observation, Observations,
    Origin, OverwriteReason, PathFacts, PathShape, PathState, Placement, Plan, PlanOptions,
    Refusal, Report, Result, Row, Status, recorded_landing, sha256_hex, walked_ancestry,
};

/// One row per path in the union of the manifest and the observations,
/// skipping the state subtree named by `state_prefix`.
pub(crate) fn classify(
    manifest: &Manifest,
    observations: &Observations,
    state_prefix: Option<&Utf8Path>,
) -> Status {
    let mut rows = BTreeMap::new();
    for (path, observation) in &observations.paths {
        if in_state(path, state_prefix) {
            continue;
        }
        let recorded = manifest.entries.get(path);
        let verdict = match (recorded, observation) {
            (Some(recorded), observation) => recorded_state(recorded, Some(observation)),
            (None, Observation::Absent) => continue,
            (None, _) => PathState::Foreign,
        };
        rows.insert(
            path.clone(),
            Row {
                facts: recorded.map(|recorded| recorded_facts(recorded, Some(observation))),
                verdict,
            },
        );
    }
    for (path, recorded) in &manifest.entries {
        if in_state(path, state_prefix) {
            continue;
        }
        rows.entry(path.clone()).or_insert_with(|| Row {
            facts: Some(recorded_facts(recorded, None)),
            verdict: PathState::Missing,
        });
    }
    Report { rows }
}

/// [`classify`], less every unrecorded path observed as a directory.
pub(crate) fn status(
    manifest: &Manifest,
    observations: &Observations,
    state_prefix: Option<&Utf8Path>,
) -> Status {
    let mut report = classify(manifest, observations, state_prefix);
    report.rows.retain(|path, row| {
        !(row.verdict == PathState::Foreign
            && matches!(observations.paths.get(path), Some(Observation::Directory)))
    });
    report
}

/// The manifest records a link by the hash of its target, so the target string
/// comes from the observation: what the walk read at the path, and `None` where
/// the disk names none — nothing was reached, a non-link stands there, or the
/// target is not UTF-8.
fn recorded_facts(recorded: &ManifestEntry, observed: Option<&Observation>) -> PathFacts {
    let shape = match recorded.kind {
        EntryKind::File => PathShape::File {
            executable: recorded.executable,
        },
        EntryKind::Symlink => PathShape::Symlink {
            target: match observed {
                Some(Observation::Symlink { target, .. }) => target.clone(),
                _ => None,
            },
        },
        EntryKind::Block { .. } => PathShape::Block,
    };
    PathFacts {
        shape: Some(shape),
        owners: recorded.owners.clone(),
        origin: None,
    }
}

/// The deciding stage: `(desired, manifest, observations) -> Plan`, with no
/// filesystem access.
pub(crate) fn decide(
    owner: &str,
    desired: &Desired,
    manifest: &Manifest,
    observations: &Observations,
    state_prefix: Option<&Utf8Path>,
    options: PlanOptions,
) -> Result<Plan> {
    let actions = plan_actions(
        owner,
        desired,
        manifest,
        observations,
        state_prefix,
        options,
        &Judged::Everything,
    );
    if let Some(path) = deepest_write(&actions) {
        return Err(Error::DestinationTooDeep {
            path,
            limit: MAX_WALK_DEPTH,
        });
    }
    Ok(Plan {
        owner: owner.to_owned(),
        origins: origins_of(desired, &actions),
        external_targets: options.external_targets,
        actions,
        dropped: desired.dropped().clone(),
    })
}

/// Each sourced desired key under the path its action landed on: the key
/// itself when an action is keyed by it, and its normalized location when
/// admission moved the action there.
fn origins_of(
    desired: &Desired,
    actions: &BTreeMap<Utf8PathBuf, Action>,
) -> BTreeMap<Utf8PathBuf, Origin> {
    desired
        .sources()
        .filter(|(_, origin)| **origin != Origin::Caller)
        .filter_map(|(key, origin)| {
            let path = if actions.contains_key(key) {
                key.clone()
            } else {
                contained_normalize(key)?
            };
            Some((path, origin.clone()))
        })
        .collect()
}

fn deepest_write(actions: &BTreeMap<Utf8PathBuf, Action>) -> Option<Utf8PathBuf> {
    actions
        .iter()
        .filter(|(_, action)| {
            matches!(
                action,
                Action::Write { .. } | Action::Overwrite { .. } | Action::OverwriteDirectory { .. }
            )
        })
        .map(|(path, _)| path)
        .find(|path| path.components().count() - 1 > MAX_WALK_DEPTH)
        .map(|path| {
            let head: Vec<&str> = path
                .components()
                .take(MAX_WALK_DEPTH + 1)
                .map(|component| component.as_str())
                .collect();
            Utf8PathBuf::from(head.join("/"))
        })
}

/// The plan that clears what `owner` holds, narrowed by `scope`: [`decide`]
/// against an empty desired tree. A requested path the manifest does not
/// record under `owner` yields [`Action::NotRecorded`], which changes
/// nothing and says so.
pub(crate) fn decide_removal(
    owner: &str,
    scope: RemovalScope<'_>,
    manifest: &Manifest,
    observations: &Observations,
    state_prefix: Option<&Utf8Path>,
    drift: DriftPolicy,
) -> Plan {
    let options = PlanOptions {
        drift,
        external_targets: ExternalTargetPolicy::Refuse,
    };
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
                        refused.insert(
                            request.clone(),
                            refuse(Refusal::Containment { through: None }),
                        );
                    }
                }
            }
            (Judged::Paths(admitted), refused)
        }
    };
    let mut actions = plan_actions(
        owner,
        &Desired::new(),
        manifest,
        observations,
        state_prefix,
        options,
        &judged,
    );
    actions.extend(refused);
    if let Judged::Paths(admitted) = &judged {
        for path in admitted {
            actions.entry(path.clone()).or_insert(Action::NotRecorded);
        }
    }
    Plan {
        owner: owner.to_owned(),
        origins: BTreeMap::new(),
        external_targets: options.external_targets,
        actions,
        dropped: BTreeSet::new(),
    }
}

/// What a removal clears: everything the owner holds, or the paths a caller
/// names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalScope<'a> {
    /// Every path recorded under the owner.
    Everything,
    /// The recorded paths this set names, and no others; an empty set names
    /// nothing and clears nothing.
    Paths(&'a BTreeSet<Utf8PathBuf>),
}

enum Judged {
    Everything,
    /// Normalized paths, matching the manifest's own keys.
    Paths(BTreeSet<Utf8PathBuf>),
}

impl Judged {
    fn covers(&self, path: &Utf8Path) -> bool {
        match self {
            Judged::Everything => true,
            Judged::Paths(paths) => paths.contains(path),
        }
    }
}

/// The action table behind [`decide`] and [`decide_removal`], with `judged`
/// narrowing which of the owner's recorded paths it judges.
fn plan_actions(
    owner: &str,
    desired: &Desired,
    manifest: &Manifest,
    observations: &Observations,
    state_prefix: Option<&Utf8Path>,
    options: PlanOptions,
    judged: &Judged,
) -> BTreeMap<Utf8PathBuf, Action> {
    let states = classify(manifest, observations, state_prefix);
    let mut actions: BTreeMap<Utf8PathBuf, Action> = BTreeMap::new();

    // `named` is every location the desired tree names, refused or not.
    let mut claims: BTreeMap<Utf8PathBuf, BTreeMap<&Utf8PathBuf, &Entry>> = BTreeMap::new();
    let mut named: BTreeSet<Utf8PathBuf> = BTreeSet::new();
    for (key, entry) in desired.iter() {
        let Some(normalized) = contained_normalize(key) else {
            actions.insert(key.clone(), refuse(Refusal::Containment { through: None }));
            continue;
        };
        named.insert(normalized.clone());
        if overlaps_state(&normalized, state_prefix) {
            actions.insert(key.clone(), refuse(Refusal::Containment { through: None }));
            continue;
        }
        claims.entry(normalized).or_default().insert(key, entry);
    }

    // Overlaps between distinct claimed locations: one normalized path lying
    // beneath another, recorded on both sides.
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

    // Recorded paths the desired tree no longer names, judged before the
    // admitted paths: a removal vacates its path, and act runs removals first.
    // Each carries the location it acts at, which is its key unless the walk
    // below came out somewhere else; an action that touches no node carries
    // none.
    let mut orphans: Vec<(Utf8PathBuf, Action, Option<Utf8PathBuf>)> = Vec::new();
    // The removals whose walk came out somewhere other than their key, each
    // with the link that took it there: what `recorded_landing` below grades
    // once the plan's own claims are known.
    let mut followed: BTreeMap<Utf8PathBuf, Landing> = BTreeMap::new();
    for (path, recorded) in &manifest.entries {
        if named.contains(path)
            || in_state(path, state_prefix)
            || !recorded.owners.contains(owner)
            || !judged.covers(path)
        {
            continue;
        }
        if overlaps_state(path, state_prefix) || !contained(path) {
            actions.insert(path.clone(), refuse(Refusal::Containment { through: None }));
            continue;
        }
        // A release walks nothing and reads no disk, so only the branch below
        // grades ancestry. It grades it with nothing vacated: removals run
        // deepest-first, so a recorded link above this path is still standing
        // when apply's own walk reaches it. Following one is what
        // `docs/implementation.lex` section 3 says a removal does, which puts
        // the node it re-checks at the location the walk came out at — and
        // that location answers to containment as the key does, since the
        // state subtree is out of the projection's reach however a walk
        // arrives at it.
        let (action, at) = if recorded.owners.len() > 1 {
            (Action::Release, None)
        } else {
            match walked_ancestry(path, manifest, observations, &BTreeSet::new(), false) {
                Err(refusal) => (refuse(refusal), None),
                Ok(Some(landing)) if overlaps_state(&landing.at, state_prefix) => (
                    refuse(Refusal::Containment {
                        through: landing.through,
                    }),
                    None,
                ),
                Ok(landing) => {
                    let at = match landing {
                        Some(landing) if landing.at != *path => {
                            let at = landing.at.clone();
                            followed.insert(path.clone(), landing);
                            at
                        }
                        Some(landing) => landing.at,
                        None => path.clone(),
                    };
                    // A block owns a region of the container, not the
                    // container, and two keys can reach one container: the
                    // region parsed under this record's own marker is stated
                    // under this record's key, wherever the container sits.
                    let observed = if recorded.kind.is_block() {
                        observations.paths.get(path)
                    } else {
                        observations.paths.get(&at)
                    };
                    let state = recorded_state(recorded, observed);
                    let action = drifted_directory(
                        owner,
                        &at,
                        recorded,
                        manifest,
                        observations,
                        options.drift,
                        || Action::RemoveDirectory,
                    )
                    .unwrap_or_else(|| orphan_action(recorded, observed, state, options.drift));
                    let acts = !matches!(action, Action::Refuse { .. });
                    (action, acts.then_some(at))
                }
            }
        };
        orphans.push((path.clone(), action, at));
    }

    // One physical node, one action. A removal that followed a recorded link
    // acts where it came out rather than at its key, so two keys can name one
    // node: apply would carry out the first and refuse the second half-way
    // through the run. Nothing orders them — the reading `docs/design.lex`
    // section 2 gives two desired keys over one location — so both refuse.
    // A removal expecting nothing claims nothing: apply only re-checks that
    // the location is empty, which leaves it free for another key's write.
    // A block removal claims one marker's region and republishes the
    // container, so two of them at one container conflict only where they
    // strip the same marker; every other claim is on the whole node and
    // conflicts with all of them.
    let mut claimed: BTreeMap<&Utf8Path, Vec<(Option<&str>, &Utf8Path)>> = BTreeMap::new();
    for (path, action, at) in &orphans {
        if matches!(action, Action::Remove { expected: None }) {
            continue;
        }
        if let Some(at) = at {
            let marker = manifest
                .entries
                .get(path)
                .and_then(|recorded| block::block_kind(&recorded.kind))
                .map(|(marker, _)| marker);
            claimed
                .entry(at.as_path())
                .or_default()
                .push((marker, path.as_path()));
        }
    }
    for path in admitted.keys() {
        claimed
            .entry(path.as_path())
            .or_default()
            .push((None, path.as_path()));
    }
    let mut collided: BTreeMap<Utf8PathBuf, BTreeSet<Utf8PathBuf>> = BTreeMap::new();
    for claims in claimed.into_values() {
        for (marker, key) in &claims {
            let others: BTreeSet<Utf8PathBuf> = claims
                .iter()
                .filter(|(other_marker, other)| {
                    other != key
                        && (marker.is_none() || other_marker.is_none() || other_marker == marker)
                })
                .map(|(_, other)| (*other).to_owned())
                .collect();
            if !others.is_empty() {
                collided.insert((*key).to_owned(), others);
            }
        }
    }
    let conflict = |paths: &BTreeSet<Utf8PathBuf>| {
        refuse(Refusal::TreeConflict {
            paths: paths.clone(),
        })
    };

    // Every location this run leaves empty, which is what the ancestry walks,
    // the planned symlink targets, and the directory readings below all ask
    // about — so each is the location a removal acts at, not the key it is
    // filed under. A removal that strips a block republishes its container,
    // so only the whole-node removals empty anything.
    let mut vacated: BTreeSet<Utf8PathBuf> = BTreeSet::new();
    for (path, action, at) in orphans {
        // A landing no other action of this plan claims, but the manifest
        // records: unlinking it would take a node its owners still hold, and
        // no verdict of this plan is about that node. Graded after the
        // collisions above, which are the landings this plan does act on.
        let action = match collided.get(&path) {
            Some(paths) => conflict(paths),
            None => followed
                .get(&path)
                .and_then(|landing| recorded_landing(landing, manifest))
                .map_or(action, refuse),
        };
        let whole_node = !manifest
            .entries
            .get(&path)
            .is_some_and(|recorded| recorded.kind.is_block());
        if matches!(action, Action::Remove { .. } | Action::RemoveDirectory) && whole_node {
            vacated.insert(at.unwrap_or_else(|| path.clone()));
        }
        actions.insert(path, action);
    }

    // Decided against the removals above rather than inserted among them: a
    // directory standing at a desired path is cleared by those removals, and
    // whether they empty it is what `directory_in_the_way` reads.
    let mut desired_actions: Vec<(Utf8PathBuf, Action)> = Vec::new();
    for (path, entry) in &admitted {
        if let Some(paths) = collided.get(path) {
            desired_actions.push((path.clone(), conflict(paths)));
            continue;
        }
        let action = match link_refusal(
            path,
            entry,
            &admitted,
            manifest,
            observations,
            &vacated,
            options,
        ) {
            Some(refusal) => refuse(refusal),
            None => directory_action(
                owner,
                path,
                entry,
                manifest,
                observations,
                &vacated,
                options.drift,
            )
            .unwrap_or_else(|| {
                desired_action(
                    owner,
                    entry,
                    states.rows.get(path).map(|row| &row.verdict),
                    manifest.entries.get(path),
                    observations.paths.get(path),
                    options.drift,
                )
            }),
        };
        desired_actions.push((path.clone(), action));
    }
    actions.extend(desired_actions);

    actions
}

/// The action for a desired path standing on a directory: the write that
/// clears the projection's own scaffolding out of the way, the overwrite that
/// replaces an empty directory the record drifted into, or the refusal naming
/// what holds the directory in place. `None` leaves the path to
/// [`desired_action`] — nothing stands there, what stands there is not a
/// directory, a block is involved, or the record is not this owner's alone.
fn directory_action(
    owner: &str,
    path: &Utf8Path,
    entry: &Entry,
    manifest: &Manifest,
    observations: &Observations,
    vacated: &BTreeSet<Utf8PathBuf>,
    policy: DriftPolicy,
) -> Option<Action> {
    if !matches!(observations.paths.get(path), Some(Observation::Directory))
        || matches!(entry, Entry::Block { .. })
    {
        return None;
    }
    match manifest.entries.get(path) {
        Some(recorded) => drifted_directory(
            owner,
            path,
            recorded,
            manifest,
            observations,
            policy,
            || Action::OverwriteDirectory {
                entry: entry.clone(),
            },
        ),
        None => Some(
            match directory_in_the_way(path, manifest, observations, vacated) {
                Some(refusal) => refuse(refusal),
                None => Action::Write {
                    entry: entry.clone(),
                },
            },
        ),
    }
}

/// The action for a recorded path whose on-disk node is a directory: the
/// record drifted into a kind no signature describes, so only an empty
/// directory goes, and only where `policy` lifts the drift. Anything the
/// directory holds is unrecorded at that location, so no policy clears it.
/// `None` where the path is not a directory on disk, or is a block, or is a
/// record `owner` does not hold alone: a kind swap is no way past the owner
/// boundary, so a shared or another owner's record goes on to
/// [`desired_action`] and its [`Refusal::OwnerConflict`].
fn drifted_directory(
    owner: &str,
    path: &Utf8Path,
    recorded: &ManifestEntry,
    manifest: &Manifest,
    observations: &Observations,
    policy: DriftPolicy,
    clear: impl FnOnce() -> Action,
) -> Option<Action> {
    if !matches!(observations.paths.get(path), Some(Observation::Directory))
        || recorded.kind.is_block()
        || recorded.owners.len() > 1
        || !recorded.owners.contains(owner)
    {
        return None;
    }
    // Nothing beneath a drifted directory is recorded at that location, so no
    // removal reaches any of it: everything standing there holds it.
    let unreadable = unreadable_beneath(path, observations);
    let (held, holding) = holding_beneath(path, manifest, observations, |node, _| {
        unreadable.contains(node)
    });
    if held || !unreadable.is_empty() {
        return Some(refuse(Refusal::DirectoryInTheWay {
            holding,
            unreadable,
        }));
    }
    Some(match policy {
        DriftPolicy::Refuse => refuse(Refusal::Drift),
        DriftPolicy::Overwrite => clear(),
    })
}

/// What keeps the unrecorded directory at `path` standing after this run's
/// removals: every node on disk beneath it that no removal vacates, every
/// directory beneath it that no removal empties, and every directory beneath
/// it holding a name observation cannot represent. `None` where the run
/// empties the directory and pruning takes it; the refusal carries an empty
/// `holding` where the directory holds nothing at all, which makes it
/// somebody else's rather than this projection's scaffolding.
fn directory_in_the_way(
    path: &Utf8Path,
    manifest: &Manifest,
    observations: &Observations,
    vacated: &BTreeSet<Utf8PathBuf>,
) -> Option<Refusal> {
    let emptied = |directory: &Utf8Path| {
        vacated
            .iter()
            .any(|node| node.starts_with(directory) && node != directory)
    };
    let unreadable = unreadable_beneath(path, observations);
    let (held, holding) = holding_beneath(path, manifest, observations, |node, observation| {
        // What this run removes from a directory holding an unrepresentable
        // name never adds up to an empty one, and `unreadable` already names
        // it with the reason it stays.
        unreadable.contains(node)
            || match observation {
                // A directory this run removes outright is one
                // `drifted_directory` already found empty, so it goes whether
                // or not a deeper removal empties it.
                Observation::Directory => vacated.contains(node) || emptied(node),
                _ => vacated.contains(node),
            }
    });
    let blocked = held || !unreadable.is_empty();
    (blocked || !emptied(path)).then_some(Refusal::DirectoryInTheWay {
        holding,
        unreadable,
    })
}

/// What stands beneath `path` that `cleared` does not account for, and whether
/// anything does. Both rules that refuse a directory name the same thing by
/// it: a directory still standing because of what it holds is not itself worth
/// naming, since whatever holds it up is named below it, so the map carries
/// the leaves. One holding nothing has nothing below it to name, so it is
/// named here.
fn holding_beneath(
    path: &Utf8Path,
    manifest: &Manifest,
    observations: &Observations,
    cleared: impl Fn(&Utf8Path, &Observation) -> bool,
) -> (bool, BTreeMap<Utf8PathBuf, BTreeSet<String>>) {
    let mut held = false;
    let mut holding = BTreeMap::new();
    for (node, observation) in beneath(path, observations) {
        if cleared(node, observation) {
            continue;
        }
        held = true;
        if *observation == Observation::Directory && beneath(node, observations).next().is_some() {
            continue;
        }
        holding.insert(node.to_owned(), owners_of(manifest, node));
    }
    (held, holding)
}

/// The directory at `path` and every one beneath it that observation could
/// not state in full: each held a name that is not UTF-8, so no reading of
/// what the run removes concludes it empties.
fn unreadable_beneath(path: &Utf8Path, observations: &Observations) -> BTreeSet<Utf8PathBuf> {
    observations
        .unreadable
        .iter()
        .filter(|directory| directory.starts_with(path))
        .cloned()
        .collect()
}

/// Every node the walk saw beneath `path`, at any depth. A recorded path the
/// walk did not reach is not one of them: it stands nowhere, so it holds no
/// directory up.
fn beneath<'a>(
    path: &'a Utf8Path,
    observations: &'a Observations,
) -> impl Iterator<Item = (&'a Utf8Path, &'a Observation)> {
    observations
        .paths
        .iter()
        .map(|(node, observation)| (node.as_path(), observation))
        .filter(move |(node, observation)| {
            **observation != Observation::Absent && *node != path && node.starts_with(path)
        })
}

fn owners_of(manifest: &Manifest, path: &Utf8Path) -> BTreeSet<String> {
    manifest
        .entries
        .get(path)
        .map(|recorded| recorded.owners.clone())
        .unwrap_or_default()
}

/// Whether a manifest key is one the projection may act at: the same
/// containment contract a desired key passes, re-run against the recorded
/// side so a forged manifest decides the verdict apply would reach rather
/// than a removal apply then refuses.
fn contained(path: &Utf8Path) -> bool {
    contained_normalize(path).is_some_and(|normalized| normalized == *path)
}

/// Whether acting at `path` would touch the state subtree: symmetric, so a
/// location the state directory merely sits beneath overlaps too.
fn overlaps_state(path: &Utf8Path, state_prefix: Option<&Utf8Path>) -> bool {
    state_prefix.is_some_and(|prefix| path.starts_with(prefix) || prefix.starts_with(path))
}

/// Whether `path` is itself inside the state subtree, the prefix included.
///
/// Narrower than `overlaps_state` on purpose: a path the state directory
/// merely sits beneath still classifies and shows in `status`, while acting on
/// that same path refuses.
fn in_state(path: &Utf8Path, state_prefix: Option<&Utf8Path>) -> bool {
    state_prefix.is_some_and(|prefix| path.starts_with(prefix))
}

/// The symlink refusals for an admitted path, judged before its
/// classification; `None` admits it to the ordinary action table.
fn link_refusal(
    path: &Utf8Path,
    entry: &Entry,
    admitted: &BTreeMap<Utf8PathBuf, &Entry>,
    manifest: &Manifest,
    observations: &Observations,
    vacated: &BTreeSet<Utf8PathBuf>,
    options: PlanOptions,
) -> Option<Refusal> {
    match walked_ancestry(path, manifest, observations, vacated, true) {
        Err(refusal) => return Some(refusal),
        // A write goes down at its action key or nowhere, so a walk that
        // followed a recorded link out to somewhere else refuses.
        Ok(Some(landing)) if landing.at != *path => {
            return Some(Refusal::Containment {
                through: landing.through,
            });
        }
        Ok(_) => {}
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

/// `true` where a desired symlink's target, resolved from the link's parent
/// through the links the destination will hold, lands inside the destination.
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

/// What will stand at one destination-relative path once this run finishes:
/// the desired tree first, then `vacated`, then the observation snapshot.
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
    if let Some(refusal) = block_refusal(
        entry,
        recorded.is_some_and(|recorded| recorded.kind.is_block()),
        observation,
    ) {
        return refuse(refusal);
    }
    let Some(state) = state else {
        return match entry {
            Entry::Block { .. } => refuse(Refusal::Block {
                fault: BlockFault::ContainerMissing,
            }),
            Entry::File { .. } | Entry::Symlink { .. } => Action::Write {
                entry: entry.clone(),
            },
        };
    };
    if *state == PathState::Foreign {
        // A block owns the region, not the container: an unrecorded regular
        // file plans a write, and apply's read of the bytes settles the rest.
        if matches!(entry, Entry::Block { .. })
            && matches!(
                observation,
                Some(Observation::File { .. } | Observation::Block { .. })
            )
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
                skip(entry)
            } else {
                Action::Overwrite {
                    entry: entry.clone(),
                    expected: recorded_signature(recorded),
                    reason: clean_overwrite_reason(entry, recorded),
                }
            }
        }
        PathState::Drifted => {
            let observation = observation.expect("a drifted path was observed");
            if observation_matches_desired(entry, recorded, observation) {
                skip(entry)
            } else {
                lift_or_refuse_drift(recorded, observation, policy, |drifted| Action::Overwrite {
                    entry: entry.clone(),
                    expected: drifted,
                    reason: OverwriteReason::ForcedDrift,
                })
            }
        }
        PathState::Foreign => unreachable!("handled above"),
    }
}

/// The refusals a desired [`Block`](Entry::Block) earns before its
/// classification is consulted.
fn block_refusal(
    entry: &Entry,
    recorded_is_block: bool,
    observation: Option<&Observation>,
) -> Option<Refusal> {
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
    let author_ready = match observation {
        Some(Observation::Block {
            hash: None,
            desired: Some(desired),
            ..
        }) => desired.author_newline_terminated,
        Some(Observation::Block {
            newline_terminated, ..
        }) => *newline_terminated,
        _ => true,
    };
    if *placement == Placement::Append && !author_ready {
        return Some(Refusal::Block {
            fault: BlockFault::ContainerNotNewlineTerminated,
        });
    }
    let Some(Observation::Block {
        hash,
        occurrences,
        desired: Some(desired),
        ..
    }) = observation
    else {
        return None;
    };
    if hash.is_some() {
        return (*occurrences == 1 && desired.occurrences > 0).then_some(Refusal::Block {
            fault: BlockFault::MarkerInAuthorText,
        });
    }
    let adopted = desired.occurrences == 1 && desired.hash.as_deref() == Some(&sha256_hex(body));
    if desired.occurrences > 0 && !adopted {
        return Some(if recorded_is_block {
            Refusal::Drift
        } else {
            Refusal::Foreign
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

/// Resolves a drifted path under `policy`: refuse, or the action `lift`
/// builds expecting the drifted node's signature. A node without a signature
/// is refused under either policy.
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

/// The skip for a path whose disk node already equals the desired `entry`,
/// carrying the desired signature for apply to re-check and record.
fn skip(entry: &Entry) -> Action {
    Action::Skip {
        entry: entry.clone(),
        expected: desired_signature(entry),
    }
}

fn clean_overwrite_reason(entry: &Entry, recorded: &ManifestEntry) -> OverwriteReason {
    if entry.kind() == recorded.kind
        && desired_hash(entry) == recorded.hash
        && desired_executable(entry) != recorded.executable
    {
        OverwriteReason::ExecutableChanged
    } else {
        OverwriteReason::ContentChanged
    }
}

fn recorded_signature(recorded: &ManifestEntry) -> NodeSignature {
    NodeSignature {
        kind: recorded.kind.clone(),
        hash: recorded.hash.clone(),
        executable: recorded.executable,
    }
}

fn desired_signature(entry: &Entry) -> NodeSignature {
    NodeSignature {
        kind: entry.kind(),
        hash: desired_hash(entry),
        executable: desired_executable(entry),
    }
}

/// The observed node's signature, where it has one: files, symlinks, and a
/// region the container still identifies. `recorded` says which node the
/// observation is about.
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

/// The state of a recorded path, given what stands at the location a walk to
/// it comes out at. `None` is a location the walk never reached, which reads
/// exactly as [`Observation::Absent`] does.
fn recorded_state(recorded: &ManifestEntry, observation: Option<&Observation>) -> PathState {
    match observation {
        None | Some(Observation::Absent) => PathState::Missing,
        Some(Observation::Block { hash: None, .. }) if recorded.kind.is_block() => {
            PathState::Missing
        }
        Some(observation) => {
            if observation_matches_recorded(recorded, observation) {
                PathState::Clean
            } else {
                PathState::Drifted
            }
        }
    }
}

/// Whether the observed node is exactly the recorded entry — kind and hash,
/// plus the executable bit for files; for a block, the region's body alone.
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
/// container holds no marker occurrence, and `None` where it holds more
/// than one.
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

/// Whether the observed node is exactly the desired entry. `recorded` is the
/// entry the observation was taken against; a desired block whose marker or
/// placement differs from it never matches.
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
/// executable bit.
fn desired_matches_recorded(entry: &Entry, recorded: &ManifestEntry) -> bool {
    entry.kind() == recorded.kind
        && desired_executable(entry) == recorded.executable
        && desired_hash(entry) == recorded.hash
}

fn desired_hash(entry: &Entry) -> String {
    match entry {
        Entry::File { contents, .. } => sha256_hex(contents),
        Entry::Symlink { target } => sha256_hex(target.as_bytes()),
        Entry::Block { body, .. } => sha256_hex(body),
    }
}

fn desired_executable(entry: &Entry) -> bool {
    match entry {
        Entry::File { executable, .. } => *executable,
        Entry::Symlink { .. } | Entry::Block { .. } => false,
    }
}

#[cfg(test)]
#[path = "decide_tests.rs"]
mod tests;
