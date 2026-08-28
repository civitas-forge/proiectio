use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use serde::Serialize;

#[cfg(unix)]
use camino::Utf8Path;
#[cfg(unix)]
use cap_std::fs_utf8::Dir;

#[cfg(unix)]
use crate::{Manifest, Result, classify, load_manifest, observe};

/// The classification of one path in the union of the manifest and the
/// destination directory.
///
/// Planning runs this classification too, then compares against the
/// desired tree to choose each path's [`Action`](crate::Action); status
/// needs no desired tree, so a path only the desired tree names has no
/// state here — it first appears in a [`Plan`](crate::Plan) as a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PathState {
    /// Disk matches the recorded entry: bytes, kind, and executable bit.
    Clean,
    /// Disk differs from the recorded entry — bytes, kind, or executable
    /// bit — a user edit.
    Drifted,
    /// Recorded, but gone from disk.
    Missing,
    /// On disk, absent from the manifest. Planning refuses to touch it —
    /// except where a desired [`Block`](crate::EntryKind::Block) entry
    /// owns only the delimited region inside it, in which case the
    /// container stays foreign as a file while the region is a write
    /// target. Until block-region classification lands, the deciding
    /// stage cannot yet see regions and refuses that case too —
    /// conservative, never a wrong write
    /// ([`decide`](crate::decide)'s rustdoc names the seam).
    Foreign,
}

/// The classification of every path in the union of the manifest and the
/// destination directory, with nothing written.
///
/// Classification covers what UTF-8 can name: a non-UTF-8 entry on disk
/// can never match a desired or recorded path, so it stays outside this
/// map — never overwritten, never removed, and a directory holding one
/// is never pruned.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Status {
    /// Per-path states, keyed by path relative to the destination.
    pub paths: BTreeMap<Utf8PathBuf, PathState>,
}

/// The read-only run: load the manifest, [`observe`] the destination,
/// [`classify`] the union, and return the report. Nothing is written — not
/// the manifest, not a lock file, nothing in the destination
/// (`docs/design.lex` section 2).
///
/// `state` is the capability handle on the state directory, and `None` says
/// that directory does not exist. Nothing in this crate spends ambient
/// authority — every I/O function takes a handle the caller opened — so the
/// absence a caller meets when opening the state directory is spelled in
/// the signature rather than discovered here.
///
/// `state_prefix` is the same state directory seen from the other side: its
/// path relative to the destination when it lies inside it, and `None` when
/// it lies outside. That subtree is the projection's own state and never
/// classifies, so a prefix that does not describe the directory `state`
/// opens misreports — omitting it for an in-dest state directory reports
/// the manifest and the lock file as [`Foreign`](PathState::Foreign), and
/// naming an unrelated subtree hides whatever the destination holds there.
/// [`Projection`](crate::Projection) holds both paths and derives the
/// prefix from them ([`state_prefix`](crate::Projection::state_prefix)), so
/// a caller reads the two arguments off one value rather than deriving the
/// second itself:
///
/// ```no_run
/// # use cap_std::ambient_authority;
/// # use cap_std::fs_utf8::Dir;
/// # use libproiectio::{Projection, Result, status};
/// # fn read(projection: &Projection) -> Result<()> {
/// let dest = Dir::open_ambient_dir(projection.target(), ambient_authority()).unwrap();
/// let state = match Dir::open_ambient_dir(projection.state_dir(), ambient_authority()) {
///     Ok(state) => Some(state),
///     // Never projected here: no state directory, so nothing is recorded.
///     Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
///     Err(e) => panic!("{e}"),
/// };
/// let report = status(&dest, state.as_ref(), projection.state_prefix())?;
/// # let _ = report;
/// # Ok(())
/// # }
/// ```
///
/// Both ways of having recorded nothing report the same thing, and neither
/// is an error: a state directory that does not exist reads as the empty
/// [`Manifest`], and so does one holding no manifest file yet
/// ([`load_manifest`]). Against a destination that is also empty the report
/// is empty; against one holding anything, every path it can name
/// classifies [`Foreign`](PathState::Foreign) — the projection wrote none
/// of them.
///
/// Foreign covers directories as well as files, here as everywhere
/// ([`classify`]): the manifest records no directories, so every directory
/// the walk meets is unrecorded — including the parents a past
/// [`apply`](crate::apply) created for the owned files inside them, which
/// report [`Clean`](PathState::Clean) beneath a
/// [`Foreign`](PathState::Foreign) parent. A caller rendering a report for
/// a person can drop the directory rows; planning is unaffected, since a
/// desired path only meets a foreign refusal where the tree names that
/// exact location.
///
/// No lock is taken. A concurrent [`apply`](crate::apply) can move the disk
/// under the walk, so a report is what the destination looked like, not a
/// promise about what it still looks like — the same reason `apply`
/// re-checks every node against the signature its plan expects.
#[cfg(unix)]
pub fn status(dest: &Dir, state: Option<&Dir>, state_prefix: Option<&Utf8Path>) -> Result<Status> {
    let manifest = match state {
        Some(state) => load_manifest(state)?,
        None => Manifest::new(),
    };
    let observations = observe(dest, &manifest)?;
    Ok(classify(&manifest, &observations, state_prefix))
}

#[cfg(all(test, unix))]
#[path = "status_tests.rs"]
mod tests;
