//! The write stage: executes a [`Plan`] verbatim against the destination and
//! persists the [`Manifest`] into the state directory.
//!
//! All I/O goes through two capability handles: a `Dir` rooted at the
//! destination and a second `Dir` rooted at the caller-chosen state
//! directory, which holds the manifest. Unix-only.

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
    Origin, Placement, Plan, Refusal, Refused, Result, sha256_hex,
};

/// Loads the manifest from `state`'s [`MANIFEST_FILE_NAME`]; a state
/// directory holding no manifest file loads as the empty [`Manifest`].
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

/// Atomically persists `manifest` as `state`'s [`MANIFEST_FILE_NAME`],
/// mode `0o644`: tempfile inside the state directory, renamed over the path.
pub(crate) fn save_manifest(state: &Dir, manifest: &Manifest) -> Result<()> {
    let path = Utf8Path::new(MANIFEST_FILE_NAME);
    let mut json = serde_json::to_vec_pretty(manifest).map_err(|source| Error::ManifestFormat {
        path: path.to_owned(),
        source,
    })?;
    json.push(b'\n');
    persist(state, MANIFEST_FILE_NAME, path, &json, false)
}

/// Executes `plan` verbatim against `dest` and persists the updated
/// manifest into `state`, reporting what each action did. Nothing is
/// written when the plan cannot be honored whole ([`validate`]).
///
/// A block over a container the manifest does not record is the exception:
/// what that container holds is unknown until this stage reads it, so its
/// refusals arrive mid-run, after whatever the plan already applied.
pub(crate) fn apply(
    dest: &Dir,
    state: &Dir,
    manifest: &Manifest,
    plan: &Plan,
) -> Result<ApplyReport> {
    validate(manifest, plan)?;
    let mut manifest = manifest.clone();
    let mut outcomes = BTreeMap::new();
    match run(dest, &mut manifest, plan, &mut outcomes) {
        Ok(()) => {
            save_manifest(state, &manifest)?;
            Ok(ApplyReport { outcomes, manifest })
        }
        Err(error) => {
            if !outcomes.is_empty() {
                let _ = save_manifest(state, &manifest);
            }
            Err(error)
        }
    }
}

/// The up-front whole-plan check behind [`apply`]'s "nothing is written"
/// promise: every refusal in `plan` — the ones it carries and the ones a
/// forged plan would slip past — reduced by [`Refused::aggregate`] to one
/// error, with a too-deep destination reported only when nothing refused.
fn validate(manifest: &Manifest, plan: &Plan) -> Result<()> {
    let mut refused: Vec<(Utf8PathBuf, Refusal, Origin)> = plan
        .refusals()
        .map(|(path, refusal, origin)| (path.to_owned(), refusal.clone(), origin))
        .collect();
    let mut too_deep = None;
    for (path, action) in &plan.actions {
        let mut refuse =
            |refusal: Refusal| refused.push((path.clone(), refusal, plan.origin_of(path)));
        if matches!(action, Action::Refuse { .. }) {
            continue;
        }
        match contained_normalize(path) {
            Some(normalized) if normalized == *path => {}
            _ => {
                refuse(Refusal::Containment);
                continue;
            }
        }
        if !matches!(action, Action::Write { .. }) && !manifest.entries.contains_key(path) {
            refuse(Refusal::Foreign);
            continue;
        }
        let written = match action {
            Action::Write { entry } | Action::Overwrite { entry, .. } => Some(entry),
            _ => None,
        };
        if let Some(Entry::Symlink { target }) = written {
            if !is_pathname(target) {
                refuse(Refusal::InvalidTarget {
                    target: target.clone(),
                });
                continue;
            }
        }
        if too_deep.is_none() && path.components().count() - 1 > MAX_WALK_DEPTH {
            let mut offender = Vec::new();
            for component in path.components().take(MAX_WALK_DEPTH + 1) {
                offender.push(component.as_str());
            }
            too_deep = Some(Utf8PathBuf::from(offender.join("/")));
        }
        let recorded_kind = manifest.entries.get(path).map(|recorded| &recorded.kind);
        let record_is_block = recorded_kind.is_some_and(EntryKind::is_block);
        let mut block = |fault: BlockFault| refuse(Refusal::Block { fault });
        match action {
            Action::Write { entry } => {
                if let Some(fault) = entry_block_fault(entry) {
                    block(fault);
                }
                if manifest.entries.contains_key(path) && record_is_block != entry.kind().is_block()
                {
                    block(BlockFault::KindChange);
                }
            }
            Action::Overwrite { entry, expected } => {
                if let Some(fault) = entry_block_fault(entry) {
                    block(fault);
                }
                if record_is_block != entry.kind().is_block() {
                    block(BlockFault::KindChange);
                }
                if let Some(fault) = signature_block_fault(recorded_kind, &expected.kind) {
                    block(fault);
                }
            }
            Action::Skip { expected }
            | Action::Remove {
                expected: Some(expected),
            } => {
                if let Some(fault) = signature_block_fault(recorded_kind, &expected.kind) {
                    block(fault);
                }
            }
            Action::Remove { expected: None } | Action::Release => {}
            Action::Refuse { .. } => unreachable!("matched above"),
        }
    }
    if let Some(refused) = Refused::aggregate(refused) {
        return Err(refused.into());
    }
    if let Some(path) = too_deep {
        return Err(Error::DestinationTooDeep {
            path,
            limit: MAX_WALK_DEPTH,
        });
    }
    Ok(())
}

