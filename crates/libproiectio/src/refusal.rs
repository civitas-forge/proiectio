use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

use crate::Origin;

/// Why one path is refused rather than acted on: the per-path vocabulary
/// [`Action::Refuse`](crate::Action::Refuse) carries and [`Refused`]
/// aggregates.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Refusal {
    /// The projection may not write the path — it is refused by
    /// [`contained_join`](crate::contained_join), it lies beneath a symlink
    /// that outlives the plan, or it overlaps the state directory.
    Containment,
    /// The desired tree claims one on-disk location more than once: this key
    /// shares a normalized path with another desired key, or its path lies
    /// beneath another desired path. Both sides of a conflict are refused.
    TreeConflict {
        /// The other desired keys, verbatim, claiming the same or an
        /// overlapping location.
        paths: BTreeSet<Utf8PathBuf>,
    },
    /// The path is on disk but absent from the manifest.
    Foreign,
    /// The recorded path was edited on disk. Lifted per-plan by
    /// [`DriftPolicy::Overwrite`](crate::DriftPolicy::Overwrite).
    Drift,
    /// A directory stands where the plan needs a file or a link, or needs the
    /// path gone, and the run's own removals do not empty it. Lifted by
    /// nothing: what the directory holds is not the projection's to unlink.
    DirectoryInTheWay {
        /// What keeps the directory standing, each with the owners recording
        /// it — empty where nothing records it. An empty map is a directory
        /// holding nothing the run may remove: one somebody made by hand.
        holding: BTreeMap<Utf8PathBuf, BTreeSet<String>>,
        /// The directory itself, or ones beneath it, holding a name that is
        /// not UTF-8. Observation cannot represent such an entry, so nothing
        /// may conclude the directory empties, and the run refuses rather
        /// than remove on a premise it cannot check.
        unreadable: BTreeSet<Utf8PathBuf>,
    },
    /// The desired entry — bytes, kind, or executable bit — differs from what
    /// another owner holds at this path.
    OwnerConflict {
        /// The other owners holding the path.
        owners: BTreeSet<String>,
    },
    /// A desired symlink whose target, resolved from the link's parent
    /// through the destination's own links, lands outside the destination.
    /// Lifted per-plan by [`ExternalTargetPolicy::Allow`](crate::ExternalTargetPolicy::Allow).
    ExternalTarget {
        /// The offending target string, verbatim.
        target: String,
    },
    /// A desired symlink whose target is not a pathname on any host: the
    /// empty string, or one carrying a NUL byte.
    InvalidTarget {
        /// The offending target string, verbatim.
        target: String,
    },
    /// A [`Block`](crate::EntryKind::Block) entry the projection declines.
    Block {
        /// Which rule the entry or its container broke.
        fault: BlockFault,
    },
}

/// Why one [`Block`](crate::EntryKind::Block) entry is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum BlockFault {
    /// The marker is the empty string, which would match every line start.
    MarkerEmpty,
    /// The marker carries a `\n` or a `\r`; a marker is one line.
    MarkerNotOneLine,
    /// The marker begins or ends with a space or a tab.
    MarkerEdgeWhitespace,
    /// A line of the body equals the marker.
    BodyCarriesMarker,
    /// The placement is [`Prepend`](crate::Placement::Prepend) and the body
    /// neither is empty nor ends with `\n`.
    BodyNotNewlineTerminated,
    /// The placement is [`Append`](crate::Placement::Append) and the author's
    /// side of the container neither is empty nor ends with `\n`.
    ContainerNotNewlineTerminated,
    /// The container, or a directory above it, is not there.
    ContainerMissing,
    /// The path is recorded as a whole node and desired as a block, or the
    /// other way round.
    KindChange,
    /// A plan's expected signature names a marker or placement the manifest
    /// does not record at that path.
    SignatureNotRecorded,
    MarkerInAuthorText,
}

