use camino::{Utf8Path, Utf8PathBuf};

#[cfg(unix)]
use std::collections::BTreeMap;

#[cfg(unix)]
use std::io::ErrorKind::NotFound;

#[cfg(unix)]
use cap_std::ambient_authority;
#[cfg(unix)]
use cap_std::fs_utf8::Dir;

#[cfg(unix)]
use crate::{
    Entry, Error, Manifest, Origin, Plan, PlanOptions, RemovalScope, Result, Status, classify,
    decide, decide_removal, load_manifest, observe,
};

/// A destination directory paired with the state directory holding its
/// manifest — the whole public entry point (`docs/design.lex` section 3).
///
/// Both paths are absolute and caller-chosen, and constructing one touches
/// no filesystem. The projection opens what it needs when a call needs it:
/// nothing here takes or returns a directory handle, and nothing resolves
/// against the process's current directory.
///
/// The reads take no lock and open nothing the caller can see:
/// [`status`](Projection::status), [`manifest`](Projection::manifest),
/// [`plan`](Projection::plan) and
/// [`plan_removal`](Projection::plan_removal). A plan they return is a
/// report of what applying would do, not a reservation, which is what a dry
/// run wants — and it is why it cannot be applied. Writing is
/// [`begin`](Projection::begin), which returns the [`Run`](crate::Run) that
/// owns the single-writer guard and is the only thing that can apply.
///
/// `state_dir` may lie inside `target` (as a proper subdirectory, never
/// `target` itself). The projection's own state subtree is excluded from
/// classification — the manifest never reads as foreign — and a desired
/// path overlapping it is refused as
/// [`Containment`](crate::Error::Containment): one inside the subtree, and
/// one the state directory sits beneath.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    target: Utf8PathBuf,
    state_dir: Utf8PathBuf,
}

impl Projection {
    /// A projection writing into `target`, with its manifest kept in
    /// `state_dir`.
    ///
    /// # Panics
    ///
    /// Panics if either path is relative — the crate never consults the
    /// current directory, so a relative path here has no meaning it could
    /// honor — or carries `..` components: this type reasons about the two
    /// paths lexically ([`state_prefix`](Projection::state_prefix) strips
    /// one from the other), and a `..` does not resolve lexically, so such
    /// a spelling could misplace the state subtree. (`.` components need
    /// no refusal: path comparison works component-wise and already treats
    /// them as absent.) Panics too if `state_dir` equals
    /// `target`: the state files would sit at the destination root with no
    /// subtree to exclude from classification, so the projection's own
    /// manifest would read as foreign. Keep the state in a subdirectory
    /// (the conventional `<target>/.proiectio`) or outside the target
    /// entirely.
    pub fn new(target: Utf8PathBuf, state_dir: Utf8PathBuf) -> Self {
        assert!(
            target.is_absolute(),
            "projection target must be absolute, got {target}"
        );
        assert!(
            state_dir.is_absolute(),
            "projection state_dir must be absolute, got {state_dir}"
        );
        assert!(
            is_normalized(&target),
            "projection target must not carry `..` components, got {target}"
        );
        assert!(
            is_normalized(&state_dir),
            "projection state_dir must not carry `..` components, got {state_dir}"
        );
        assert!(
            state_dir != target,
            "projection state_dir must not equal the target ({target}): \
             the projection's own state files would classify as foreign"
        );
        Projection { target, state_dir }
    }

    /// The directory the projection writes into.
    pub fn target(&self) -> &Utf8Path {
        &self.target
    }

    /// The directory holding the manifest.
    pub fn state_dir(&self) -> &Utf8Path {
        &self.state_dir
    }

    /// The state directory's path relative to the target, when it lies
    /// inside the target: the subtree under it never classifies, and a
    /// location that overlaps it — inside the subtree, or with the state
    /// directory beneath the location — refuses as
    /// [`Containment`](crate::Error::Containment) wherever a plan would
    /// write or remove there.
    ///
    /// `None` when the state directory lives outside the target — nothing
    /// in the destination is the projection's own state, so nothing is
    /// excluded. (A state directory equal to the target, which would leave
    /// its files inside the destination yet outside any excludable
    /// subtree, is rejected by [`new`](Projection::new).)
    pub fn state_prefix(&self) -> Option<&Utf8Path> {
        match self.state_dir.strip_prefix(&self.target) {
            Ok(prefix) if !prefix.as_str().is_empty() => Some(prefix),
            _ => None,
        }
    }
}

/// The reads: no lock, and the caller opens nothing (`docs/design.lex`
/// section 3).
///
/// Each call opens the destination, reads the recorded state, and drops both
/// handles before it returns. A concurrent [`Run`](crate::Run) can move the
/// disk under any of them, so what comes back is what the destination looked
/// like, not a promise about what it still looks like.
#[cfg(unix)]
impl Projection {
    /// The classification of every path in the union of the manifest and the
    /// destination, with nothing written (`docs/design.lex` section 2).
    ///
    /// A destination nothing was ever projected into reports rather than
    /// failing: a state directory that does not exist and one holding no
    /// manifest both read as the empty [`Manifest`], against which every path
    /// the walk can name classifies [`Foreign`](crate::PathState::Foreign).
    pub fn status(&self) -> Result<Status> {
        let dest = self.open_target()?;
        let manifest = self.manifest()?;
        let observations = observe(&dest, &manifest)?;
        Ok(classify(&manifest, &observations, self.state_prefix()))
    }