/// Whether an action's expected signature disagrees with the record at its
/// path about being a block, or names a region the record does not.
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

/// Executes a validated plan's actions, recording into `manifest` and
/// `outcomes` as each one lands, so a mid-run error leaves both holding
/// exactly what was applied.
fn run(
    dest: &Dir,
    manifest: &mut Manifest,
    plan: &Plan,
    outcomes: &mut BTreeMap<Utf8PathBuf, ApplyOutcome>,
) -> Result<()> {
    let mut removed_dirs_candidates: BTreeSet<Utf8PathBuf> = BTreeSet::new();
    for (path, action) in plan.actions.iter().rev() {
        if let Action::Remove { expected } = action {
            if manifest
                .entries
                .get(path)
                .is_some_and(|recorded| recorded.kind.is_block())
            {
                remove_block(dest, manifest, path, expected.as_ref())?;
            } else if let Some(resolved) = remove(dest, manifest, path, expected.as_ref())? {
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
                        manifest.entries.remove(path);
                    }
                }
                outcomes.insert(path.clone(), ApplyOutcome::Released);
            }
        }
    }
    settle_links(dest, manifest, plan, outcomes, links, unpublished)
}

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

/// Executes the plan's symlink actions last, holding each link until its
/// target grades in-dest and its resolution crosses no path the run is
/// still going to publish a link at.
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
        let mut escaping = Vec::new();
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
                    escaping.push((
                        path.clone(),
                        Refusal::ExternalTarget {
                            target: target.clone(),
                        },
                        plan.origin_of(path),
                    ));
                    held.push((path, action));
                }
            }
        }
        if held.len() == before {
            return Err(Refused::aggregate(escaping)
                .expect("a held link is an escaping one")
                .into());
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

/// Re-grades the on-disk target of a link an [`Action::Skip`] leaves in
/// place, holding the finished destination to the plan's in-dest verdict.
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
    Err(refuse(
        path,
        Refusal::ExternalTarget {
            target: target.to_owned(),
        },
    ))
}

/// Executes one [`Action::Remove`], returning the *resolved* location it
/// unlinked — the action key unless the walk followed an owned link — or
/// `None` where the plan expected nothing there. The entry leaves the
/// manifest either way.
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
                return Err(drift(path));
            };
            check_leaf(&parent, &leaf, path, expected)?;
            parent.remove_file(&leaf).map_err(io_error(path))?;
            Ok(Some(resolved_parent.join(leaf)))
        }
        None => {
            if let Some((parent, leaf, _)) = verified_parent(dest, manifest, path, false)? {
                match parent.symlink_metadata(&leaf) {
                    Ok(_) => return Err(drift(path)),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(io_error(path)(e)),
                }
            }
            Ok(None)
        }
    }
}

/// Prunes directories emptied by this run's removals, deepest first. A
/// directory still holding anything is kept, not an error; so is one
/// already gone or no longer a directory.
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

/// Publishes a planned entry over `path`'s leaf in one rename inside the
/// verified parent, and only where the walk came out at the action key.
/// `fresh` marks an [`Action::Write`], whose target must still be absent.
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
                    refuse(path, Refusal::Foreign)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Written {
    Published,
    /// A symlink whose target does not grade in-dest against the
    /// destination as it stands; nothing was written.
    Held,
}

/// Publishes `contents` at `leaf` inside `dir` atomically: tempfile, mode
/// set on the open handle, then rename over the path.
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

/// [`persist`] with the mode named outright, for a block's container, which
/// keeps the author's permission bits.
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
/// walk and reads it once. `None` means nothing stands at the path; a node
/// that is not a regular file refuses.
///
/// The run's guard does not cover the gap between this read and the rename
/// that republishes the container: a write by anything else in that window
/// is silently lost. The rename replaces the inode, so ownership, ACLs,
/// extended attributes and any other hard link to the file do not survive
/// it; the mode does.
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
            refuse(path, Refusal::Foreign)
        }),
    }
}

struct OpenContainer {
    parent: Dir,
    leaf: String,
    bytes: Vec<u8>,
    mode: u32,
    /// Where the no-follow walk came out, relative to the destination: the
    /// action key unless the walk followed an owned link.
    landing: Utf8PathBuf,
}

