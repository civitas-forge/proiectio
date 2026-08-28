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
/// `state` says where the recorded state is, in the one place a report
/// depends on it twice: the handle the manifest is read through, and the
/// state directory's position relative to the destination, which decides
/// whether the walk crosses the projection's own files. [`StateDir`] pairs
/// them, so a report cannot be asked for against a handle and a prefix that
/// describe different directories. Nothing here opens anything — every I/O
/// function in this crate takes a handle the caller opened, so the absence
/// a caller meets at the state directory is one of the variants rather than
/// something discovered here.
///
/// [`Projection`](crate::Projection) holds the two paths and already
/// answers whether the state directory lies inside the target
/// ([`state_prefix`](crate::Projection::state_prefix)), so the variant
/// follows from what a caller has:
///
/// ```no_run
/// # use cap_std::ambient_authority;
/// # use cap_std::fs_utf8::Dir;
/// # use libproiectio::{Projection, Result, StateDir, status};
/// # fn read(projection: &Projection) -> Result<()> {
/// let dest = Dir::open_ambient_dir(projection.target(), ambient_authority()).unwrap();
/// let opened = match Dir::open_ambient_dir(projection.state_dir(), ambient_authority()) {
///     Ok(dir) => Some(dir),
///     // Never projected here: no state directory, so nothing is recorded.
///     Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
///     Err(e) => panic!("{e}"),
/// };
/// let state = match (opened.as_ref(), projection.state_prefix()) {
///     (None, _) => StateDir::Missing,
///     (Some(dir), Some(prefix)) => StateDir::Inside { dir, prefix },
///     (Some(dir), None) => StateDir::Outside(dir),
/// };
/// let report = status(&dest, state)?;
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
/// [`Foreign`](PathState::Foreign) parent. A row carries that relationship
/// and nothing else — not the kind of node standing there — so nothing in
/// the report separates a foreign directory from a foreign file, and a
/// caller that wants to render them differently needs a kind [`Status`]
/// does not yet carry. Planning is unaffected either way: a desired path
/// meets a foreign refusal only where the tree names that exact location.
///
/// No lock is taken. A concurrent [`apply`](crate::apply) can move the disk
/// under the walk, so a report is what the destination looked like, not a
/// promise about what it still looks like — the same reason `apply`
/// re-checks every node against the signature its plan expects.
#[cfg(unix)]
pub fn status(dest: &Dir, state: StateDir<'_>) -> Result<Status> {
    let (manifest, state_prefix) = match state {
        StateDir::Missing => (Manifest::new(), None),
        StateDir::Outside(dir) => (load_manifest(dir)?, None),
        StateDir::Inside { dir, prefix } => (load_manifest(dir)?, Some(prefix)),
    };
    let observations = observe(dest, &manifest)?;
    Ok(classify(&manifest, &observations, state_prefix))
}

/// Where a [`status`] run reads recorded state from, and whether that
/// state sits inside the destination it is reporting on.
///
/// The two facts travel together because a report needs both and they must
/// describe one directory: the manifest is read through the handle, and the
/// destination-relative prefix is the subtree the classification skips as
/// the projection's own. Spelled apart they could disagree — a handle on an
/// in-dest state directory with no prefix reports the manifest and the lock
/// file as [`Foreign`](PathState::Foreign), and a prefix naming some other
/// subtree hides whatever the destination holds there — and no handle can
/// be checked against a path to catch it. Named together, neither
/// combination can be written.
#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
pub enum StateDir<'a> {
    /// The state directory does not exist: nothing was ever projected
    /// here, so nothing is recorded and no subtree of the destination is
    /// the projection's own. Reads as the empty [`Manifest`].
    Missing,
    /// A state directory outside the destination. Every path the walk
    /// reaches belongs to the destination, so the whole tree classifies.
    Outside(&'a Dir),
    /// A state directory inside the destination, at `prefix` relative to
    /// it — the conventional `<target>/.proiectio`. The subtree under
    /// `prefix` is the projection's own state and never classifies.
    Inside {
        /// The handle the manifest is read through.
        dir: &'a Dir,
        /// Where that same directory sits relative to the destination.
        prefix: &'a Utf8Path,
    },
}

#[cfg(all(test, unix))]
#[path = "status_tests.rs"]
mod tests;
