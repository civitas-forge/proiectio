//! The write stage: executes a [`Plan`] verbatim against the destination
//! and persists the [`Manifest`] into the state directory
//! (`docs/implementation.lex` sections 3, 5, and 6).
//!
//! All I/O goes through two capability handles: a `Dir` rooted at the
//! destination — every open refuses any resolution that escapes it — and a
//! second `Dir` rooted at the caller-chosen state directory, which holds
//! the manifest. The module is Unix-only, like [`observe`](crate::observe):
//! exec bits and symlinks are the behavior under test, and the crate does
//! not target Windows yet.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Write as _;
use std::os::fd::AsFd;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::fs_utf8::{Dir, MetadataExt};
use cap_tempfile::TempFile;
use serde::Deserialize;

use crate::block;
use crate::containment::{
    Hop, contained_normalize, contained_target, contained_target_chain, is_pathname,
};
use crate::observe::{Container, io_error, read_container, sha256_hex_of_reader};
use crate::{
    Action, ApplyOutcome, ApplyReport, BlockFault, Entry, EntryKind, Error, ExternalTargetPolicy,
    MANIFEST_FILE_NAME, MANIFEST_VERSION, MAX_WALK_DEPTH, Manifest, ManifestEntry, NodeSignature,
    Origin, Placement, Plan, Refusal, Result, sha256_hex,
};

/// Loads the manifest from `state`'s [`MANIFEST_FILE_NAME`]; a state
/// directory that has no manifest file yet — a first run — loads as the
/// empty [`Manifest`].
///
/// The declared version is read leniently before the strict decode, so an
/// unsupported future format — likely carrying fields this version does not
/// know — reports [`Error::ManifestVersion`], not
/// [`Error::ManifestFormat`]. Error paths are reported relative to the
/// state directory, which is all the capability handle can name.
pub(crate) fn load_manifest(state: &Dir) -> Result<Manifest> {
    let path = Utf8Path::new(MANIFEST_FILE_NAME);
    let bytes = match state.read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Manifest::new()),
        Err(e) => return Err(io_error(path)(e)),
    };
    #[derive(Deserialize)]
    struct VersionProbe {
        version: u32,
    }
    let probe: VersionProbe =
        serde_json::from_slice(&bytes).map_err(|source| Error::ManifestFormat {
            path: path.to_owned(),
            source,
        })?;
    if probe.version != MANIFEST_VERSION {
        return Err(Error::ManifestVersion {
            path: path.to_owned(),
            found: probe.version,
            supported: MANIFEST_VERSION,
        });
    }
    serde_json::from_slice(&bytes).map_err(|source| Error::ManifestFormat {
        path: path.to_owned(),
        source,
    })
}

/// Atomically persists `manifest` as `state`'s [`MANIFEST_FILE_NAME`]: the
/// JSON is written to a tempfile inside the state directory and renamed
/// over the path, so a crash leaves the old manifest or the new one, never
/// a torn write — and a failed persist removes its tempfile on drop, so the
/// state directory is never littered. Mode `0o644`, set on the open
/// tempfile handle before the rename, so bytes and mode publish together.
pub(crate) fn save_manifest(state: &Dir, manifest: &Manifest) -> Result<()> {
    let path = Utf8Path::new(MANIFEST_FILE_NAME);
    let mut json = serde_json::to_vec_pretty(manifest).map_err(|source| Error::ManifestFormat {
        path: path.to_owned(),
        source,
    })?;
    json.push(b'\n');
    persist(state, MANIFEST_FILE_NAME, path, &json, false)
}

