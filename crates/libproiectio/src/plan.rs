use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

use crate::{Entry, EntryKind, Origin, Refusal};

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
}

/// One planned per-path action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
        /// The desired node's signature, which the disk carries at plan time
        /// and must still carry at apply time.
        expected: NodeSignature,
    },
    /// Remove an orphan — recorded under this owner alone and absent from the
    /// desired tree. Directories emptied by removal are pruned.
    Remove {
        /// The node the disk must still hold at apply time. `None` for a path
        /// already gone at plan time, which apply drops from the manifest
        /// alone and refuses if a node has appeared since.
        expected: Option<NodeSignature>,
    },
    /// Drop this owner from the path's manifest entry and leave the disk
    /// alone: other owners still hold it. Apply re-checks nothing on disk.
    Release,
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

/// The on-disk node an action expects at apply time. Apply re-checks all
/// three fields and refuses if any changed since the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
