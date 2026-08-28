use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use serde::Serialize;

use crate::Entry;

/// What planning does when a recorded path's bytes on disk differ from the
/// recorded hash — a user edit.
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
/// re-validates containment and re-hashes targets before touching anything,
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
    /// survives.
    Skip,
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
    /// The path escapes the destination — absolute, climbing out via
    /// `..`, containing empty or `.` components, or writing through a
    /// symlinked ancestor; see
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