/// The write stage: executes `plan` verbatim against `dest` and persists
/// the updated manifest into `state` (`docs/implementation.lex` section 1;
/// the mechanics are section 3's cap-std adoption record).
///
/// `manifest` is the recorded state the plan was decided against; apply
/// records what it does into a copy and returns that copy in the
/// [`ApplyReport`], persisted to `state` as the run's last write. Plans are
/// plain data, not capabilities: apply re-validates every action key
/// against the lexical containment gateway, re-checks the disk against each
/// action's expected [`NodeSignature`] before every destructive or
/// recording step, and enforces the no-follow ancestor walk below — so a
/// hand-built or stale plan refuses rather than misfires. (The
/// state-directory exclusion is the deciding stage's admission check; this
/// function cannot see where `state` lives relative to `dest`.) A symlink's
/// target is re-graded against the live disk before the link is published,
/// rather than trusting the plan-time snapshot the verdict was taken from
/// (`settle_links`); [`Plan::external_targets`] says whether there is a
/// verdict to hold the destination to, since a caller who permitted
/// external targets permitted them whatever the destination now holds. What
/// containment means at apply time is the walk below, which writes
/// *through* no link it grades external.
///
/// # Up-front failures
///
/// Nothing is written when the plan cannot be honored whole:
///
/// - a plan carrying any [`Action::Refuse`] fails with the matching refusal
///   variant of [`Error`], aggregating every refused path — as does an
///   action key the containment gateway rejects or one not in normalized
///   form, and an [`Overwrite`](Action::Overwrite), [`Skip`](Action::Skip),
///   [`Remove`](Action::Remove), or [`Release`](Action::Release) keyed by a
///   path `manifest` does not record, which refuses as [`Error::Foreign`]:
///   the deciding stage plans those actions only for recorded paths, so on
///   an unrecorded one they would touch — or adopt — what the projection
///   never wrote. Where refusals of several kinds appear in one plan, the
///   variant reported first is fixed: [`Error::Containment`],
///   [`Error::TreeConflict`], [`Error::Foreign`], [`Error::Drift`],
///   [`Error::OwnerConflict`], [`Error::ExternalTarget`],
///   [`Error::InvalidTarget`], then [`Error::Block`];
/// - a plan writing a symlink whose target is not a pathname on any host —
///   the empty string, or one carrying a NUL byte — fails as
///   [`Error::InvalidTarget`]. Such a target would reach the OS and come
///   back an error partway through the run, which is the one thing this
///   check exists to prevent; a target the filesystem rejects for its
///   *length* still surfaces mid-run, since nothing lexical foresees that;
/// - a plan writing or overwriting a path more than [`MAX_WALK_DEPTH`]
///   directories below `dest` fails as [`Error::DestinationTooDeep`],
///   naming the directory a level past the limit. That is the depth
///   [`observe`](crate::observe) descends, so a node written deeper would
///   leave every later run unable to observe the destination at all — the
///   run that would remove it included. The key is judged here, and the
///   directory the walk below is about to create is judged again there:
///   an owned-link restart resolves a key to a path of another depth, and
///   only the walk knows which. Deciding judges neither, because its
///   verdicts are refusals and this is a failure — nothing is being
///   declined, the projection simply cannot write what it could not read
///   back. Removals are exempt, since they add no directory and are the
///   way back from a destination already too deep;
/// - a plan whose [`Block`](EntryKind::Block) entry breaks one of the marker
///   or body rules [`EntryKind::Block`] states, whose entry or signature
///   disagrees with the manifest about whether the path holds a region, or
///   whose signature names a marker and placement the manifest does not
///   record there, fails as [`Error::Block`]. The deciding stage produces
///   none of these; a plan is plain data, so a hand-built one meets them
///   here, and the last is what keeps an expectation from pointing apply's
///   strip at a line the author wrote.
///
/// # Blocks
///
/// A [`Block`](EntryKind::Block) action never reaches the generic write or
/// re-check below. Its container is opened once through the no-follow walk,
/// with `create = false` — a block never creates a directory or a container —
/// and the regular-file verdict, the mode and the bytes all come from that
/// one file description, so nothing can be substituted between the check and
/// the read. That same read serves the changed-since-plan re-check *and* the
/// splice, so an overwrite or a removal of a region has no window between
/// deciding the disk still matches and writing it. The bytes outside the
/// region are copied through by range — never parsed, compared, or passed
/// through a pattern substitution — and the container is republished with the
/// author's mode, taken off that descriptor rather than from the entry.
///
/// One case is the exception to the up-front promise above, and it is stated
/// rather than implied: a block over a container the manifest does not
/// record. [`observe`](crate::observe) locates a region with the *recorded*
/// marker, and there is none at a path never recorded, so what such a
/// container holds is unknown until this read — deciding plans a
/// [`Write`](Action::Write) either way, and the read then splices, adopts an
/// identical region, or refuses ([`Error::Foreign`] for a region carrying
/// other bytes, [`Error::Block`] for a container an
/// [`Append`](Placement::Append) cannot be added to). Those three refusals
/// arrive mid-run, after whatever the plan already applied. The manifest
/// still records exactly what landed, so a re-run heals rather than meeting
/// its own writes as foreign.
///
/// # What a block costs
///
/// Publishing by rename replaces the container's inode. The mode survives;
/// ownership, ACLs, extended attributes, the inode number and any other hard
/// link to the file do not. Writing in place would preserve them, and would
/// also permit a torn write inside somebody else's file and write through a
/// hard link into whatever else names that inode.
///
/// The run's guard serializes two proiectio runs sharing a state directory,
/// and therefore their container writes too. It does not cover the author's
/// editor, `git checkout`, another tool's installer, or **the window between
/// this read of the container and its
/// rename** — a concurrent write in that window is silently lost, because the
/// bytes outside the region are never compared to anything. Re-reading before
/// the rename would narrow that window, not close it, so it is not done. This
/// is the largest thing a block adds over a whole-file write.
///
/// # Execution
///
/// Actions run deterministically (`docs/implementation.lex` section 6):
/// removals first, in reverse sorted order — children before parents — with
/// directories emptied by removal pruned afterwards (deepest first; a
/// directory still holding anything, a non-UTF-8 name included, is kept,
/// never an error); then everything else in sorted order, parents before
/// children, creating missing parent directories on the way; then the
/// symlinks, which grading makes order-dependent (`settle_links`).
///
/// Every path to a mutation is resolved by a no-follow walk from the `dest`
/// handle: each ancestor component is opened with cap-primitives'
/// `open_dir_nofollow` from the previously verified handle, and the final
/// mutation happens relative to that verified parent — so a component
/// swapped for a symlink after its check cannot redirect a write. When the
/// walk does meet a symlink it consults the manifest
/// (`docs/security.lex` section 2: no write through a symlinked ancestor
/// unless the projection owns the link and it resolves in-dest):
///
/// - unrecorded — refused as [`Error::Containment`] carrying the action's
///   path;
/// - recorded, but the on-disk target no longer hashes to the recorded
///   string — refused as [`Error::Drift`] carrying the link's path, the
///   same refusal every stale plan gets;
/// - recorded and matching, but the target grades external — refused as
///   [`Error::Containment`]: an external target is never written through;
/// - recorded, matching, and in-dest — followed, by resolving the target
///   lexically from the link's parent and restarting the walk from the
///   `dest` root along the resolved path. Restarts carry a per-walk
///   visited set: revisiting a link means an owned-link cycle, refused as
///   [`Error::Containment`] rather than looped.
///
/// What a restart earns the action depends on what the action does, and
/// `docs/implementation.lex` section 3 states all three answers together. A
/// **write** — file, symlink, or a block's container — must go down at its
/// action key, since that is the path the manifest records: one the walk
/// relocated is refused as [`Error::Containment`] (`at_action_key`). A
/// **removal** follows the link and reports where it unlinked, so pruning
/// judges the directory that actually lost a child (`remove`) — nothing is
/// created and the manifest entry goes away either way. A **release** walks
/// nothing: it drops an owner from a manifest entry and reads no disk
/// ([`Action::Release`]).
///
/// A creating walk stops at [`MAX_WALK_DEPTH`] directories as well,
/// wherever a restart has taken it, failing as
/// [`Error::DestinationTooDeep`] before it creates the directory past the
/// limit. Unlike the up-front check on the key, this one can fire with
/// shallower directories already created, the way any mid-run failure can:
/// the depth an owned link resolves to is not knowable before the walk,
/// just as drift is not.
///
/// Deciding refuses to plan a write beneath a link that outlives the plan
/// (its no-alias rule), so in a decided plan these arms judge what appeared
/// in the gap between the two calls. The followed arm additionally carries
/// an action whose key lies beneath an owned link — a shape only a
/// hand-built plan or a manifest predating the no-alias rule produces,
/// since deciding, whose observations never descend a link, classifies such
/// a path Missing and plans a removal expecting nothing, which the arm
/// below then refuses as drift. A hand-built plan meets all four arms the
/// same way.
///
/// File bytes go through a tempfile created inside the verified parent and
/// renamed over the path, with permissions (the exec bit included) set on
/// the open tempfile handle before the rename — a crash leaves the old
/// file or the new one, never a torn write and never a visible file with a
/// wrong mode. A symlink is published the same way: created under a
/// temporary name in the verified parent and renamed over the path, so a
/// file becoming a link — or a link becoming a file — publishes in one
/// rename, and the target string reaches disk verbatim, whatever it points
/// at — after the deciding stage's grading has been re-run over it against
/// the disk, so a link whose target became escaping since the plan refuses
/// as [`Error::ExternalTarget`] instead of publishing. Before every
/// overwrite, removal, and skip the target is
/// re-checked against the action's expected signature — kind, hash,
/// executable bit — and a mismatch refuses as [`Error::Drift`] carrying the
/// path: the drift rule holds across the gap between plan and apply. A
/// node found where a [`Write`](Action::Write) expected none refuses as
/// [`Error::Drift`] when the path is recorded and [`Error::Foreign`]
/// otherwise. A `Dir` escape past the no-follow walk would mean a bug in
/// the walk itself — defense in depth — and surfaces as the [`Error::Io`]
/// it is.
///
/// # Error honesty (`docs/implementation.lex` section 5)
///
/// The first error aborts the run: no recovery, no rollback. But the
/// manifest reflects reality, not success — when any action has already
/// applied, the manifest recording exactly those actions is persisted to
/// `state` before the error returns, so a partial run heals on re-run
/// instead of wedging behind the Foreign rule. (Should that persist itself
/// fail, the action's error is still the one returned: it is the primary
/// truth about the run.) A failed write's tempfile is removed on drop —
/// `dest` is never littered with temp files.
pub(crate) fn apply(
    dest: &Dir,
    state: &Dir,
    manifest: &Manifest,
    plan: &Plan,
) -> Result<ApplyReport> {
    // Whole-plan validation reads nothing but `plan.actions`, so every
    // offending value it names is one the desired tree chose and the plan's
    // origin is the file to go and edit. The walk below is the other case:
    // it refuses over what it finds on disk, and those refusals keep
    // `Origin::Caller` — see `regrade_recorded_link`, which reads a target
    // string off a link a *past* run wrote, so naming this plan's mapping
    // would send a reader to a file the string is not in.
    validate(manifest, plan).map_err(|error| error.with_origin(&plan.origin))?;
    let mut manifest = manifest.clone();
    let mut outcomes = BTreeMap::new();
    match run(dest, &mut manifest, plan, &mut outcomes) {
        Ok(()) => {
            save_manifest(state, &manifest)?;
            Ok(ApplyReport { outcomes, manifest })
        }
        Err(error) => {
            // §5: the manifest records what was actually applied. The
            // action's error is the primary truth about the run; a failure
            // persisting the partial manifest cannot displace it.
            if !outcomes.is_empty() {
                let _ = save_manifest(state, &manifest);
            }
            Err(error)
        }
    }
}

