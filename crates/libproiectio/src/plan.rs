use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

use crate::report::recorded_shape;
use crate::{
    Dropped, Entry, EntryKind, Manifest, ManifestEntry, Origin, PathFacts, PathShape, Refusal,
    Refused, Report, Row,
};

/// What planning does when a recorded path's state on disk differs from the
/// recorded entry — bytes, kind, or executable bit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum DriftPolicy {
    /// Refuse and name the path (the default).
    #[default]
    Refuse,
    /// Overwrite the edit (a CLI's `--force`).
    Overwrite,
}

/// What planning does with a desired symlink whose target resolves outside
/// the destination.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum ExternalTargetPolicy {
    /// Refuse the link and name it with its target (the default).
    #[default]
    Refuse,
    /// Write the link, target verbatim (a CLI's `--allow-external-targets`).
    Allow,
}

/// The policy inputs one [planning](crate::Projection::plan) call runs under.
///
/// Both fields default to refusing:
///
/// ```
/// # use libproiectio::{DriftPolicy, ExternalTargetPolicy, PlanOptions};
/// let forced = PlanOptions {
///     drift: DriftPolicy::Overwrite,
///     ..PlanOptions::default()
/// };
/// assert_eq!(forced.external_targets, ExternalTargetPolicy::Refuse);
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PlanOptions {
    /// What to do with a recorded path edited on disk.
    pub drift: DriftPolicy,
    /// What to do with a desired symlink whose target grades external.
    pub external_targets: ExternalTargetPolicy,
}

/// Every action apply would perform, keyed by path relative to the
/// destination.
///
/// A plan is a report, not a capability: only a [`Run`](crate::Run) executes
/// one, and only the one it decided itself. Applying re-checks containment,
/// the manifest, and each action's `expected` signature before touching
/// anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The owner the plan was computed for; applied entries are recorded
    /// under this name in the manifest.
    pub owner: String,
    /// Where each path came from, for the paths a source named. Absent means
    /// [`Origin::Caller`](crate::Origin::Caller).
    pub origins: BTreeMap<Utf8PathBuf, Origin>,
    /// Whether the caller permitted external symlink targets when this plan
    /// was decided; apply re-grades each target against this.
    pub external_targets: ExternalTargetPolicy,
    /// The per-path actions.
    pub actions: BTreeMap<Utf8PathBuf, Action>,
    /// Archive members `strip` erased on the way to the desired tree, which
    /// no action can state: they reach no path in the destination, so there
    /// is nothing to do about them.
    pub dropped: BTreeSet<Dropped>,
}

impl Plan {
    /// Which source named `path`; [`Origin::Caller`] for one no source did.
    pub fn origin_of(&self, path: &Utf8Path) -> Origin {
        self.origins.get(path).cloned().unwrap_or_default()
    }

    /// Every refused path with its reason and the source that named it.
    pub fn refusals(&self) -> impl Iterator<Item = (&Utf8Path, &Refusal, Origin)> {
        self.actions
            .iter()
            .filter_map(|(path, action)| match action {
                Action::Refuse { refusal } => Some((path.as_path(), refusal, self.origin_of(path))),
                _ => None,
            })
    }

    pub fn report(&self, manifest: &Manifest) -> Report<PlannedAction> {
        Report {
            rows: self
                .actions
                .iter()
                .map(|(path, action)| {
                    let row = Row {
                        facts: facts_of(action, self.origin_of(path), manifest.entries.get(path)),
                        verdict: verdict_of(action),
                    };
                    (path.clone(), row)
                })
                .collect(),
        }
    }

    pub fn refused(&self) -> Option<Refused> {
        Refused::aggregate(
            self.refusals()
                .map(|(path, refusal, origin)| (path.to_owned(), refusal.clone(), origin)),
        )
    }
}

fn facts_of(
    action: &Action,
    origin: Origin,
    recorded: Option<&ManifestEntry>,
) -> Option<PathFacts> {
    let shape = match action {
        Action::Write { entry }
        | Action::Overwrite { entry, .. }
        | Action::OverwriteDirectory { entry }
        | Action::Skip { entry, .. } => Some(match entry {
            Entry::File { executable, .. } => PathShape::File {
                executable: *executable,
            },
            Entry::Symlink { target } => PathShape::Symlink {
                target: Some(target.clone()),
            },
            Entry::Block { .. } => PathShape::Block,
        }),
        Action::Remove {
            expected: Some(expected),
        } => Some(recorded_shape(&expected.kind, expected.executable)),
        // None of these four names a node of its own, so each row states what
        // the manifest records at the path — the shape and the owners,
        // including the owner a release drops, the other owner a path this one
        // does not hold turns out to have, and the file a directory drifted
        // over. Apply's row for the same path draws on the same entry, so a
        // dry run and a real run state alike.
        Action::Release
        | Action::NotRecorded
        | Action::RemoveDirectory
        | Action::Remove { expected: None } => {
            let recorded = recorded?;
            Some(recorded_shape(&recorded.kind, recorded.executable))
        }
        // A refusal decides no node either, but no apply row follows it: a
        // plan holding one refuses whole. Its row states the source that
        // named the path, plus the owners already recorded there.
        Action::Refuse { .. } => None,
    };
    Some(PathFacts {
        shape,
        owners: recorded
            .map(|recorded| recorded.owners.clone())
            .unwrap_or_default(),
        origin: Some(origin),
    })
}

