use std::collections::{BTreeMap, BTreeSet};

use camino::Utf8PathBuf;
use serde::Serialize;

use crate::{Entry, EntryKind};

/// What planning does when a recorded path's state on disk differs from
/// the recorded entry — bytes, kind, or executable bit — a user edit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum DriftPolicy {
    /// Refuse and name the path (the default; a CLI maps this refusal to
    /// exit 2).
    #[default]
    Refuse,
    /// Overwrite the edit (a CLI's `--force`).
    Overwrite,
}

/// Every action apply would perform, keyed by path relative to the
/// destination.
///
/// A plan is complete: apply's inputs are the projection and the plan —
/// never the desired tree — because each action carries what executing it
/// needs: the [`Entry`] to write, and for destructive actions the hash the
/// disk must still match. Plans are plain data, not capabilities: apply
/// re-validates containment, re-hashes targets, and re-checks the recorded
/// kind and executable bit against the manifest before touching anything,
/// so a hand-built or stale plan refuses rather than misfires. `BTreeMap`
/// keeps plans sorted, diffable, and deterministic; apply derives execution
/// order from it (parents before children, removals in reverse).
///
/// An empty desired tree plans a removal of everything the owner alone
/// holds and a release of everything it shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Plan {
    /// The owner the plan was computed for; applied entries are recorded
    /// under this name in the manifest.
    pub owner: String,
    /// The per-path actions.
    pub actions: BTreeMap<Utf8PathBuf, Action>,
}

/// One planned per-path action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Action {
    /// Create a path that is not on disk: one never recorded, or one
    /// recorded but gone ([`PathState::Missing`](crate::PathState)) that
    /// the desired tree still wants.
    ///
    /// For a [`Entry::Block`] entry, "on disk" means the projection's
    /// delimited region, not the container file: a pre-existing container
    /// without this projection's markers plans a `Write`, which inserts
    /// the region and leaves the rest of the file alone. Until block-region
    /// classification lands, the deciding stage does not yet produce that
    /// plan — it refuses the pre-existing container as foreign
    /// ([`decide`](crate::decide)'s rustdoc names the seam).
    Write {
        /// What to write.
        entry: Entry,
    },
    /// Replace a recorded path whose desired content changed. Apply
    /// re-hashes the target first and refuses if the disk no longer
    /// matches `expected_hash`.
    Overwrite {
        /// What to write.
        entry: Entry,
        /// The hash the disk must still carry at apply time: the recorded
        /// hash for a clean path, or — when [`DriftPolicy::Overwrite`]
        /// lifted a drift refusal — the hash of the drifted bytes observed
        /// at plan time.
        expected_hash: String,
    },
    /// Disk already equals desired: nothing is written and the mtime
    /// survives. Apply re-checks the whole signature — kind, hash, and
    /// executable bit — against the disk, refuses if any of it changed
    /// since the plan, and records the signature with this plan's owner on
    /// the path's manifest entry. That recording is how an owner joins a
    /// path another owner already holds identically, and how the manifest
    /// catches up when a drifted path was edited into agreement with the
    /// desired tree — the recorded entry may differ from this signature in
    /// any field, so the action carries all of them.
    Skip {
        /// The desired node's kind, which the disk node matches.
        kind: EntryKind,
        /// The hash of the desired node — its file contents or link
        /// target — which the disk carries at plan time and must still
        /// carry at apply time.
        expected_hash: String,
        /// The desired executable bit, which the disk node matches;
        /// always `false` for symlinks
        /// ([`ManifestEntry::executable`](crate::ManifestEntry::executable)).
        executable: bool,
    },
    /// Remove an orphan — recorded under this owner alone and absent from
    /// the desired tree. Apply re-hashes the target first and refuses if
    /// the disk no longer matches `expected_hash`; a path already gone
    /// from disk drops from the manifest alone. Directories emptied by
    /// removal are pruned.
    Remove {
        /// The hash the disk must still carry at apply time: the recorded
        /// hash, or — when [`DriftPolicy::Overwrite`] lifted a drift
        /// refusal — the hash of the drifted bytes observed at plan time.
        expected_hash: String,
    },
    /// Drop this owner from the path's manifest entry and leave the disk
    /// alone: the path is absent from this owner's desired tree, but other
    /// owners still hold it.
    Release,
    /// The path is named and left untouched. A plan containing refusals
    /// reports them all; applying it fails with the matching refusal
    /// variant of [`Error`](crate::Error).
    Refuse {
        /// Why the path is refused.
        refusal: Refusal,
    },
}

/// Why a planned path is refused rather than acted on.
///
/// Each value corresponds to one refusal variant of
/// [`Error`](crate::Error), which is what applying a plan containing that
/// refusal returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Refusal {
    /// The recorded path was edited on disk; see
    /// [`Error::Drift`](crate::Error::Drift). Lifted per-plan by
    /// [`DriftPolicy::Overwrite`].
    Drift,
    /// The path is on disk but absent from the manifest; see
    /// [`Error::Foreign`](crate::Error::Foreign). No policy lifts this.
    Foreign,
    /// The desired entry — bytes, kind, or executable bit — differs from
    /// what another owner holds at this path; see
    /// [`Error::OwnerConflict`](crate::Error::OwnerConflict). No policy
    /// lifts this.
    OwnerConflict {
        /// The other owners holding the path.
        owners: BTreeSet<String>,
    },
    /// The desired tree claims one on-disk location more than once: this
    /// key shares a normalized path with another desired key, or its path
    /// lies beneath another desired path (every desired entry is a
    /// non-directory, so nothing can be projected beneath one). Both sides
    /// of a conflict are refused — there is no deterministic entry to
    /// prefer; see [`Error::TreeConflict`](crate::Error::TreeConflict).
    /// [`load_mapping`](crate::load_mapping) rejects same-path duplicates
    /// at parse time as
    /// [`MappingDuplicate`](crate::Error::MappingDuplicate); this refusal
    /// is the deciding stage's verdict on any tree, however built.
    TreeConflict {
        /// The other desired keys, verbatim, claiming the same or an
        /// overlapping location.
        paths: BTreeSet<Utf8PathBuf>,
    },
    /// The projection may not write the path — it is refused by
    /// [`contained_join`](crate::contained_join) (absolute, climbing out
    /// via `..`, empty or `.` components, backslashes, and component
    /// shapes Windows resolves specially — its rustdoc is the full list),
    /// writes through a symlinked ancestor, or enters the projection's own
    /// state directory; see
    /// [`Error::Containment`](crate::Error::Containment).
    Containment,
    /// A symlink whose target resolves outside the destination; see
    /// [`Error::ExternalTarget`](crate::Error::ExternalTarget).
    ExternalTarget {
        /// The offending target string, verbatim.
        target: String,
    },
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