/// The up-front whole-plan check behind [`apply`]'s "nothing is written"
/// promise: aggregates every planned refusal, every action key the lexical
/// containment gateway rejects, every recorded-path action keyed by a path
/// `manifest` does not record, and every entry apply cannot honor yet
/// into the single error the rustdoc's fixed precedence names.
fn validate(manifest: &Manifest, plan: &Plan) -> Result<()> {
    let mut drift = BTreeSet::new();
    let mut foreign = BTreeSet::new();
    let mut containment = BTreeSet::new();
    let mut tree_conflict = BTreeSet::new();
    let mut owner_conflicts = BTreeMap::new();
    let mut external = BTreeMap::new();
    let mut invalid = BTreeMap::new();
    let mut blocks: BTreeMap<Utf8PathBuf, BlockFault> = BTreeMap::new();
    let mut too_deep = None;
    for (path, action) in &plan.actions {
        // Refusals are keyed by the desired key verbatim — possibly a
        // spelling the gateway rejects, which is often why they refused —
        // so they are matched before the key is judged.
        if let Action::Refuse { refusal } = action {
            match refusal {
                Refusal::Drift => {
                    drift.insert(path.clone());
                }
                Refusal::Foreign => {
                    foreign.insert(path.clone());
                }
                Refusal::Containment => {
                    containment.insert(path.clone());
                }
                Refusal::TreeConflict { .. } => {
                    tree_conflict.insert(path.clone());
                }
                Refusal::OwnerConflict { owners } => {
                    owner_conflicts.insert(path.clone(), owners.clone());
                }
                Refusal::ExternalTarget { target } => {
                    external.insert(path.clone(), target.clone());
                }
                Refusal::InvalidTarget { target } => {
                    invalid.insert(path.clone(), target.clone());
                }
                Refusal::Block { fault } => {
                    blocks.insert(path.clone(), *fault);
                }
            }
            continue;
        }
        // Every other action mutates disk or manifest at its key, so the
        // key must already be in the gateway's normalized form: a plan is
        // plain data, and a hand-built `../escape` or `a/../b` key must
        // refuse here, not resolve.
        match contained_normalize(path) {
            Some(normalized) if normalized == *path => {}
            _ => {
                containment.insert(path.clone());
                continue;
            }
        }
        // Deciding plans Overwrite, Skip, Remove, and Release only for
        // recorded paths; a hand-built or stale plan keying one by a path
        // the manifest does not record would remove or overwrite a foreign
        // node whose signature happens to match — or, for Skip, adopt one
        // into the manifest and so onto the removal path. Foreign, always.
        if !matches!(action, Action::Write { .. }) && !manifest.entries.contains_key(path) {
            foreign.insert(path.clone());
            continue;
        }
        // A target that is not a pathname reaches the OS as one and comes
        // back an error, which would break the "nothing is written" promise
        // partway through a run. Deciding refuses such an entry; a
        // hand-built plan meets the same refusal here.
        let written = match action {
            Action::Write { entry } | Action::Overwrite { entry, .. } => Some(entry),
            _ => None,
        };
        if let Some(Entry::Symlink { target }) = written {
            if !is_pathname(target) {
                invalid.insert(path.clone(), target.clone());
                continue;
            }
        }
        // A path this run would put a node at must be one the next
        // observation can read back: `observe` descends `MAX_WALK_DEPTH`
        // directories below the destination, so anything written deeper
        // would leave every later run failing to observe the destination at
        // all — the run that would remove what was written included. The
        // walk-shaped errors name the first offender rather than aggregate,
        // and the plan is sorted, so the first one found is the one named.
        // Removals are exempt: they add no directory, and this is the only
        // route left to a destination that was already too deep.
        if matches!(action, Action::Write { .. } | Action::Overwrite { .. })
            && too_deep.is_none()
            && path.components().count() - 1 > MAX_WALK_DEPTH
        {
            let mut offender = Vec::new();
            for component in path.components().take(MAX_WALK_DEPTH + 1) {
                offender.push(component.as_str());
            }
            // Joined with `/` rather than pushed onto a path: a
            // destination-relative path is spelled the same on every host,
            // as `contained_normalize` spells the keys this one prefixes.
            too_deep = Some(Utf8PathBuf::from(offender.join("/")));
        }
        // A block entry's own rules, and the one distinction a path never
        // crosses: the entry apply would write and the signature it would
        // re-check must both agree with the record about whether the node
        // here is a region rather than a whole node
        // ([`EntryKind::Block`](crate::EntryKind::Block)). A recorded path is
        // guaranteed above for every action but `Write`.
        let recorded_kind = manifest.entries.get(path).map(|recorded| &recorded.kind);
        let record_is_block = recorded_kind.is_some_and(EntryKind::is_block);
        match action {
            Action::Write { entry } => {
                if let Some(fault) = entry_block_fault(entry) {
                    blocks.insert(path.clone(), fault);
                }
                if manifest.entries.contains_key(path) && record_is_block != entry.kind().is_block()
                {
                    blocks.insert(path.clone(), BlockFault::KindChange);
                }
            }
            Action::Overwrite { entry, expected } => {
                if let Some(fault) = entry_block_fault(entry) {
                    blocks.insert(path.clone(), fault);
                }
                if record_is_block != entry.kind().is_block() {
                    blocks.insert(path.clone(), BlockFault::KindChange);
                }
                if let Some(fault) = signature_block_fault(recorded_kind, &expected.kind) {
                    blocks.insert(path.clone(), fault);
                }
            }
            Action::Skip { expected }
            | Action::Remove {
                expected: Some(expected),
            } => {
                if let Some(fault) = signature_block_fault(recorded_kind, &expected.kind) {
                    blocks.insert(path.clone(), fault);
                }
            }
            Action::Remove { expected: None } | Action::Release => {}
            Action::Refuse { .. } => unreachable!("matched above"),
        }
    }
    if !containment.is_empty() {
        return Err(Error::Containment {
            paths: containment,
            origin: Origin::Caller,
        });
    }
    if !tree_conflict.is_empty() {
        return Err(Error::TreeConflict {
            paths: tree_conflict,
            origin: Origin::Caller,
        });
    }
    if !foreign.is_empty() {
        return Err(Error::Foreign { paths: foreign });
    }
    if !drift.is_empty() {
        return Err(Error::Drift { paths: drift });
    }
    if !owner_conflicts.is_empty() {
        return Err(Error::OwnerConflict {
            conflicts: owner_conflicts,
        });
    }
    if !external.is_empty() {
        return Err(Error::ExternalTarget {
            links: external,
            origin: Origin::Caller,
        });
    }
    if !invalid.is_empty() {
        return Err(Error::InvalidTarget {
            links: invalid,
            origin: Origin::Caller,
        });
    }
    if !blocks.is_empty() {
        return Err(Error::Block { blocks });
    }
    if let Some(path) = too_deep {
        return Err(Error::DestinationTooDeep {
            path,
            limit: MAX_WALK_DEPTH,
        });
    }
    Ok(())
}

/// What an action's expected signature earns from the record at its path:
/// the one distinction a path never crosses, and — where the record is a
/// block — the region that signature names.
///
/// A block signature is what locates the bytes apply strips or replaces, so
/// it must name the region the manifest records, marker and placement both.
/// A signature naming another marker would have apply treat lines the author
/// wrote as the projection's region and strip or replace them. The *entry* of
/// an overwrite may name a new marker — that is the migration one publish
/// performs — but the expectation is what the disk is re-checked against, and
/// it is the record's.
fn signature_block_fault(recorded: Option<&EntryKind>, expected: &EntryKind) -> Option<BlockFault> {
    let record_is_block = recorded.is_some_and(EntryKind::is_block);
    if record_is_block != expected.is_block() {
        return Some(BlockFault::KindChange);
    }
    if record_is_block && recorded != Some(expected) {
        return Some(BlockFault::SignatureNotRecorded);
    }
    None
}

/// The plan-time refusals a written [`Block`](Entry::Block) entry earns from
/// its own fields — the same check the deciding stage runs, repeated here
/// because a plan is plain data and a hand-built one must meet it too.
fn entry_block_fault(entry: &Entry) -> Option<BlockFault> {
    match entry {
        Entry::Block {
            body,
            marker,
            placement,
        } => block::entry_fault(marker, *placement, body),
        Entry::File { .. } | Entry::Symlink { .. } => None,
    }
}

/// Executes a validated plan's actions in the documented order, recording
/// into `manifest` and `outcomes` as each action lands — so on a mid-run
/// error both hold exactly what was applied.
fn run(
    dest: &Dir,
    manifest: &mut Manifest,
    plan: &Plan,
    outcomes: &mut BTreeMap<Utf8PathBuf, ApplyOutcome>,
) -> Result<()> {
    // Removals first, children before parents (§6: removals in reverse):
    // a plan may remove a recorded file and write beneath its former path
    // in one run, so the ground is cleared before anything is placed.
    let mut removed_dirs_candidates: BTreeSet<Utf8PathBuf> = BTreeSet::new();
    for (path, action) in plan.actions.iter().rev() {
        if let Action::Remove { expected } = action {
            // A block's removal strips its region out of a container that
            // stays, so it empties no directory and its re-check rides the
            // splice's own read rather than a separate one.
            if manifest
                .entries
                .get(path)
                .is_some_and(|recorded| recorded.kind.is_block())
            {
                remove_block(dest, manifest, path, expected.as_ref())?;
            } else if let Some(resolved) = remove(dest, manifest, path, expected.as_ref())? {
                // Only an actual disk removal can have emptied ancestors —
                // and the ancestors that lost a child are the *resolved*
                // location's, which differs from the action key's when the
                // walk followed an owned link.
                for ancestor in resolved.ancestors().skip(1) {
                    if !ancestor.as_str().is_empty() {
                        removed_dirs_candidates.insert(ancestor.to_owned());
                    }
                }
            }
            manifest.entries.remove(path);
            outcomes.insert(path.clone(), ApplyOutcome::Removed);
        }
    }
    prune(dest, manifest, &removed_dirs_candidates)?;
    // Then everything else in sorted order, parents before children — the
    // symlinks excepted. A link's target is graded against the destination
    // as it stands, so it waits for whatever the run is still going to put
    // where its target resolves through ([`settle_links`]).
    let mut links: Vec<(&Utf8PathBuf, &Action)> = Vec::new();
    let mut unpublished: BTreeSet<Utf8PathBuf> = BTreeSet::new();
    for (path, action) in &plan.actions {
        match action {
            Action::Write { entry } | Action::Overwrite { entry, .. }
                if matches!(entry, Entry::Symlink { .. }) =>
            {
                links.push((path, action));
                unpublished.insert(path.clone());
            }
            Action::Skip { expected } if expected.kind == EntryKind::Symlink => {
                links.push((path, action));
            }
            _ => {}
        }
    }
    for (path, action) in &plan.actions {
        match action {
            Action::Remove { .. } | Action::Refuse { .. } => {}
            Action::Write { entry } | Action::Overwrite { entry, .. }
                if matches!(entry, Entry::Symlink { .. }) => {}
            Action::Skip { expected } if expected.kind == EntryKind::Symlink => {}
            // Blocks before the generic arms: a region's re-check and its
            // splice share one read of the container, so neither goes through
            // `write` or `check_expected`.
            Action::Write { entry } if matches!(entry, Entry::Block { .. }) => {
                let outcome = write_block(dest, manifest, path, entry)?;
                record(manifest, path, entry, &plan.owner);
                outcomes.insert(path.clone(), outcome);
            }
            Action::Overwrite { entry, expected } if matches!(entry, Entry::Block { .. }) => {
                overwrite_block(dest, manifest, path, entry, expected)?;
                record(manifest, path, entry, &plan.owner);
                outcomes.insert(path.clone(), ApplyOutcome::Overwritten);
            }
            Action::Write { entry } => {
                write(dest, manifest, path, entry, true, plan, &unpublished)?;
                record(manifest, path, entry, &plan.owner);
                outcomes.insert(path.clone(), ApplyOutcome::Written);
            }
            Action::Overwrite { entry, expected } => {
                check_expected(dest, manifest, path, expected)?;
                write(dest, manifest, path, entry, false, plan, &unpublished)?;
                record(manifest, path, entry, &plan.owner);
                outcomes.insert(path.clone(), ApplyOutcome::Overwritten);
            }
            Action::Skip { expected } => {
                check_expected(dest, manifest, path, expected)?;
                skip(manifest, path, expected, &plan.owner);
                outcomes.insert(path.clone(), ApplyOutcome::Skipped);
            }
            Action::Release => {
                if let Some(entry) = manifest.entries.get_mut(path) {
                    entry.owners.remove(&plan.owner);
                    if entry.owners.is_empty() {
                        // Last owner out: an entry held by nobody records
                        // nothing. `decide` only plans Release on shared
                        // paths, but a plan is plain data.
                        manifest.entries.remove(path);
                    }
                }
                outcomes.insert(path.clone(), ApplyOutcome::Released);
            }
        }
    }
    settle_links(dest, manifest, plan, outcomes, links, unpublished)
}

