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

/// What planning does with a desired symlink whose target resolves outside
/// the destination — an absolute target, one climbing out, one reaching
/// outside through a link the run leaves the destination holding — one
/// already there or one the tree projects — or one of the
/// spellings graded external on every host (`docs/security.lex` section 3
/// carries the whole rule).
///
/// Such a link writes nothing outside the destination; it is only a
/// pointer. But a tree the invoker did not author planting pointers into
/// the rest of the filesystem is a surprise, so it takes an opt-in on the
/// invocation rather than a key in the tree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum ExternalTargetPolicy {
    /// Refuse the link and name it with its target (the default; a CLI maps
    /// this refusal to exit 2).
    #[default]
    Refuse,
    /// Write the link, target verbatim (a CLI's `--allow-external-targets`).
    Allow,
}

/// The policy inputs one [`decide`](crate::decide) call runs under: what
/// the caller permits, as opposed to what the desired tree, the manifest,
/// and the disk say.
///
/// Both fields default to refusing, so `PlanOptions::default()` is the
/// strict projection; a caller lifts one policy at a time:
///
/// ```
/// # use libproiectio::{DriftPolicy, ExternalTargetPolicy, PlanOptions};
/// let forced = PlanOptions {
///     drift: DriftPolicy::Overwrite,
///     ..PlanOptions::default()
/// };
/// assert_eq!(forced.external_targets, ExternalTargetPolicy::Refuse);
/// ```
///
/// The two policies stay separate types because they lift unrelated rules:
/// drift is about the destination's own edits, external targets about where
/// a projected pointer points.
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
/// A plan is complete: apply's inputs are the projection and the plan —
/// never the desired tree — because each action carries what executing it
/// needs: the [`Entry`] to write, and for destructive and recording
/// actions the [`NodeSignature`] the disk must still match. Plans are
/// plain data, not capabilities: apply re-validates containment, refuses
/// an [`Overwrite`](Action::Overwrite), [`Skip`](Action::Skip),
/// [`Remove`](Action::Remove), or [`Release`](Action::Release) keyed by a
/// path the manifest does not record, and re-checks the disk against each
/// action's `expected` signature before touching anything — the recorded
/// entry itself is never the signature baseline, since `expected` may
/// deliberately differ from it (an agreement skip, a lifted drift) — so a
/// hand-built or stale plan refuses rather than misfires. `BTreeMap`
/// keeps plans sorted, diffable, and deterministic; apply derives execution
/// order from it — removals in reverse, then everything else parents before
/// children, then the symlinks, each published only once the destination
/// holds whatever its target resolves through (`docs/implementation.lex`
/// section 6).
///
/// An empty desired tree plans a removal of everything the owner alone
/// holds and a release of everything it shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Plan {
    /// The owner the plan was computed for; applied entries are recorded
    /// under this name in the manifest.
    pub owner: String,
    /// Whether the caller permitted external symlink targets when this
    /// plan was decided.
    ///
    /// The one policy a plan carries, because it is the one apply cannot
    /// read back off an action. A lifted drift refusal reaches apply as the
    /// drifted node in an action's `expected` signature, so apply honors it
    /// without knowing [`DriftPolicy`]; a *permitted* external target
    /// leaves nothing behind — the link is written verbatim either way, and
    /// a plan that graded every target in-dest is indistinguishable from one
    /// that never graded any. Apply re-grades a link's target against the
    /// disk before publishing it (`docs/security.lex` section 3), and this
    /// says whether that re-grade has a verdict to hold the destination to:
    /// under [`Refuse`](ExternalTargetPolicy::Refuse) a target that has
    /// become escaping since the plan refuses, and under
    /// [`Allow`](ExternalTargetPolicy::Allow) there was never a verdict, so
    /// nothing is re-graded and every target still lands verbatim.
    pub external_targets: ExternalTargetPolicy,
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
    /// For an [`Entry::Block`] entry, "on disk" means the region, not the
    /// container: a container without this projection's marker plans a
    /// `Write`, which splices the region in and leaves the rest of the file
    /// byte for byte. The container itself is never created — apply refuses
    /// as [`Error::Block`](crate::Error::Block) where it is gone — and a
    /// region apply finds already carrying the desired body is adopted and
    /// reported [`Skipped`](crate::ApplyOutcome::Skipped) instead.
    Write {
        /// What to write.
        entry: Entry,
    },
    /// Replace a recorded path whose desired content changed. Apply
    /// re-checks the target against `expected` first and refuses if the
    /// disk no longer matches.
    Overwrite {
        /// What to write.
        entry: Entry,
        /// The node the disk must still hold at apply time: the recorded
        /// signature for a clean path, or — when [`DriftPolicy::Overwrite`]
        /// lifted a drift refusal — the drifted node observed at plan
        /// time.
        expected: NodeSignature,
    },
    /// Disk already equals desired: nothing is written and the mtime
    /// survives. Apply re-checks the disk against `expected` — the desired
    /// node's signature, which the disk matched at plan time — refuses if
    /// anything changed since the plan, and records the signature with
    /// this plan's owner on the path's manifest entry. That recording is
    /// how an owner joins a path another owner already holds identically,
    /// and how the manifest catches up when a drifted path was edited into
    /// agreement with the desired tree — the recorded entry may differ
    /// from `expected` in any field, so the action carries the signature
    /// whole.
    Skip {
        /// The desired node's signature, which the disk carries at plan
        /// time and must still carry at apply time.
        expected: NodeSignature,
    },
    /// Remove an orphan — recorded under this owner alone and absent from
    /// the desired tree. Apply re-checks the target against `expected`
    /// first and refuses if the disk no longer matches. Directories
    /// emptied by removal are pruned.
    Remove {
        /// The node the disk must still hold at apply time — the recorded
        /// signature, or the drifted node observed at plan time when
        /// [`DriftPolicy::Overwrite`] lifted a drift refusal. `None` for a
        /// path that was already gone at plan time
        /// ([`Missing`](crate::PathState::Missing)): apply then drops the
        /// entry from the manifest alone, and refuses if a node has
        /// appeared at the path since the plan.
        expected: Option<NodeSignature>,
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

/// The on-disk node an action expects at apply time: what the deciding
/// stage knew about the path — the recorded entry, the desired entry the
/// disk already matched, or the drifted node a lifted refusal was granted
/// for. Apply re-checks all three fields against the disk before the
/// action proceeds and refuses if any changed since the plan, so the
/// changed-since-plan guarantee covers exactly what classification calls
/// drift: bytes, kind, and mode
/// ([`PathState::Drifted`](crate::PathState::Drifted)).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeSignature {
    /// The node's kind. For a [`Block`](EntryKind::Block) it carries the
    /// marker and placement the region is located with, so a caller who
    /// changed either has apply strip the old region and splice the new one
    /// in the same publish.
    pub kind: EntryKind,
    /// [`sha256_hex`](crate::sha256_hex) of the node — file contents, the
    /// link target string, or a region's body.
    pub hash: String,
    /// The executable bit; always `false` for symlinks and blocks
    /// ([`ManifestEntry::executable`](crate::ManifestEntry::executable)).
    pub executable: bool,
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
    /// lies beneath another desired path. No file or block can hold
    /// children; beneath a desired *symlink* the nesting is expressible on
    /// disk — apply's owned-link walk would follow the link and write
    /// through it — but the write would land somewhere the plan does not
    /// name, so it is refused as well (the no-alias rule
    /// [`decide`](crate::decide) documents). Both sides
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
    /// it lies beneath a symlink that outlives the plan (the no-alias rule
    /// [`decide`](crate::decide) documents — a link on disk, owned or
    /// foreign, that no action in this plan removes; beneath a *desired*
    /// link the refusal is [`TreeConflict`](Refusal::TreeConflict)), or it
    /// enters the projection's own state directory.
    ///
    /// Applying refuses under the narrower apply-time rule of
    /// `docs/security.lex` section 2, which still lets the walk follow an
    /// ancestor link the manifest owns whose target resolves inside the
    /// destination; see [`Error::Containment`](crate::Error::Containment)
    /// for that split.
    Containment,
    /// A desired symlink whose target, resolved from the link's parent
    /// through the destination's own links, lands outside the destination —
    /// absolute, climbing out, reaching outside through a link the run
    /// leaves dest holding, or one of the spellings graded external on every
    /// host (`docs/security.lex` section 3 carries the whole rule). Lifted per-plan by
    /// [`ExternalTargetPolicy::Allow`], which writes the link with the
    /// target verbatim; see
    /// [`Error::ExternalTarget`](crate::Error::ExternalTarget).
    ExternalTarget {
        /// The offending target string, verbatim.
        target: String,
    },
    /// A desired symlink whose target is not a pathname on any host: the
    /// empty string, or one carrying a NUL byte. Judged before grading —
    /// a string naming no path lands nowhere to grade — and no policy
    /// lifts it, since there is no pointer to permit; see
    /// [`Error::InvalidTarget`](crate::Error::InvalidTarget).
    InvalidTarget {
        /// The offending target string, verbatim.
        target: String,
    },
    /// A [`Block`](EntryKind::Block) entry the projection declines — a
    /// marker or body the rules on [`EntryKind::Block`] forbid, a container
    /// that is not there, or a path changing between a whole node and a
    /// block. No policy lifts any of them; see
    /// [`Error::Block`](crate::Error::Block).
    Block {
        /// Which rule the entry or its container broke.
        fault: crate::BlockFault,
    },
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