fn verdict_of(action: &Action) -> PlannedAction {
    match action {
        Action::Write { .. } => PlannedAction::Write,
        Action::Overwrite { reason, .. } => PlannedAction::Overwrite { reason: *reason },
        Action::OverwriteDirectory { .. } => PlannedAction::Overwrite {
            reason: OverwriteReason::ForcedDrift,
        },
        Action::Skip { .. } => PlannedAction::Skip,
        Action::Remove {
            expected: Some(_), ..
        }
        | Action::RemoveDirectory => PlannedAction::Remove,
        Action::Remove { expected: None } => PlannedAction::Forget,
        Action::Release => PlannedAction::Release,
        Action::NotRecorded => PlannedAction::NotRecorded,
        Action::Refuse { refusal } => PlannedAction::Refuse {
            refusal: refusal.clone(),
        },
    }
}

/// Whether `action` vacates the whole node standing where it acts: the two
/// unlinking removals, and the removal half of [`Action::OverwriteDirectory`].
/// A removal expecting nothing verifies absence, a release, a skip and a
/// not-recorded leave the disk alone, and a refusal does nothing — none of
/// them take the node. This is the one reading of "the plan claims this
/// location" both stages grade a landing by.
pub(crate) fn vacates_node(action: &Action) -> bool {
    matches!(
        action,
        Action::Remove { expected: Some(_) }
            | Action::RemoveDirectory
            | Action::OverwriteDirectory { .. }
    )
}

/// One planned per-path action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Create a path that is not on disk: one never recorded, or one recorded
    /// but gone that the desired tree still wants. For an [`Entry::Block`]
    /// entry, "on disk" means the region, not the container.
    Write {
        /// What to write.
        entry: Entry,
    },
    /// Replace a recorded path.
    Overwrite {
        /// What to write.
        entry: Entry,
        /// The node the disk must still hold at apply time: the recorded
        /// signature, or the drifted node observed at plan time when
        /// [`DriftPolicy::Overwrite`] lifted a drift refusal.
        expected: NodeSignature,
        reason: OverwriteReason,
    },
    /// Disk already equals desired: nothing is written, the mtime survives,
    /// and apply records the signature under this plan's owner.
    Skip {
        /// The desired node, which the disk already holds.
        entry: Entry,
        /// The desired node's signature, which the disk carries at plan time
        /// and must still carry at apply time.
        expected: NodeSignature,
    },
    /// Remove an orphan — recorded under this owner alone and absent from the
    /// desired tree. Directories emptied by removal are pruned.
    Remove {
        /// The node the disk must still hold at apply time. `None` for a path
        /// already gone at plan time, which apply drops from the manifest
        /// without unlinking anything — pruning the directories the absent
        /// path leaves empty — and refuses if a node has appeared since.
        expected: Option<NodeSignature>,
    },
    /// Replace the empty directory a recorded path drifted into. No
    /// [`NodeSignature`] names a directory, so apply re-checks this one by
    /// removing it, and anything but an empty directory there refuses as
    /// [`Refusal::Drift`]. Planned only under [`DriftPolicy::Overwrite`], and
    /// never over a directory holding anything, which is not the projection's
    /// to unlink.
    OverwriteDirectory {
        /// What to write where the directory stood.
        entry: Entry,
    },
    /// Drop a recorded path whose node drifted into an empty directory,
    /// removing that directory. Re-checked and planned on the same terms as
    /// [`OverwriteDirectory`](Action::OverwriteDirectory); the directories
    /// above it are pruned as any removal's are.
    RemoveDirectory,
    /// Drop this owner from the path's manifest entry and leave the disk
    /// alone: other owners still hold it. Apply re-checks nothing on disk.
    Release,
    /// The removal named this path and the owner does not hold it — nothing
    /// records it, or another owner alone does. Nothing is written and no
    /// record changes; the row says the path was named and not held.
    NotRecorded,
    /// The path is named and left untouched. Applying a plan containing
    /// refusals fails with [`Error::Refused`](crate::Error::Refused).
    Refuse {
        /// Why the path is refused.
        refusal: Refusal,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum OverwriteReason {
    ContentChanged,
    ExecutableChanged,
    ForcedDrift,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum PlannedAction {
    Write,
    Overwrite {
        reason: OverwriteReason,
    },
    Skip,
    Remove,
    /// Drop the record of a path nothing stands at; nothing is unlinked.
    Forget,
    Release,
    /// The path was named by the removal and this owner does not hold it.
    NotRecorded,
    Refuse {
        refusal: Refusal,
    },
}

/// The on-disk node an action expects at apply time. Apply re-checks all
/// three fields and refuses if any changed since the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeSignature {
    /// The node's kind. For a [`Block`](EntryKind::Block) it carries the
    /// marker and placement the region is located with.
    pub kind: EntryKind,
    /// [`sha256_hex`](crate::sha256_hex) of the node — file contents, the
    /// link target string, or a region's body.
    pub hash: String,
    /// The executable bit; always `false` for symlinks and blocks.
    pub executable: bool,
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