/// Records `path` in `manifest` under `owner` with the signature the plan
/// expects — an [`Action::Skip`]'s whole effect on the manifest.
fn skip(manifest: &mut Manifest, path: &Utf8Path, expected: &NodeSignature, owner: &str) {
    let mut owners = manifest
        .entries
        .get(path)
        .map(|entry| entry.owners.clone())
        .unwrap_or_default();
    owners.insert(owner.to_owned());
    manifest.entries.insert(
        path.to_owned(),
        ManifestEntry {
            kind: expected.kind.clone(),
            hash: expected.hash.clone(),
            executable: expected.executable,
            owners,
        },
    );
}

/// Executes the plan's symlink actions, after everything else in the run —
/// the part of apply that grading makes order-dependent.
///
/// A link's target is graded against the destination as it stands
/// (`docs/security.lex` section 3), and a run may be putting the very thing
/// the target resolves through in place: replacing a `pivot -> /etc` with an
/// in-dest link, or replacing a link with a file so nothing lives beneath
/// it. Sorted order reaches `evil -> pivot/x` before `pivot`, so grading a
/// link where sorted order happens to place it would refuse a run whose
/// finished destination holds nothing external.
///
/// So a link is published only when two things hold at once: its target
/// grades in-dest against the disk, and the chain that graded it walked
/// through no path this run is still going to publish a link at. Otherwise
/// the link is *held*, not refused, and the pass repeats over what it held.
/// The second condition is what keeps a published link in-dest afterwards:
/// every path its own resolution passed through is already final, so no
/// later publication can move where it lands. Grading in-dest at the moment
/// of publishing is not enough on its own — publish `a -> b/../escape`
/// against a `b` that still points at a directory, then republish `b` at the
/// destination root, and `a` reaches outside without either grading ever
/// saying so.
///
/// Those two conditions are the whole of it, and what makes them enough is
/// that only a symlink can redirect a path:
///
/// - by the time settling starts, removals, prunes and every file the plan
///   writes have run, so the only node the destination gains from here on is
///   a link this loop publishes. A `write` still creates missing ancestor
///   directories, and a directory appearing where a chain found nothing
///   leaves that chain's landing exactly where it was.
/// - the chain asks about every path it walks past the link's own parent —
///   one question per name, and each pop and each followed link leaves the
///   walked prefix on a path it already asked about — so `unpublished`
///   covers all of them.
/// - the paths a chain walks but never asks about are the link's own parent
///   and that parent's ancestors, which the walk inside `hop_on_disk`
///   crosses. [`verified_parent`] has just opened or created every one of
///   them as a real directory, and a link is published by renaming over its
///   leaf, which cannot replace a directory. So none of them can turn into a
///   link before the run ends.
/// - `unpublished` names each remaining link by its action key, and [`write`]
///   refuses a link whose walk resolved to a different location, so the key
///   is the location the link goes down at.
///
/// Together: after a link is published, every path its resolution passed
/// through is either final or a directory that stays one, so nothing the
/// rest of the run does moves where it lands.
///
/// Each pass either publishes something or the destination will never
/// satisfy the rest, so a pass that publishes nothing refuses every link
/// still waiting as [`Error::ExternalTarget`] — which is also how a plan
/// whose links wait on each other in a cycle ends, though deciding refuses
/// such a tree before apply sees it. The run therefore never holds a pointer
/// out of the destination that it published, not between two actions and not
/// after a run that failed partway. The cost is a pass per step of the
/// longest chain a tree's own links form, which is one pass for every tree
/// that does not point through a link it is itself writing.
///
/// Symlink skips come last, once the destination is finished: nothing is
/// published for them, but a plan-time in-dest verdict is still a verdict,
/// and a pivot swapped under an untouched recorded link invalidates it the
/// same way it invalidates one this run writes.
fn settle_links(
    dest: &Dir,
    manifest: &mut Manifest,
    plan: &Plan,
    outcomes: &mut BTreeMap<Utf8PathBuf, ApplyOutcome>,
    links: Vec<(&Utf8PathBuf, &Action)>,
    mut unpublished: BTreeSet<Utf8PathBuf>,
) -> Result<()> {
    let (mut pending, skips): (Vec<_>, Vec<_>) = links
        .into_iter()
        .partition(|(_, action)| !matches!(action, Action::Skip { .. }));
    while !pending.is_empty() {
        let before = pending.len();
        let mut held: Vec<(&Utf8PathBuf, &Action)> = Vec::new();
        let mut escaping: BTreeMap<Utf8PathBuf, String> = BTreeMap::new();
        for (path, action) in pending {
            let (entry, fresh, outcome) = match action {
                Action::Write { entry } => (entry, true, ApplyOutcome::Written),
                Action::Overwrite { entry, expected } => {
                    check_expected(dest, manifest, path, expected)?;
                    (entry, false, ApplyOutcome::Overwritten)
                }
                _ => unreachable!("only writes and overwrites are pending here"),
            };
            match write(dest, manifest, path, entry, fresh, plan, &unpublished)? {
                Written::Published => {
                    unpublished.remove(path);
                    record(manifest, path, entry, &plan.owner);
                    outcomes.insert(path.clone(), outcome);
                }
                Written::Held => {
                    let Entry::Symlink { target } = entry else {
                        unreachable!("only a symlink is ever held");
                    };
                    escaping.insert(path.clone(), target.clone());
                    held.push((path, action));
                }
            }
        }
        if held.len() == before {
            // These targets are the plan's own desired entries, so the tree
            // that chose them is the one to name.
            return Err(Error::ExternalTarget {
                links: escaping,
                origin: plan.origin.clone(),
            });
        }
        pending = held;
    }
    for (path, action) in skips {
        let Action::Skip { expected } = action else {
            unreachable!("only skips are left here");
        };
        check_expected(dest, manifest, path, expected)?;
        regrade_recorded_link(dest, manifest, plan, path)?;
        skip(manifest, path, expected, &plan.owner);
        outcomes.insert(path.clone(), ApplyOutcome::Skipped);
    }
    Ok(())
}

/// Grades the target of the link already on disk at `path`, for an
/// [`Action::Skip`] the run leaves in place: the plan called it in-dest, and
/// this holds the finished destination to that verdict.
///
/// The target is read off the disk rather than the plan, which carries only
/// the link's [`NodeSignature`] — [`check_expected`] has already held that
/// signature against the node, so the string read here is the one the plan
/// graded. A matching hash proves agreement with the record, not UTF-8: a
/// manifest this crate never writes can record the hash of raw bytes, and
/// what cannot be graded is refused as [`Error::Containment`], the verdict
/// the ancestor walk gives the same shape.
fn regrade_recorded_link(
    dest: &Dir,
    manifest: &Manifest,
    plan: &Plan,
    path: &Utf8Path,
) -> Result<()> {
    if plan.external_targets == ExternalTargetPolicy::Allow {
        return Ok(());
    }
    let Some((parent, leaf, resolved_parent)) = verified_parent(dest, manifest, path, false)?
    else {
        return Err(drift(path));
    };
    let target = parent
        .as_cap_std()
        .read_link_contents(&leaf)
        .map_err(io_error(path))?;
    let bytes = std::os::unix::ffi::OsStrExt::as_bytes(target.as_os_str());
    let Ok(target) = std::str::from_utf8(bytes) else {
        return Err(containment(path));
    };
    if link_settles(dest, plan, &BTreeSet::new(), &resolved_parent, target)? {
        return Ok(());
    }
    // The target string was read off the disk, not out of this plan's tree:
    // the link is one a past run wrote, and a `Skip` carries only its
    // signature. So the refusal names no source — this plan's origin would
    // point a reader at a file the string is not in.
    Err(Error::ExternalTarget {
        links: BTreeMap::from([(path.to_owned(), target.to_owned())]),
        origin: Origin::Caller,
    })
}