/// Executes an [`Action::Write`] whose entry is a block: splices the region
/// into a container that must already exist, leaving every byte outside the
/// region where it was. A region already carrying the desired body is
/// adopted; any other region there refuses.
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
        if block::locate(&container.bytes, was_marker, was_placement).is_some() {
            return Err(drift(path));
        }
    }
    let unidentified = || {
        if manifest.entries.contains_key(path) {
            drift(path)
        } else {
            refuse(path, Refusal::Foreign)
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
/// supplies the bytes the new region is spliced into. The old region is
/// located with `expected`'s marker and the new one written with the entry's.
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
    if block::occurrence_count(&author, marker) > 0 {
        return Err(block_refusal(path, BlockFault::MarkerInAuthorText));
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
/// the strip empties it. A container that is gone leaves nothing to strip.
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

fn block_refusal(path: &Utf8Path, fault: BlockFault) -> Error {
    refuse(path, Refusal::Block { fault })
}

/// Whether the symlink `target`, resolved from `parent` against the live
/// destination, lands inside it without walking a path in `unpublished`.
/// `false` is not a verdict of external on its own — [`settle_links`] holds
/// the link and asks again.
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

/// What stands at one destination-relative path on disk, for
/// [`link_settles`]'s chain resolution. Only a symlink continues a chain;
/// a non-UTF-8 target and a symlinked ancestor are [`Unresolvable`](Hop::Unresolvable).
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

/// Publishes the symlink `leaf -> target` inside `dir` atomically: created
/// under a temporary name in that same directory and renamed over the leaf.
/// `target` reaches disk verbatim, an absolute one included.
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
/// `symlink_contents` creates exclusively, so a name already taken fails
/// with `EEXIST` and is retried under a fresh one.
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

/// Records what a write or overwrite placed at `path`, under the union of
/// the previously recorded owners and this plan's.
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
/// verified parent and compares the leaf against `expected`.
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
/// difference, absence included, is [`Refusal::Drift`] at `path`.
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

/// [`sha256_hex`] of the raw on-disk target of the link at `leaf`, hashed as
/// bytes so a target edited to non-UTF-8 still hashes instead of failing.
fn link_target_hash(parent: &Dir, leaf: &str, path: &Utf8Path) -> Result<String> {
    let target = parent
        .as_cap_std()
        .read_link_contents(leaf)
        .map_err(io_error(path))?;
    let bytes = std::os::unix::ffi::OsStrExt::as_bytes(target.as_os_str());
    Ok(sha256_hex(bytes))
}

/// The no-follow ancestor walk: opens each ancestor component of `path` from
/// the previously verified handle and returns that parent handle, the leaf
/// name, and the walked prefix — which differs from `path`'s parent where the
/// walk followed an owned link. Without `create`, missing ancestry answers `None`.
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
                    // Lost a race with a concurrent creator: re-judge what
                    // appeared through the ordinary arms below.
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
                    return Err(containment(path));
                };
                let target = dir
                    .as_cap_std()
                    .read_link_contents(&component)
                    .map_err(io_error(&here))?;
                let bytes = std::os::unix::ffi::OsStrExt::as_bytes(target.as_os_str());
                if sha256_hex(bytes) != recorded.hash {
                    return Err(drift(&here));
                }
                let Ok(target) = std::str::from_utf8(bytes) else {
                    return Err(containment(path));
                };
                let Some(resolved) = contained_target(&prefix, target) else {
                    return Err(containment(path));
                };
                if !visited.insert(here) {
                    return Err(containment(path));
                }
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
                if !create {
                    return Ok(None);
                }
                return Err(if manifest.entries.contains_key(&here) {
                    drift(&here)
                } else {
                    refuse(&here, Refusal::Foreign)
                });
            }
        }
        prefix = here;
    }
    Ok(Some((dir, leaf, prefix)))
}

/// Opens the directory named `name` inside `dir` without following a final
/// symlink — cap-primitives' `open_dir_nofollow`, which is public there and
/// not on `Dir` itself.
fn open_nofollow(dir: &Dir, name: &str) -> std::io::Result<Dir> {
    let start = std::fs::File::from(dir.as_cap_std().as_fd().try_clone_to_owned()?);
    let opened = cap_primitives::fs::open_dir_nofollow(&start, std::path::Path::new(name))?;
    Ok(Dir::from_std_file(opened))
}

/// Holds a write to its action key: `landing` is where [`verified_parent`]'s
/// walk came out, and anywhere but `path` refuses as [`Refusal::Containment`].
/// Removals are held to no key.
fn at_action_key(path: &Utf8Path, landing: &Utf8Path) -> Result<()> {
    if landing == path {
        Ok(())
    } else {
        Err(containment(path))
    }
}

/// A refusal met mid-run, once [`validate`] has passed and the disk has
/// moved: a verdict on what the disk holds, which no source named.
fn refuse(path: &Utf8Path, refusal: Refusal) -> Error {
    Refused::one(path.to_owned(), refusal, Origin::Caller).into()
}

fn drift(path: &Utf8Path) -> Error {
    refuse(path, Refusal::Drift)
}

fn containment(path: &Utf8Path) -> Error {
    refuse(path, Refusal::Containment)
}

#[cfg(test)]
#[path = "act_tests.rs"]
mod tests;
