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

use crate::containment::contained_normalize;
use crate::observe::{io_error, sha256_hex_of_reader};
use crate::{
    Action, ApplyOutcome, ApplyReport, Entry, EntryKind, Error, MANIFEST_FILE_NAME,
    MANIFEST_VERSION, Manifest, ManifestEntry, NodeSignature, Plan, Refusal, Result, sha256_hex,
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
pub fn load_manifest(state: &Dir) -> Result<Manifest> {
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
pub fn save_manifest(state: &Dir, manifest: &Manifest) -> Result<()> {
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
/// function cannot see where `state` lives relative to `dest`.)
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
///   [`Error::OwnerConflict`], then [`Error::ExternalTarget`];
/// - a plan writing a symlink fails as [`Error::ApplySymlinkUnimplemented`]
///   (creation waits on target grading — issue #8), and one touching a
///   [`Block`](EntryKind::Block) entry as [`Error::ApplyBlockUnimplemented`]
///   (issue #14). Removing or releasing a *recorded* symlink is
///   implemented.
///
/// # Execution
///
/// Actions run deterministically (`docs/implementation.lex` section 6):
/// removals first, in reverse sorted order — children before parents — with
/// directories emptied by removal pruned afterwards (deepest first; a
/// directory still holding anything, a non-UTF-8 name included, is kept,
/// never an error); then everything else in sorted order, parents before
/// children, creating missing parent directories on the way.
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
/// File bytes go through a tempfile created inside the verified parent and
/// renamed over the path, with permissions (the exec bit included) set on
/// the open tempfile handle before the rename — a crash leaves the old
/// file or the new one, never a torn write and never a visible file with a
/// wrong mode. Before every overwrite, removal, and skip the target is
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
pub fn apply(dest: &Dir, state: &Dir, manifest: &Manifest, plan: &Plan) -> Result<ApplyReport> {
    validate(manifest, plan)?;
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
    let mut symlink_unimplemented = BTreeSet::new();
    let mut block_unimplemented = BTreeSet::new();
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
            }
            continue;
        }
        // Every other action mutates disk or manifest at its key, so the
        // key must already be in the gateway's normalized form: a plan is
        // plain data, and a hand-built `../escape` or `a/../b` key must
        // refuse here, not resolve.
        match contained_normalize(path) {
            Ok(normalized) if normalized == *path => {}
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
        match action {
            Action::Write { entry } => entry_seam(
                entry,
                path,
                &mut symlink_unimplemented,
                &mut block_unimplemented,
            ),
            Action::Overwrite { entry, expected } => {
                entry_seam(
                    entry,
                    path,
                    &mut symlink_unimplemented,
                    &mut block_unimplemented,
                );
                // The re-check side too: check_leaf cannot honor a block
                // signature, and finding that out mid-run would break the
                // up-front promise.
                if expected.kind == EntryKind::Block {
                    block_unimplemented.insert(path.clone());
                }
            }
            Action::Skip { expected }
            | Action::Remove {
                expected: Some(expected),
            } => {
                if expected.kind == EntryKind::Block {
                    block_unimplemented.insert(path.clone());
                }
            }
            Action::Remove { expected: None } | Action::Release => {}
            Action::Refuse { .. } => unreachable!("matched above"),
        }
    }
    if !containment.is_empty() {
        return Err(Error::Containment { paths: containment });
    }
    if !tree_conflict.is_empty() {
        return Err(Error::TreeConflict {
            paths: tree_conflict,
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
        return Err(Error::ExternalTarget { links: external });
    }
    if !block_unimplemented.is_empty() {
        return Err(Error::ApplyBlockUnimplemented {
            paths: block_unimplemented,
        });
    }
    if !symlink_unimplemented.is_empty() {
        return Err(Error::ApplySymlinkUnimplemented {
            paths: symlink_unimplemented,
        });
    }
    Ok(())
}

/// Sorts a written entry's kind into [`validate`]'s unimplemented-seam
/// sets: symlink and block creation wait on issues #8 and #14.
fn entry_seam(
    entry: &Entry,
    path: &Utf8Path,
    symlink_unimplemented: &mut BTreeSet<Utf8PathBuf>,
    block_unimplemented: &mut BTreeSet<Utf8PathBuf>,
) {
    match entry {
        Entry::File { .. } => {}
        Entry::Symlink { .. } => {
            symlink_unimplemented.insert(path.to_owned());
        }
        Entry::Block { .. } => {
            block_unimplemented.insert(path.to_owned());
        }
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
            // Only an actual disk removal can have emptied ancestors — and
            // the ancestors that lost a child are the *resolved* location's,
            // which differs from the action key's when the walk followed an
            // owned link.
            if let Some(resolved) = remove(dest, manifest, path, expected.as_ref())? {
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
    // Then everything else in sorted order, parents before children.
    for (path, action) in &plan.actions {
        match action {
            Action::Remove { .. } | Action::Refuse { .. } => {}
            Action::Write { entry } => {
                write(dest, manifest, path, entry, true)?;
                record(manifest, path, entry, &plan.owner);
                outcomes.insert(path.clone(), ApplyOutcome::Written);
            }
            Action::Overwrite { entry, expected } => {
                check_expected(dest, manifest, path, expected)?;
                write(dest, manifest, path, entry, false)?;
                record(manifest, path, entry, &plan.owner);
                outcomes.insert(path.clone(), ApplyOutcome::Overwritten);
            }
            Action::Skip { expected } => {
                check_expected(dest, manifest, path, expected)?;
                let mut owners = manifest
                    .entries
                    .get(path)
                    .map(|entry| entry.owners.clone())
                    .unwrap_or_default();
                owners.insert(plan.owner.clone());
                manifest.entries.insert(
                    path.clone(),
                    ManifestEntry {
                        kind: expected.kind,
                        hash: expected.hash.clone(),
                        executable: expected.executable,
                        owners,
                    },
                );
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
    Ok(())
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

/// Writes a planned entry at `path` through a tempfile in the verified
/// parent, renamed over the leaf. `fresh` marks an [`Action::Write`], whose
/// target must still be absent: a node found there refuses — [`Error::Drift`]
/// when the path is recorded (it changed relative to the plan's view),
/// [`Error::Foreign`] otherwise (something the projection never wrote
/// appeared). Symlink and block entries were rejected by [`validate`];
/// reaching one here is a bug, but still errors rather than panics.
fn write(
    dest: &Dir,
    manifest: &Manifest,
    path: &Utf8Path,
    entry: &Entry,
    fresh: bool,
) -> Result<()> {
    let Entry::File {
        contents,
        executable,
    } = entry
    else {
        let paths = BTreeSet::from([path.to_owned()]);
        return Err(match entry {
            Entry::Symlink { .. } => Error::ApplySymlinkUnimplemented { paths },
            _ => Error::ApplyBlockUnimplemented { paths },
        });
    };
    let Some((parent, leaf, _)) = verified_parent(dest, manifest, path, true)? else {
        unreachable!("a creating walk opens or creates every ancestor");
    };
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
    persist(&parent, &leaf, path, contents, *executable)
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
    let mut temp = TempFile::new(dir.as_cap_std()).map_err(io_error(path))?;
    temp.write_all(contents).map_err(io_error(path))?;
    let mode = if executable { 0o755 } else { 0o644 };
    let permissions =
        cap_std::fs::Permissions::from_std(std::os::unix::fs::PermissionsExt::from_mode(mode));
    temp.as_file()
        .set_permissions(permissions)
        .map_err(io_error(path))?;
    temp.replace(leaf).map_err(io_error(path))
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
        Entry::Block { body } => (sha256_hex(body), false),
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
/// difference, absence included, is [`Error::Drift`] at `path`. Block
/// signatures never reach here ([`validate`] rejects them); the arm errors
/// rather than panics all the same.
fn check_leaf(parent: &Dir, leaf: &str, path: &Utf8Path, expected: &NodeSignature) -> Result<()> {
    let meta = match parent.symlink_metadata(leaf) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(drift(path)),
        Err(e) => return Err(io_error(path)(e)),
    };
    match expected.kind {
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
        EntryKind::Block => {
            return Err(Error::ApplyBlockUnimplemented {
                paths: BTreeSet::from([path.to_owned()]),
            });
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
                    // Lost a benign race with a concurrent creator; the
                    // no-follow open below still verifies what is there.
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
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
                // Arm three: grade the target from the link's parent; only
                // an in-dest resolution may be followed. `join` on an
                // absolute target yields an absolute path, which the
                // gateway refuses like any escaping one.
                let Ok(resolved) = contained_normalize(&prefix.join(target)) else {
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
    }
}

#[cfg(test)]
#[path = "act_tests.rs"]
mod tests;