/// Executes one [`Action::Remove`]: with an expected signature, re-checks
/// the node and unlinks it through the verified parent handle, returning
/// the *resolved* location it unlinked — the action key unless the walk
/// followed an owned link — so pruning judges the ancestors that actually
/// lost a child; expecting `None` — the node was already gone at plan time
/// — verifies nothing has appeared, touches only the manifest, and returns
/// `None`. Either way the entry leaves the manifest.
fn remove(
    dest: &Dir,
    manifest: &Manifest,
    path: &Utf8Path,
    expected: Option<&NodeSignature>,
) -> Result<Option<Utf8PathBuf>> {
    match expected {
        Some(expected) => {
            let Some((parent, leaf, resolved_parent)) =
                verified_parent(dest, manifest, path, false)?
            else {
                // An ancestor is gone, so the node is too: the disk no
                // longer holds what the plan expects.
                return Err(drift(path));
            };
            check_leaf(&parent, &leaf, path, expected)?;
            parent.remove_file(&leaf).map_err(io_error(path))?;
            Ok(Some(resolved_parent.join(leaf)))
        }
        None => {
            if let Some((parent, leaf, _)) = verified_parent(dest, manifest, path, false)? {
                match parent.symlink_metadata(&leaf) {
                    // A node appeared at the path since the plan: a change,
                    // refused exactly like a present node changing.
                    Ok(_) => return Err(drift(path)),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(io_error(path)(e)),
                }
            }
            Ok(None)
        }
    }
}

/// Prunes directories emptied by this run's removals: every ancestor of a
/// removed node's resolved location, deepest first, removed through the
/// verified walk when empty. A directory still holding anything — a foreign file, another
/// projected path, an entry whose name is not UTF-8 — is kept, not an
/// error; so is one already gone or no longer a directory.
fn prune(dest: &Dir, manifest: &Manifest, candidates: &BTreeSet<Utf8PathBuf>) -> Result<()> {
    use std::io::ErrorKind;
    for path in candidates.iter().rev() {
        let Some((parent, leaf, _)) = verified_parent(dest, manifest, path, false)? else {
            continue;
        };
        match parent.remove_dir(&leaf) {
            Ok(()) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::DirectoryNotEmpty | ErrorKind::NotFound | ErrorKind::NotADirectory
                ) => {}
            Err(e) => return Err(io_error(path)(e)),
        }
    }
    Ok(())
}