    /// The recorded state: what the projection wrote, per path, with its
    /// owners.
    ///
    /// A state directory that does not exist, and one holding no manifest
    /// file yet, both read as the empty [`Manifest`].
    pub fn manifest(&self) -> Result<Manifest> {
        match self.open_state() {
            Some(state) => load_manifest(&state?),
            None => Ok(Manifest::new()),
        }
    }

    /// Every write, overwrite, removal, and refusal applying `desired` under
    /// `owner` would perform. An empty tree plans a removal.
    ///
    /// `origin` says where the tree came from; every refusal the plan carries
    /// names it ([`Origin`]).
    ///
    /// The plan is a report, not a reservation: it is decided outside the
    /// single-writer guard, so the manifest it was decided against can move
    /// before anything acts on it. That is what a dry run wants, and it is
    /// why nothing applies a plan from here — [`begin`](Projection::begin)
    /// decides and applies under one guard.
    pub fn plan(
        &self,
        owner: &str,
        desired: &BTreeMap<Utf8PathBuf, Entry>,
        origin: Origin,
        options: PlanOptions,
    ) -> Result<Plan> {
        let dest = self.open_target()?;
        let manifest = self.manifest()?;
        let observations = observe(&dest, &manifest)?;
        Ok(decide(
            owner,
            desired,
            origin,
            &manifest,
            &observations,
            self.state_prefix(),
            options,
        ))
    }

    /// The removal on its own terms: everything `owner` holds, or the
    /// recorded paths `scope` names. Clearing the owner and naming no path
    /// are separate spellings, never an empty list
    /// ([`RemovalScope`]).
    ///
    /// A report, on the same terms as [`plan`](Projection::plan).
    pub fn plan_removal(
        &self,
        owner: &str,
        scope: RemovalScope<'_>,
        options: PlanOptions,
    ) -> Result<Plan> {
        let dest = self.open_target()?;
        let manifest = self.manifest()?;
        let observations = observe(&dest, &manifest)?;
        Ok(decide_removal(
            owner,
            scope,
            &manifest,
            &observations,
            self.state_prefix(),
            options,
        ))
    }

    /// A handle on the destination directory, which must already exist: a
    /// projection writes into a directory somebody chose, and creating one
    /// from a mistyped path would put the tree somewhere nobody named.
    pub(crate) fn open_target(&self) -> Result<Dir> {
        Dir::open_ambient_dir(&self.target, ambient_authority()).map_err(|source| Error::Io {
            path: self.target.clone(),
            source,
        })
    }

    /// A handle on the state directory, or `None` where there is none yet —
    /// a destination never projected into. Reads distinguish the two;
    /// [`begin`](Projection::begin) creates the directory instead.
    ///
    /// A state directory inside the target is reached through the
    /// destination handle, on the same terms as
    /// [`begin`](Projection::begin) opens it: the handle and the prefix
    /// [`state_prefix`](Projection::state_prefix) excludes from
    /// classification then name one directory, because a prefix component
    /// that is a symlink leaving the target is refused rather than
    /// followed. A destination that does not exist reads as no state
    /// directory, since an in-dest state directory cannot outlive the
    /// target it sits in.
    fn open_state(&self) -> Option<Result<Dir>> {
        match self.state_prefix() {
            Some(prefix) => match self.open_target() {
                Ok(dest) => self.absent_is_none(dest.open_dir(prefix)),
                Err(Error::Io { source, .. }) if source.kind() == NotFound => None,
                Err(error) => Some(Err(error)),
            },
            None => {
                self.absent_is_none(Dir::open_ambient_dir(&self.state_dir, ambient_authority()))
            }
        }
    }

    /// One open of the state directory, with its absence reported as `None`
    /// rather than as an error.
    fn absent_is_none(&self, opened: std::io::Result<Dir>) -> Option<Result<Dir>> {
        match opened {
            Ok(state) => Some(Ok(state)),
            Err(source) if source.kind() == NotFound => None,
            Err(source) => Some(Err(Error::Io {
                path: self.state_dir.clone(),
                source,
            })),
        }
    }
}

/// Whether an absolute path is free of `..` components — the shape
/// [`Projection`]'s lexical equality and prefix reasoning requires. (`.`
/// components never survive component iteration, so they need no check.)
fn is_normalized(path: &Utf8Path) -> bool {
    path.components()
        .all(|component| component != camino::Utf8Component::ParentDir)
}

#[cfg(test)]
#[path = "projection_tests.rs"]
mod tests;
