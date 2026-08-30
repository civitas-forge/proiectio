use camino::{Utf8Path, Utf8PathBuf};

use std::io::ErrorKind::NotFound;

use cap_std::ambient_authority;
use cap_std::fs_utf8::Dir;

use crate::{
    BlockMarkers, Desired, Error, Manifest, Plan, PlanOptions, PlannedAction, RemovalScope, Report,
    Result, Status, absolutize, block_markers, decide, decide_removal, load_manifest, observe,
    status,
};

const DEFAULT_STATE_DIR: &str = ".proiectio";

/// A destination directory paired with the state directory holding its
/// manifest. The reads here return reports nothing can apply;
/// [`begin`](Projection::begin) returns the [`Run`](crate::Run) that writes.
///
/// `state_dir` may lie inside `target` as a proper subdirectory: that
/// subtree is excluded from classification, and a desired path overlapping
/// it refuses as [`Containment`](crate::Refusal::Containment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    target: Utf8PathBuf,
    state_dir: Utf8PathBuf,
}

impl Projection {
    /// A projection writing into `target`, with its manifest kept in
    /// `state_dir`, which defaults to `<target>/.proiectio`.
    pub fn new(target: &Utf8Path, state_dir: Option<&Utf8Path>) -> Result<Projection> {
        let target = absolutize(target)?;
        let state_dir = match state_dir {
            Some(state_dir) => absolutize(state_dir)?,
            None => target.join(DEFAULT_STATE_DIR),
        };
        if state_dir == target {
            return Err(Error::StateDirIsTarget { path: target });
        }
        Ok(Projection { target, state_dir })
    }

    /// The directory the projection writes into.
    pub fn target(&self) -> &Utf8Path {
        &self.target
    }

    /// The directory holding the manifest.
    pub fn state_dir(&self) -> &Utf8Path {
        &self.state_dir
    }

    /// The state directory's path relative to the target where it lies
    /// inside it — the subtree excluded from classification — and `None`
    /// where it lies outside.
    pub fn state_prefix(&self) -> Option<&Utf8Path> {
        match self.state_dir.strip_prefix(&self.target) {
            Ok(prefix) if !prefix.as_str().is_empty() => Some(prefix),
            _ => None,
        }
    }
}

/// The reads: each call opens the destination and the recorded state, takes
/// no lock, and drops both handles before it returns.
impl Projection {
    /// The [`Status`] of the destination, with nothing written. A missing
    /// state directory or manifest reads as the empty [`Manifest`].
    pub fn status(&self) -> Result<Status> {
        let dest = self.open_target()?;
        let manifest = self.manifest_under(&dest)?;
        let observations = observe(&dest, &manifest, &BlockMarkers::new())?;
        Ok(status(&manifest, &observations, self.state_prefix()))
    }

    /// The recorded state: what the projection wrote, per path, with its
    /// owners. A missing state directory or manifest file reads as the empty
    /// [`Manifest`].
    pub fn manifest(&self) -> Result<Manifest> {
        match self.open_state(None) {
            Some(state) => load_manifest(&state?),
            None => Ok(Manifest::new()),
        }
    }

    /// The manifest read through a destination handle the caller already
    /// holds, so an in-dest state directory is a child of the directory the
    /// observation walks. A second open of the target path would let a rename
    /// between the two opens classify one directory against another's
    /// manifest.
    fn manifest_under(&self, dest: &Dir) -> Result<Manifest> {
        match self.open_state(Some(dest)) {
            Some(state) => load_manifest(&state?),
            None => Ok(Manifest::new()),
        }
    }

    /// Every write, overwrite, removal, and refusal applying `desired` under
    /// `owner` would perform, decided outside the single-writer guard and so
    /// not applicable, with the manifest it was decided against. An empty
    /// tree plans a removal; `origin` is named by every refusal the plan
    /// carries.
    pub fn plan(&self, owner: &str, desired: &Desired, options: PlanOptions) -> Result<Planned> {
        let dest = self.open_target()?;
        let manifest = self.manifest_under(&dest)?;
        let observations = observe(&dest, &manifest, &block_markers(desired))?;
        let plan = decide(
            owner,
            desired,
            &manifest,
            &observations,
            self.state_prefix(),
            options,
        )?;
        Ok(Planned { plan, manifest })
    }

    /// The removal of everything `owner` holds, or of the recorded paths
    /// `scope` names ([`RemovalScope`]); a report, on the same terms as
    /// [`plan`](Projection::plan).
    pub fn plan_removal(
        &self,
        owner: &str,
        scope: RemovalScope<'_>,
        options: PlanOptions,
    ) -> Result<Plan> {
        let dest = self.open_target()?;
        let manifest = self.manifest_under(&dest)?;
        let observations = observe(&dest, &manifest, &BlockMarkers::new())?;
        Ok(decide_removal(
            owner,
            scope,
            &manifest,
            &observations,
            self.state_prefix(),
            options,
        ))
    }

    /// A handle on the destination directory, which must already exist.
    pub(crate) fn open_target(&self) -> Result<Dir> {
        Dir::open_ambient_dir(&self.target, ambient_authority()).map_err(|source| Error::Io {
            path: self.target.clone(),
            source,
        })
    }

    /// A handle on the state directory, or `None` where there is none yet.
    /// An in-dest state directory is opened through `dest` where the caller
    /// holds one, and otherwise through a fresh open of the target, whose
    /// absence also reads as `None`.
    fn open_state(&self, dest: Option<&Dir>) -> Option<Result<Dir>> {
        let Some(prefix) = self.state_prefix() else {
            return self
                .absent_is_none(Dir::open_ambient_dir(&self.state_dir, ambient_authority()));
        };
        match dest {
            Some(dest) => self.absent_is_none(dest.open_dir(prefix)),
            None => match self.open_target() {
                Ok(dest) => self.absent_is_none(dest.open_dir(prefix)),
                Err(Error::Io { source, .. }) if source.kind() == NotFound => None,
                Err(error) => Some(Err(error)),
            },
        }
    }

    /// One open of the state directory, with its absence reported as `None`.
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

/// A plan and the manifest read in the same pass, so a report's owners come
/// from the state the plan's verdicts were decided against rather than from a
/// second read a concurrent writer could move between.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    /// What applying would do, per path.
    pub plan: Plan,
    /// The manifest the plan was decided against.
    pub manifest: Manifest,
}

impl Planned {
    /// The plan's rows, each carrying the owners this manifest records.
    pub fn report(&self) -> Report<PlannedAction> {
        self.plan.report(&self.manifest)
    }
}

#[cfg(test)]
#[path = "projection_tests.rs"]
mod tests;