/// Writes a planned entry at `path` inside the verified parent, publishing
/// it over the leaf in one rename — a tempfile for a file's bytes, a
/// temporarily named link for a symlink. `fresh` marks an
/// [`Action::Write`], whose target must still be absent: a node found there
/// refuses — [`Error::Drift`] when the path is recorded (it changed
/// relative to the plan's view), [`Error::Foreign`] otherwise (something
/// the projection never wrote appeared). A symlink's target is re-graded
/// against the destination as it stands before it is published, and the
/// write comes back [`Held`](Written::Held) — not refused — where it does
/// not land in-dest yet, for [`settle_links`] to try again once the run has
/// put more of the destination in place. Only a symlink is ever held, so a
/// caller writing a file has nothing to read in the answer.
///
/// Nothing is written where the walk did not go down at the action key
/// (`at_action_key`) — a symlink, a file, and a block's container alike.
/// A block never reaches this function: [`run`] splices its region instead,
/// through a walk that creates nothing but is held to the same key.
///
/// The key is judged after the walk, so a walk that created directories on
/// its way to a relocated landing leaves them behind, like any mid-run
/// failure (`docs/implementation.lex` section 5): what the refusal promises
/// is that no node goes down off its key, not that the run leaves no trace.
fn write(
    dest: &Dir,
    manifest: &Manifest,
    path: &Utf8Path,
    entry: &Entry,
    fresh: bool,
    plan: &Plan,
    unpublished: &BTreeSet<Utf8PathBuf>,
) -> Result<Written> {
    let Some((parent, leaf, resolved_parent)) = verified_parent(dest, manifest, path, true)? else {
        unreachable!("a creating walk opens or creates every ancestor");
    };
    at_action_key(path, &resolved_parent.join(&leaf))?;
    if fresh {
        match parent.symlink_metadata(&leaf) {
            Ok(_) => {
                return Err(if manifest.entries.contains_key(path) {
                    drift(path)
                } else {
                    Error::Foreign {
                        paths: BTreeSet::from([path.to_owned()]),
                    }
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error(path)(e)),
        }
    }
    match entry {
        Entry::File {
            contents,
            executable,
        } => persist(&parent, &leaf, path, contents, *executable)?,
        Entry::Symlink { target } => {
            if !link_settles(dest, plan, unpublished, &resolved_parent, target)? {
                return Ok(Written::Held);
            }
            publish_link(&parent, &leaf, path, target)?;
        }
        Entry::Block { .. } => {
            unreachable!("`run` splices a region rather than reaching this write")
        }
    }
    Ok(Written::Published)
}

/// What one [`write`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Written {
    /// The entry is on disk.
    Published,
    /// A symlink whose target does not grade in-dest against the
    /// destination as it stands. Nothing was written; [`settle_links`]
    /// tries again after the rest of the run has landed.
    Held,
}

/// Publishes `contents` at `leaf` inside `dir` atomically: tempfile,
/// permissions set on the open handle — `0o755` executable, `0o644` not —
/// then rename over the path. A failure at any step drops the tempfile,
/// which removes it: no litter.
fn persist(
    dir: &Dir,
    leaf: &str,
    path: &Utf8Path,
    contents: &[u8],
    executable: bool,
) -> Result<()> {
    persist_mode(
        dir,
        leaf,
        path,
        contents,
        if executable { 0o755 } else { 0o644 },
    )
}

/// [`persist`] with the mode named outright, for the one node whose mode is
/// not the entry's: a block's container keeps the author's permission bits,
/// read off the same descriptor the bytes came from. setuid, setgid and
/// sticky are not among them — [`read_container`](crate::observe::read_container)
/// drops them, and says why.
fn persist_mode(dir: &Dir, leaf: &str, path: &Utf8Path, contents: &[u8], mode: u32) -> Result<()> {
    let mut temp = TempFile::new(dir.as_cap_std()).map_err(io_error(path))?;
    temp.write_all(contents).map_err(io_error(path))?;
    let permissions =
        cap_std::fs::Permissions::from_std(std::os::unix::fs::PermissionsExt::from_mode(mode));
    temp.as_file()
        .set_permissions(permissions)
        .map_err(io_error(path))?;
    temp.replace(leaf).map_err(io_error(path))
}

/// Opens the container of the block at `path` through the no-follow ancestor
/// walk and reads it once ([`read_container`]): the verified parent handle,
/// the leaf name, the container's bytes, and the author's mode.
///
/// `None` means nothing stands at the path — the container itself, or a
/// directory above it, is gone — which each caller reads differently. The
/// walk runs with `create = false`: a block never creates a directory, so it
/// can neither deepen the destination nor strand one on a refusal.
///
/// A node that is not a regular file — a directory, a FIFO, or a symlink,
/// which the open declines to follow — is refused here rather than handed
/// back: [`Error::Drift`] where the manifest records the path, since the node
/// changed under the plan, and [`Error::Foreign`] where it does not, since
/// the projection never wrote it.
///
/// [`OpenContainer::landing`] carries where the walk came out, so the two
/// callers that republish a container — [`write_block`] and
/// [`overwrite_block`] — hold it to the action key (`at_action_key`),
/// while [`remove_block`] follows an owned link like every other removal.
fn read_block_container(
    dest: &Dir,
    manifest: &Manifest,
    path: &Utf8Path,
) -> Result<Option<OpenContainer>> {
    let Some((parent, leaf, resolved_parent)) = verified_parent(dest, manifest, path, false)?
    else {
        return Ok(None);
    };
    let landing = resolved_parent.join(&leaf);
    match read_container(&parent, &leaf, path)? {
        Container::File { bytes, mode } => Ok(Some(OpenContainer {
            parent,
            leaf,
            bytes,
            mode,
            landing,
        })),
        Container::Absent => Ok(None),
        Container::Other => Err(if manifest.entries.contains_key(path) {
            drift(path)
        } else {
            Error::Foreign {
                paths: BTreeSet::from([path.to_owned()]),
            }
        }),
    }
}

/// A block's container as one read described it, together with the handle
/// and name its republish renames through.
struct OpenContainer {
    /// The verified parent directory the container sits in.
    parent: Dir,
    /// The container's name inside that directory.
    leaf: String,
    /// The container's bytes.
    bytes: Vec<u8>,
    /// The author's permission bits, carried onto the tempfile so the mode
    /// survives the rename.
    mode: u32,
    /// Where the no-follow walk came out, relative to the destination: the
    /// action key unless the walk followed an owned link to reach the
    /// container.
    landing: Utf8PathBuf,
}

/// Executes an [`Action::Write`] whose entry is a block: splices the region
/// into a container that already exists, leaving every byte outside it
/// exactly where it was.
///
/// The plan says no region was there, and this read is what settles what is
/// there now:
///
/// - no marker occurrence — splice, which is the ordinary case;
/// - a region already carrying the desired body — adopt it: record the path
///   and report [`Skipped`](ApplyOutcome::Skipped), writing nothing, rather
///   than refuse a destination that is already in the desired state;
/// - a region carrying anything else — [`Error::Drift`] where the manifest
///   records the path (the region changed under the plan) and
///   [`Error::Foreign`] where it does not (bytes the projection never wrote);
/// - the marker on more than one whole line — the same refusal. Such a
///   container identifies no region at all ([`EntryKind::Block`]), so the
///   extreme occurrence's body matching what this run would write is not
///   evidence the projection wrote it. Adopting there would record a region
///   the recorded marker cannot locate again, which every run after it
///   refuses, and splicing would add a third occurrence;
/// - no container — [`Error::Block`] carrying
///   [`ContainerMissing`](BlockFault::ContainerMissing): a block never
///   creates its container, which is what keeps the projection from owning
///   one whole.
///
/// The container this read opened must be the one the action names: a walk
/// that reached it through an owned link refuses as [`Error::Containment`]
/// (`at_action_key`) rather than splice a region the manifest would then
/// record at another path.
///
/// A recorded path is asked one question first, under the *recorded* marker
/// rather than the entry's: the plan reached a write by finding the recorded
/// region gone, so a region back under that marker is a change since the
/// plan, refused as [`Error::Drift`] exactly as a node appearing at an
/// ordinary write's path is ([`write`]). That holds whether or not the caller
/// is also changing the marker; where they are, splicing under the new marker
/// would additionally leave the old region standing with nothing recording it
/// — one stranded body per marker change, which is the growth the marker
/// exists to prevent.
///
/// Adoption is therefore an unrecorded path's alone: a recorded path's
/// expectation is that no region is here, and apply holds the disk to the
/// plan's expectations rather than to the outcome it prefers.
fn write_block(
    dest: &Dir,
    manifest: &Manifest,
    path: &Utf8Path,
    entry: &Entry,
) -> Result<ApplyOutcome> {
    let Entry::Block {
        body,
        marker,
        placement,
    } = entry
    else {
        unreachable!("dispatched on a block entry");
    };
    let Some(container) = read_block_container(dest, manifest, path)? else {
        return Err(block_refusal(path, BlockFault::ContainerMissing));
    };
    at_action_key(path, &container.landing)?;
    if let Some((was_marker, was_placement)) = manifest
        .entries
        .get(path)
        .and_then(|recorded| block::block_kind(&recorded.kind))
    {
        // The recorded region was gone at plan time; one back under the
        // recorded marker is a change since the plan.
        if block::locate(&container.bytes, was_marker, was_placement).is_some() {
            return Err(drift(path));
        }
    }
    let unidentified = || {
        if manifest.entries.contains_key(path) {
            drift(path)
        } else {
            Error::Foreign {
                paths: BTreeSet::from([path.to_owned()]),
            }
        }
    };
    if block::occurrence_count(&container.bytes, marker) > 1 {
        return Err(unidentified());
    }
    if let Some(region) = block::locate(&container.bytes, marker, *placement) {
        if &container.bytes[region.body] == body.as_slice() {
            return Ok(ApplyOutcome::Skipped);
        }
        return Err(unidentified());
    }
    if *placement == Placement::Append && !block::newline_terminated(&container.bytes) {
        return Err(block_refusal(
            path,
            BlockFault::ContainerNotNewlineTerminated,
        ));
    }
    let spliced = block::splice(&container.bytes, marker, *placement, body);
    persist_mode(
        &container.parent,
        &container.leaf,
        path,
        &spliced,
        container.mode,
    )?;
    Ok(ApplyOutcome::Written)
}

/// Executes an [`Action::Overwrite`] whose entry is a block: one read of the
/// container both re-checks the recorded region against `expected` and
/// supplies the bytes the new region is spliced into, so nothing can be
/// substituted between the check and the write.
///
/// The old region is located with `expected`'s marker and placement and the
/// new one written with the entry's, so a caller who changed either migrates
/// the region in this single publish. Like every other write, it happens at
/// the action key or not at all (`at_action_key`).
fn overwrite_block(
    dest: &Dir,
    manifest: &Manifest,
    path: &Utf8Path,
    entry: &Entry,
    expected: &NodeSignature,
) -> Result<()> {
    let Entry::Block {
        body,
        marker,
        placement,
    } = entry
    else {
        unreachable!("dispatched on a block entry");
    };
    let Some((recorded_marker, recorded_placement)) = block::block_kind(&expected.kind) else {
        unreachable!("validate pairs a block entry with a block signature");
    };
    let Some(container) = read_block_container(dest, manifest, path)? else {
        return Err(drift(path));
    };
    at_action_key(path, &container.landing)?;
    // The recorded marker must still identify one region. A duplicate that
    // appeared since the plan leaves nothing saying which occurrence bounds
    // the recorded bytes, and the extreme one hashing to `expected` does not
    // settle it ([`EntryKind::Block`]).
    if block::occurrence_count(&container.bytes, recorded_marker) != 1 {
        return Err(drift(path));
    }
    let Some(region) = block::locate(&container.bytes, recorded_marker, recorded_placement) else {
        unreachable!("one occurrence locates a region");
    };
    if sha256_hex(&container.bytes[region.body.clone()]) != expected.hash {
        return Err(drift(path));
    }
    let author = block::strip(&container.bytes, Some(&region));
    // A migration writes a marker the author's side has never had to be free
    // of. One already there would make the publish leave two occurrences of
    // the new marker, and the container would identify no region on the very
    // next run ([`EntryKind::Block`]) — a refusal the projection would have
    // written itself. Unreachable where the marker is unchanged: its one
    // occurrence bounded the region that was just stripped.
    if block::occurrence_count(&author, marker) > 0 {
        return Err(drift(path));
    }
    if *placement == Placement::Append && !block::newline_terminated(&author) {
        return Err(block_refusal(
            path,
            BlockFault::ContainerNotNewlineTerminated,
        ));
    }
    let spliced = block::splice(&author, marker, *placement, body);
    persist_mode(
        &container.parent,
        &container.leaf,
        path,
        &spliced,
        container.mode,
    )
}

/// Executes an [`Action::Remove`] whose record is a block: strips the region
/// and its marker out and republishes the container, which stays even when
/// the strip empties it. The same one read re-checks the region against
/// `expected` and supplies the bytes.
///
/// Expecting `None` — the region was already gone at plan time — verifies
/// that none has appeared and writes nothing; a region found there is a
/// change since the plan, refused as [`Error::Drift`] exactly as a node
/// appearing at a removed path is. A container that is gone leaves nothing to
/// strip, which is a removal already done rather than a failure to do one.
///
/// A container the marker occupies more than one whole line of is refused
/// under either expectation: it identifies no region ([`EntryKind::Block`]),
/// and stripping an extreme occurrence there would take a range nobody can
/// say is the recorded one while the manifest entry goes away regardless.
fn remove_block(
    dest: &Dir,
    manifest: &Manifest,
    path: &Utf8Path,
    expected: Option<&NodeSignature>,
) -> Result<()> {
    let recorded = manifest
        .entries
        .get(path)
        .expect("validate refuses a removal the manifest does not record");
    let kind = expected.map_or(&recorded.kind, |expected| &expected.kind);
    let Some((marker, placement)) = block::block_kind(kind) else {
        unreachable!("dispatched on a block record, and validate pairs it with a block signature");
    };
    let Some(container) = read_block_container(dest, manifest, path)? else {
        return if expected.is_some() {
            Err(drift(path))
        } else {
            Ok(())
        };
    };
    // A container the marker no longer identifies one region in is refused
    // whichever expectation the plan carries: stripping an extreme occurrence
    // there would take a range nobody can say is the recorded one and leave
    // the other standing with the manifest entry gone
    // ([`EntryKind::Block`]).
    if block::occurrence_count(&container.bytes, marker) > 1 {
        return Err(drift(path));
    }
    match (expected, block::locate(&container.bytes, marker, placement)) {
        (Some(expected), Some(region)) => {
            if sha256_hex(&container.bytes[region.body.clone()]) != expected.hash {
                return Err(drift(path));
            }
            let author = block::strip(&container.bytes, Some(&region));
            persist_mode(
                &container.parent,
                &container.leaf,
                path,
                &author,
                container.mode,
            )
        }
        (Some(_), None) | (None, Some(_)) => Err(drift(path)),
        (None, None) => Ok(()),
    }
}

/// [`Error::Block`] over one path and one fault.
fn block_refusal(path: &Utf8Path, fault: BlockFault) -> Error {
    Error::Block {
        blocks: BTreeMap::from([(path.to_owned(), fault)]),
    }
}

/// The changed-since-plan re-check for a symlink's target: grades `target`
/// from `parent` — the link's own parent, relative to the destination and
/// after any owned-link restarts, since that is the directory the link is
/// published in — against the destination as it stands right now.
///
/// The verdict this holds the destination to is the deciding stage's:
/// grading resolves through the destination's own links
/// (`docs/security.lex` section 3), so a `pivot` swapped for a link to
/// `/etc` between plan and apply turns a pointer the plan graded in-dest
/// into one that escapes. Every other destructive action re-checks its
/// expectation before it proceeds ([`check_expected`]); this is that
/// re-check for the one part of a link the plan cannot express as a
/// [`NodeSignature`] — where its pointer lands.
///
/// `true` means the link may be published: the chain lands inside the
/// destination *and* it walked through no path in `unpublished`, the paths
/// this run is still going to publish a link at. Both halves are needed —
/// the first because a pivot swapped since the plan must not be pointed
/// through, the second because a path the run will still change is a
/// landing that can move after the fact. So `false` does not mean external
/// on its own, which is why [`settle_links`] holds the link and asks again
/// rather than refusing on the spot.
///
/// [`ExternalTargetPolicy::Allow`] answers `true` outright: the caller
/// permitted pointers out of the destination before this plan existed, so
/// there is no plan-time verdict for the destination to have invalidated,
/// and the target lands verbatim as it always would have.
fn link_settles(
    dest: &Dir,
    plan: &Plan,
    unpublished: &BTreeSet<Utf8PathBuf>,
    parent: &Utf8Path,
    target: &str,
) -> Result<bool> {
    if plan.external_targets == ExternalTargetPolicy::Allow {
        return Ok(true);
    }
    let mut waiting = false;
    let landing = contained_target_chain(parent, target, |hop| {
        waiting |= unpublished.contains(hop);
        hop_on_disk(dest, hop)
    })?;
    Ok(!waiting && landing.is_some())
}

/// What stands at one destination-relative path on disk, read for
/// [`link_settles`]'s chain resolution: each ancestor component is
/// opened from the `dest` handle with `open_dir_nofollow`, so the lstat of
/// the final component is reached without following anything.
///
/// Only a symlink continues a chain. A missing path, or one whose ancestry
/// is missing or is anything but a directory, is [`Terminal`](Hop::Terminal)
/// — a pointer into nothing stays a pointer inside the destination. Two
/// shapes are [`Unresolvable`](Hop::Unresolvable), and both grade the chain
/// external: a target that is not UTF-8, which nothing can say the landing
/// of, and an ancestor that is a symlink — the chain established that
/// ancestor as an ordinary directory a moment ago, so meeting a link there
/// means the destination is being rewritten underneath this run, and a chain
/// resolved through it would vouch for nothing.
fn hop_on_disk(dest: &Dir, path: &Utf8Path) -> Result<Hop> {
    let mut components: VecDeque<String> = path
        .components()
        .map(|component| component.as_str().to_owned())
        .collect();
    let leaf = components
        .pop_back()
        .expect("chain resolution asks about non-empty paths");
    let mut dir = dest.try_clone().map_err(io_error(Utf8Path::new(".")))?;
    let mut prefix = Utf8PathBuf::new();
    for component in components {
        let here = prefix.join(&component);
        let meta = match dir.symlink_metadata(&component) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Hop::Terminal),
            Err(e) => return Err(io_error(&here)(e)),
        };
        if meta.file_type().is_symlink() {
            return Ok(Hop::Unresolvable);
        }
        if !meta.file_type().is_dir() {
            return Ok(Hop::Terminal);
        }
        dir = open_nofollow(&dir, &component).map_err(io_error(&here))?;
        prefix = here;
    }
    match dir.symlink_metadata(&leaf) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = dir
                .as_cap_std()
                .read_link_contents(&leaf)
                .map_err(io_error(path))?;
            let bytes = std::os::unix::ffi::OsStrExt::as_bytes(target.as_os_str());
            match std::str::from_utf8(bytes) {
                Ok(target) => Ok(Hop::Link(target.to_owned())),
                Err(_) => Ok(Hop::Unresolvable),
            }
        }
        Ok(_) => Ok(Hop::Terminal),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Hop::Terminal),
        Err(e) => Err(io_error(path)(e)),
    }
}