impl std::fmt::Display for BlockFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self {
            BlockFault::MarkerEmpty => "the marker is empty",
            BlockFault::MarkerNotOneLine => "the marker carries a line break",
            BlockFault::MarkerEdgeWhitespace => "the marker begins or ends with a space or a tab",
            BlockFault::BodyCarriesMarker => "a line of the body equals the marker",
            BlockFault::BodyNotNewlineTerminated => {
                "prepending needs a body that is empty or ends with a newline"
            }
            BlockFault::ContainerNotNewlineTerminated => {
                "appending needs a container that is empty or ends with a newline"
            }
            BlockFault::ContainerMissing => {
                "the container is not there, and a block never creates one"
            }
            BlockFault::KindChange => "a path never changes between a whole node and a block",
            BlockFault::SignatureNotRecorded => {
                "the expected signature names a region the manifest does not record"
            }
            BlockFault::MarkerInAuthorText => {
                "the container's author side already holds the marker being migrated to"
            }
        };
        f.write_str(reason)
    }
}

impl Refusal {
    /// This refusal's name without its payload.
    pub fn kind(&self) -> RefusalKind {
        match self {
            Refusal::Drift => RefusalKind::Drift,
            Refusal::DirectoryInTheWay { .. } => RefusalKind::DirectoryInTheWay,
            Refusal::Foreign => RefusalKind::Foreign,
            Refusal::Containment => RefusalKind::Containment,
            Refusal::TreeConflict { .. } => RefusalKind::TreeConflict,
            Refusal::OwnerConflict { .. } => RefusalKind::OwnerConflict,
            Refusal::ExternalTarget { .. } => RefusalKind::ExternalTarget,
            Refusal::InvalidTarget { .. } => RefusalKind::InvalidTarget,
            Refusal::Block { .. } => RefusalKind::Block,
        }
    }

    /// What a message says about one path after its name: the payload,
    /// rendered.
    fn detail(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::Drift | Refusal::Foreign | Refusal::Containment => Ok(()),
            Refusal::DirectoryInTheWay {
                holding,
                unreadable,
            } => {
                let mut clauses = Vec::new();
                if !holding.is_empty() {
                    clauses.push(format!(
                        "holding {}, which --force does not remove",
                        holding.iter().map(held).collect::<Vec<_>>().join(", ")
                    ));
                }
                if !unreadable.is_empty() {
                    clauses.push(format!(
                        "holding names that are not UTF-8 in {}",
                        join(unreadable.iter().map(|path| path.as_str()), ", ")
                    ));
                }
                if clauses.is_empty() {
                    return Ok(());
                }
                write!(f, " ({})", clauses.join(", and "))
            }
            Refusal::TreeConflict { paths } => write!(
                f,
                " (with {})",
                join(paths.iter().map(|path| path.as_str()), ", ")
            ),
            Refusal::OwnerConflict { owners } => {
                write!(
                    f,
                    " (held by {})",
                    join(owners.iter().map(String::as_str), "+")
                )
            }
            Refusal::ExternalTarget { target } => write!(f, " -> {target}"),
            // Quoted and escaped, so an empty string or a NUL byte renders
            // visibly.
            Refusal::InvalidTarget { target } => write!(f, " -> {target:?}"),
            Refusal::Block { fault } => write!(f, " ({fault})"),
        }
    }
}

/// A refusal's name: what a [`Refused`] error is about.
///
/// Declared in precedence order, which `Ord` follows: when a plan carries
/// several kinds, applying reports the least. Adding a kind means placing
/// it in this list; nothing else ranks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum RefusalKind {
    /// [`Refusal::Containment`].
    Containment,
    /// [`Refusal::TreeConflict`].
    TreeConflict,
    /// [`Refusal::Foreign`].
    Foreign,
    /// [`Refusal::Drift`].
    Drift,
    /// [`Refusal::DirectoryInTheWay`]. Ranked below [`Drift`](Self::Drift) so
    /// that a drifted node beneath the directory — the one thing `--force`
    /// clears out of the way — is the message a run states first.
    DirectoryInTheWay,
    /// [`Refusal::OwnerConflict`].
    OwnerConflict,
    /// [`Refusal::ExternalTarget`].
    ExternalTarget,
    /// [`Refusal::InvalidTarget`].
    InvalidTarget,
    /// [`Refusal::Block`].
    Block,
}

impl RefusalKind {
    /// The sentence a message of this kind opens with.
    fn headline(self) -> &'static str {
        match self {
            RefusalKind::Drift => "refusing to touch drifted paths (edited on disk)",
            RefusalKind::Foreign => {
                "refusing to touch foreign paths (not written by this projection)"
            }
            RefusalKind::DirectoryInTheWay => {
                "refusing directories the plan can neither replace nor remove"
            }
            RefusalKind::Containment => "refusing paths that violate containment",
            RefusalKind::TreeConflict => "refusing desired paths that claim overlapping locations",
            RefusalKind::OwnerConflict => {
                "refusing paths whose desired entries conflict with another owner's"
            }
            RefusalKind::ExternalTarget => "refusing symlinks with targets outside the destination",
            RefusalKind::InvalidTarget => "refusing symlinks whose targets are not paths",
            RefusalKind::Block => "refusing block entries",
        }
    }
}