/// Publishes the symlink `leaf -> target` inside `dir` atomically: the link
/// is created under a temporary name in that same verified directory and
/// renamed over the leaf, so the path holds the old node or the finished
/// link and never nothing — and because rename replaces whatever the leaf
/// was, a file becoming a link and a link becoming a link both publish in
/// that one step. A rename that fails unlinks the temporary link: `dir` is
/// never littered.
///
/// `target` reaches disk verbatim, through the plain-`Dir` view's
/// `symlink_contents`, which writes any target string — an absolute one
/// included, where `symlink` would refuse
/// (`docs/implementation.lex` section 3). Grading happened twice by now —
/// in the deciding stage and again in [`link_settles`] — and
/// neither rewrites
/// what a link points at. Nothing is written *through* the link either: the
/// no-follow walk that verified `dir` never resolves an external target.
fn publish_link(dir: &Dir, leaf: &str, path: &Utf8Path, target: &str) -> Result<()> {
    let dir = dir.as_cap_std();
    let temp = create_temp_link(dir, path, target)?;
    match dir.rename(&temp, dir, leaf) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = dir.remove_file(&temp);
            Err(io_error(path)(e))
        }
    }
}

/// Creates the link under a temporary name in `dir` and returns that name.
///
/// The name never publishes over anything: `symlink_contents` creates
/// exclusively, so a name already taken fails with `EEXIST`. A few of those
/// are retried under a fresh name rather than failing the run — a name left
/// behind by a crashed run, or one a concurrent writer occupies, costs an
/// attempt instead of the projection — and the last attempt reports
/// whatever it hits, so an `EEXIST` that keeps recurring surfaces as the
/// [`Error::Io`] it is.
fn create_temp_link(
    dir: &cap_std::fs::Dir,
    path: &Utf8Path,
    target: &str,
) -> Result<std::path::PathBuf> {
    const RETRIES: u32 = 15;
    for _ in 0..RETRIES {
        let name = temp_link_name();
        match dir.symlink_contents(target, &name) {
            Ok(()) => return Ok(name),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io_error(path)(e)),
        }
    }
    let name = temp_link_name();
    dir.symlink_contents(target, &name)
        .map_err(io_error(path))?;
    Ok(name)
}

/// A name for the temporary link [`publish_link`] renames from: the process
/// id, a per-call counter, and the full nanosecond clock. Two links
/// published at once by this process differ in the counter; two processes
/// differ in the pid; a name left behind by a crashed run of a process
/// whose id was reused would take the same nanosecond to reproduce. Rare
/// rather than impossible, which is why [`create_temp_link`] retries.
fn temp_link_name() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!(
        ".proiectio-link-{}-{}-{nanos}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
    .into()
}

/// Records what a write or overwrite placed at `path`: the entry's
/// signature, under the union of the previously recorded owners and this
/// plan's — a recorded-but-missing path written again keeps every owner
/// that held it.
fn record(manifest: &mut Manifest, path: &Utf8Path, entry: &Entry, owner: &str) {
    let (hash, executable) = match entry {
        Entry::File {
            contents,
            executable,
        } => (sha256_hex(contents), *executable),
        Entry::Symlink { target } => (sha256_hex(target.as_bytes()), false),
        Entry::Block { body, .. } => (sha256_hex(body), false),
    };
    let mut owners = manifest
        .entries
        .get(path)
        .map(|recorded| recorded.owners.clone())
        .unwrap_or_default();
    owners.insert(owner.to_owned());
    manifest.entries.insert(
        path.to_owned(),
        ManifestEntry {
            kind: entry.kind(),
            hash,
            executable,
            owners,
        },
    );
}

/// The changed-since-plan re-check for an existing node: walks to the
/// verified parent and compares the leaf against `expected`. A vanished
/// ancestor or leaf, or any mismatch of kind, hash, or executable bit,
/// refuses as [`Error::Drift`] carrying `path`.
fn check_expected(
    dest: &Dir,
    manifest: &Manifest,
    path: &Utf8Path,
    expected: &NodeSignature,
) -> Result<()> {
    let Some((parent, leaf, _)) = verified_parent(dest, manifest, path, false)? else {
        return Err(drift(path));
    };
    check_leaf(&parent, &leaf, path, expected)
}

/// Compares the node at `leaf` (inside the verified `parent`) against
/// `expected` — kind, hash, executable bit — with lstat semantics; any
/// difference, absence included, is [`Error::Drift`] at `path`.
///
/// The block arm serves [`Action::Skip`] alone, which writes nothing: an
/// overwrite or a removal of a region re-checks it inside the splice's own
/// read instead, so no window separates its check from its write.
fn check_leaf(parent: &Dir, leaf: &str, path: &Utf8Path, expected: &NodeSignature) -> Result<()> {
    let meta = match parent.symlink_metadata(leaf) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(drift(path)),
        Err(e) => return Err(io_error(path)(e)),
    };
    match &expected.kind {
        EntryKind::File => {
            if !meta.file_type().is_file() {
                return Err(drift(path));
            }
            let executable = meta.mode() & 0o100 != 0;
            let file = parent.open(leaf).map_err(io_error(path))?;
            let hash = sha256_hex_of_reader(file).map_err(io_error(path))?;
            if hash != expected.hash || executable != expected.executable {
                return Err(drift(path));
            }
        }
        EntryKind::Symlink => {
            if !meta.file_type().is_symlink() {
                return Err(drift(path));
            }
            if link_target_hash(parent, leaf, path)? != expected.hash {
                return Err(drift(path));
            }
        }
        EntryKind::Block { marker, placement } => {
            let Container::File { bytes, .. } = read_container(parent, leaf, path)? else {
                return Err(drift(path));
            };
            // One occurrence, or the marker identifies no region to re-check
            // ([`EntryKind::Block`]).
            if block::occurrence_count(&bytes, marker) != 1 {
                return Err(drift(path));
            }
            let Some(region) = block::locate(&bytes, marker, *placement) else {
                unreachable!("one occurrence locates a region");
            };
            if sha256_hex(&bytes[region.body]) != expected.hash {
                return Err(drift(path));
            }
        }
    }
    Ok(())
}