/// Every key one kind of refusal declined — the value
/// [`Error::Refused`](crate::Error::Refused) carries. Only [`Refused::one`]
/// and [`Refused::aggregate`] build one, so it is never empty and every
/// reason in it is of one [`kind`](Refused::kind).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Refused {
    paths: BTreeMap<Utf8PathBuf, RefusedPath>,
}

/// One refused key's reason and provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefusedPath {
    /// Why the key is refused.
    pub refusal: Refusal,
    /// Which source named the key: the one the plan records for it — or,
    /// for an ancestor refused while acting on a planned key, for that key
    /// — and [`Origin::Caller`] where the caller named it directly.
    pub origin: Origin,
}

impl Refused {
    /// A single refused key, for a refusal met after the plan was
    /// validated — the disk moved, or a loader declined a key.
    pub fn one(path: Utf8PathBuf, refusal: Refusal, origin: Origin) -> Refused {
        Refused {
            paths: BTreeMap::from([(path, RefusedPath { refusal, origin })]),
        }
    }

    /// Everything a plan refused, reduced to the least [`RefusalKind`]
    /// present. `None` when nothing was refused.
    pub fn aggregate(
        refused: impl IntoIterator<Item = (Utf8PathBuf, Refusal, Origin)>,
    ) -> Option<Refused> {
        let mut by_kind: BTreeMap<RefusalKind, BTreeMap<Utf8PathBuf, RefusedPath>> =
            BTreeMap::new();
        for (path, refusal, origin) in refused {
            by_kind
                .entry(refusal.kind())
                .or_default()
                .insert(path, RefusedPath { refusal, origin });
        }
        let (_, paths) = by_kind.pop_first()?;
        Some(Refused { paths })
    }

    /// The same refusal with each key's origin taken from `plan`, for one
    /// built where only the key was known: the origin the plan records for
    /// the key, or for `acting_on` — the planned key being acted on when
    /// the refusal was met — where the plan records none for the key.
    pub fn sourced_by(mut self, plan: &crate::Plan, acting_on: &Utf8Path) -> Refused {
        for (path, refused) in &mut self.paths {
            refused.origin = plan
                .origins
                .get(path)
                .cloned()
                .unwrap_or_else(|| plan.origin_of(acting_on));
        }
        self
    }

    /// What every key here was refused for.
    pub fn kind(&self) -> RefusalKind {
        self.paths
            .values()
            .next()
            .expect("a Refused is never empty")
            .refusal
            .kind()
    }

    /// The refused keys, verbatim, each with its reason and provenance. A
    /// containment refusal keeps the absolute or escaping key it declined;
    /// every other kind's keys are paths relative to the destination.
    pub fn paths(&self) -> &BTreeMap<Utf8PathBuf, RefusedPath> {
        &self.paths
    }

    /// The refused keys alone, for a caller that only wants to name them.
    pub fn keys(&self) -> impl Iterator<Item = &Utf8Path> {
        self.paths.keys().map(Utf8PathBuf::as_path)
    }
}

impl std::error::Error for Refused {}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.kind().headline())?;
        for (i, (path, RefusedPath { refusal, origin })) in self.paths.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            f.write_str(path.as_str())?;
            refusal.detail(f)?;
            match origin {
                Origin::Caller => {}
                named => write!(f, " ({named})")?,
            }
        }
        Ok(())
    }
}

fn join<'a>(items: impl Iterator<Item = &'a str>, sep: &str) -> String {
    items.collect::<Vec<_>>().join(sep)
}

/// One node keeping a directory standing: its path, and the owners recording
/// it where any do.
fn held((path, owners): (&Utf8PathBuf, &BTreeSet<String>)) -> String {
    if owners.is_empty() {
        path.to_string()
    } else {
        format!(
            "{path} (held by {})",
            join(owners.iter().map(String::as_str), "+")
        )
    }
}

#[cfg(test)]
#[path = "refusal_tests.rs"]
mod tests;