/// [`sha256_hex`] of the raw on-disk target of the link at `leaf`, read
/// through the plain-`Dir` view so a target edited to non-UTF-8 bytes
/// still hashes (and then matches no recorded hash) instead of failing.
fn link_target_hash(parent: &Dir, leaf: &str, path: &Utf8Path) -> Result<String> {
    let target = parent
        .as_cap_std()
        .read_link_contents(leaf)
        .map_err(io_error(path))?;
    let bytes = std::os::unix::ffi::OsStrExt::as_bytes(target.as_os_str());
    Ok(sha256_hex(bytes))
}

/// The no-follow ancestor walk (`docs/implementation.lex` section 3): opens
/// each ancestor component of `path` from the previously verified handle
/// with cap-primitives' `open_dir_nofollow` and returns the verified parent
/// handle, the leaf name, and the resolved parent path — the walked prefix
/// after any owned-link restarts, so a caller that mutates the leaf knows
/// the directory that actually changed — so the caller's final mutation
/// cannot be redirected by a component swapped for a symlink after its
/// check.
///
/// A symlink met on the way is judged by the four arms [`apply`]'s rustdoc
/// lists: unrecorded, external-target, or cyclic links refuse as
/// [`Error::Containment`] carrying the action's `path`; a recorded link
/// whose on-disk target no longer hashes to the recorded string refuses as
/// [`Error::Drift`] carrying the link's path; a recorded, matching, in-dest
/// link is followed by resolving its target lexically from the link's
/// parent and restarting the walk from the `dest` root, with a per-walk
/// visited set refusing cycles.
///
/// A creating walk also stops at [`MAX_WALK_DEPTH`] directories below
/// `dest`, failing as [`Error::DestinationTooDeep`]: the depth is measured
/// on the walked prefix, so a restart through an owned link is measured
/// where it landed rather than where the key was spelled.
///
/// `create` says what a missing ancestor means: a creating walk (a write
/// placing a file) creates the directory and continues — so it always
/// returns a parent — while a non-creating walk (re-checks, removals,
/// prunes) returns `None`, "the node's ancestry is gone", for the caller to
/// judge. A non-directory, non-symlink ancestor blocks a creating walk —
/// [`Error::Drift`] when that ancestor is recorded, [`Error::Foreign`] when
/// not: a foreign node is never displaced — and reads as gone ancestry for
/// a non-creating one.
fn verified_parent(
    dest: &Dir,
    manifest: &Manifest,
    path: &Utf8Path,
    create: bool,
) -> Result<Option<(Dir, String, Utf8PathBuf)>> {
    let mut components: VecDeque<String> = path
        .components()
        .map(|component| component.as_str().to_owned())
        .collect();
    let leaf = components
        .pop_back()
        .expect("validated action paths have a final component");
    let mut dir = dest.try_clone().map_err(io_error(Utf8Path::new(".")))?;
    let mut prefix = Utf8PathBuf::new();
    let mut visited: BTreeSet<Utf8PathBuf> = BTreeSet::new();
    while let Some(component) = components.pop_front() {
        let here = prefix.join(&component);
        // The depth being walked, which is not the depth `path` spells: an
        // owned-link restart replaces the prefix with the link's resolved
        // target, so a short key can arrive at a deep directory (and a long
        // one at a shallow directory). [`observe`](crate::observe) descends
        // `MAX_WALK_DEPTH` directories and no further, so a creating walk
        // stops rather than put a node where the next observation cannot
        // reach it. Non-creating walks — removals, re-checks, prunes — add
        // nothing and are the way back from a destination already too deep,
        // so they walk on.
        if create && here.components().count() > MAX_WALK_DEPTH {
            return Err(Error::DestinationTooDeep {
                path: here,
                limit: MAX_WALK_DEPTH,
            });
        }
        let meta = match dir.symlink_metadata(&component) {
            Ok(meta) => Some(meta),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(io_error(&here)(e)),
        };
        match meta {
            None => {
                if !create {
                    return Ok(None);
                }
                match dir.create_dir(&component) {
                    Ok(()) => {}
                    // Lost a benign race with a concurrent creator: re-judge
                    // what appeared through the ordinary arms, so a raced
                    // symlink or non-directory gets the same verdict as one
                    // that was there all along, not a bare open failure.
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        components.push_front(component);
                        continue;
                    }
                    Err(e) => return Err(io_error(&here)(e)),
                }
                dir = open_nofollow(&dir, &component).map_err(io_error(&here))?;
            }
            Some(meta) if meta.file_type().is_symlink() => {
                let recorded = manifest
                    .entries
                    .get(&here)
                    .filter(|recorded| recorded.kind == EntryKind::Symlink);
                let Some(recorded) = recorded else {
                    // Arm one: a link the projection does not own is never
                    // written through, wherever it points.
                    return Err(containment(path));
                };
                let target = dir
                    .as_cap_std()
                    .read_link_contents(&component)
                    .map_err(io_error(&here))?;
                let bytes = std::os::unix::ffi::OsStrExt::as_bytes(target.as_os_str());
                if sha256_hex(bytes) != recorded.hash {
                    // Arm two: recorded, but the target changed on disk —
                    // the same drift refusal every stale plan gets.
                    return Err(drift(&here));
                }
                let Ok(target) = std::str::from_utf8(bytes) else {
                    // A matching hash proves agreement with the record, not
                    // UTF-8: a manifest this crate never writes can record
                    // the hash of raw bytes. What cannot be graded is never
                    // followed.
                    return Err(containment(path));
                };
                // Arm three: grade the target from the link's parent, one
                // hop and lexically — this walk is itself the chain
                // resolution deciding runs over the observations, following
                // each in-dest hop against the live disk under the same
                // visited set — and only an in-dest resolution is followed.
                let Some(resolved) = contained_target(&prefix, target) else {
                    return Err(containment(path));
                };
                if !visited.insert(here) {
                    // An owned-link cycle: refuse rather than loop.
                    return Err(containment(path));
                }
                // Arm four: restart from the dest root along the resolved
                // directory path, the remaining components after it.
                let mut restarted: VecDeque<String> = resolved
                    .components()
                    .map(|component| component.as_str().to_owned())
                    .collect();
                restarted.append(&mut components);
                components = restarted;
                dir = dest.try_clone().map_err(io_error(Utf8Path::new(".")))?;
                prefix = Utf8PathBuf::new();
                continue;
            }
            Some(meta) if meta.file_type().is_dir() => {
                dir = open_nofollow(&dir, &component).map_err(io_error(&here))?;
            }
            Some(_) => {
                // A non-directory where the path needs a directory.
                if !create {
                    return Ok(None);
                }
                return Err(if manifest.entries.contains_key(&here) {
                    drift(&here)
                } else {
                    Error::Foreign {
                        paths: BTreeSet::from([here]),
                    }
                });
            }
        }
        prefix = here;
    }
    Ok(Some((dir, leaf, prefix)))
}

/// Opens the directory named `name` inside `dir` without following a final
/// symlink — cap-primitives' `open_dir_nofollow`, which is public there and
/// not on `Dir` itself. The start handle is a borrowed duplicate of `dir`'s
/// file descriptor; the opened handle comes back as a capability `Dir`.
fn open_nofollow(dir: &Dir, name: &str) -> std::io::Result<Dir> {
    let start = std::fs::File::from(dir.as_cap_std().as_fd().try_clone_to_owned()?);
    let opened = cap_primitives::fs::open_dir_nofollow(&start, std::path::Path::new(name))?;
    Ok(Dir::from_std_file(opened))
}

/// Holds a write to its action key: `landing` is where [`verified_parent`]'s
/// walk came out — the resolved parent joined with the leaf — and anywhere
/// but `path` refuses as [`Error::Containment`] carrying the key.
///
/// The walk follows a symlink the projection owns whose target resolves
/// in-dest, so a write can arrive somewhere the action does not name. Writing
/// there puts bytes at one location while the manifest records another, which
/// is the alias deciding's no-alias rule exists to prevent, and no later run
/// heals it: observation never descends a link, so the key classifies
/// [`Missing`](crate::PathState::Missing), the write is planned again, and
/// deciding then refuses it under that same rule — the path is neither
/// writable nor removable until someone edits the destination by hand.
/// Deciding plans no write beneath a link that outlives the plan, so a walk
/// that relocates one means the destination moved between the two calls.
///
/// A symlink carries a second reason: [`settle_links`] names the paths this
/// run will still put a link at by their action keys, and a chain that walks
/// one of them waits for it. A link going down anywhere else is one no chain
/// waits for, so a landing already vouched for could still move.
///
/// Removals are held to no key. They travel through an owned link on purpose
/// — [`remove`] reports the resolved location so pruning judges the directory
/// that lost a child — and they leave no record pointing at what they
/// unlinked (`docs/implementation.lex` section 3).
fn at_action_key(path: &Utf8Path, landing: &Utf8Path) -> Result<()> {
    if landing == path {
        Ok(())
    } else {
        Err(containment(path))
    }
}

/// [`Error::Drift`] at one path.
fn drift(path: &Utf8Path) -> Error {
    Error::Drift {
        paths: BTreeSet::from([path.to_owned()]),
    }
}

/// [`Error::Containment`] at one path.
fn containment(path: &Utf8Path) -> Error {
    Error::Containment {
        paths: BTreeSet::from([path.to_owned()]),
        origin: Origin::Caller,
    }
}

#[cfg(test)]
#[path = "act_tests.rs"]
mod tests;
