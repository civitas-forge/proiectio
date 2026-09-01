use camino::{Utf8Path, Utf8PathBuf};

use std::collections::BTreeSet;
use std::io::ErrorKind::NotFound;

use cap_std::ambient_authority;
use cap_std::fs_utf8::Dir;

use crate::{
    BlockMarkers, Desired, DriftPolicy, Error, IoRole, Manifest, Plan, PlanOptions, PlannedAction,
    RemovalScope, Report, Result, StateDir, Status, absolutize, block_markers, decide,
    decide_removal, load_manifest, observe_scoped, require_owner, status,
};

const DEFAULT_STATE_DIR: &str = ".proiectio";

/// A destination directory paired with the state directory holding its
/// manifest. The reads here return reports nothing can apply;
/// [`begin`](Projection::begin) returns the [`Run`](crate::Run) that writes.
///
/// `state_dir` may lie inside `target` as a proper subdirectory: that
/// subtree is excluded from classification, and a desired path overlapping
/// it refuses as [`Containment`](crate::Refusal::Containment).
/// [`with_pruned_components`](Projection::with_pruned_components) lets the
/// caller exclude component names at every depth, with no default exclusions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    target: Utf8PathBuf,
    state_dir: Utf8PathBuf,
    pruned_components: BTreeSet<String>,
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
        Ok(Projection {
            target,
            state_dir,
            pruned_components: BTreeSet::new(),
        })
    }

    /// Sets the path-component names that this projection never enters.
    ///
    /// Matching is component-wise at every depth: pruning `.git` excludes
    /// both `.git/config` and `vendor/project/.git/config`, without excluding
    /// `.github/workflows`. Pruned paths are not observed or reported, and a
    /// plan refuses any desired or removal path that enters one. The manifest
    /// may not record a path inside one.
    ///
    /// A component is one non-empty Unix filename other than `.` or `..`;
    /// `/` and NUL are not allowed. An in-target state directory may not enter
    /// a pruned component. Calling this method replaces the previous set.
    /// [`Projection::new`] starts with an empty set.
    pub fn with_pruned_components<I, S>(mut self, components: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut pruned = BTreeSet::new();
        for component in components {
            let component = component.as_ref();
            if !valid_pruned_component(component) {
                return Err(Error::InvalidPrunedComponent {
                    component: component.to_owned(),
                });
            }
            pruned.insert(component.to_owned());
        }
        if let Some(state_prefix) = self.state_prefix() {
            if let Some(component) = state_prefix
                .components()
                .find(|component| pruned.contains(component.as_str()))
            {
                return Err(Error::StateDirPruned {
                    path: self.state_dir.clone(),
                    component: component.as_str().to_owned(),
                });
            }
        }
        self.pruned_components = pruned;
        Ok(self)
    }

    /// The path-component names this projection never enters, in lexical
    /// order. An empty set means the whole destination is observed.
    pub fn pruned_components(&self) -> &BTreeSet<String> {
        &self.pruned_components
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
    pub(crate) fn state_prefix(&self) -> Option<&Utf8Path> {
        match self.state_dir.strip_prefix(&self.target) {
            Ok(prefix) if !prefix.as_str().is_empty() => Some(prefix),
            _ => None,
        }
    }

    pub(crate) fn validate_manifest_scope(&self, manifest: Manifest) -> Result<Manifest> {
        if let Some(path) = manifest
            .entries
            .keys()
            .find(|path| is_pruned(path, &self.pruned_components))
        {
            return Err(Error::ManifestPathPruned { path: path.clone() });
        }
        Ok(manifest)
    }
}

pub(crate) fn is_pruned(path: &Utf8Path, pruned_components: &BTreeSet<String>) -> bool {
    path.components()
        .any(|component| pruned_components.contains(component.as_str()))
}

fn valid_pruned_component(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && !component.contains('/')
        && !component.contains('\0')
}

/// The reads: each call opens the destination and the recorded state, takes
/// no lock, and drops both handles before it returns.
impl Projection {
    /// The [`Status`] of the in-scope destination, with nothing written. A
    /// missing state directory or manifest reads as the empty [`Manifest`].
    pub fn status(&self) -> Result<Status> {
        let dest = self.open_target()?;
        let manifest = self.manifest_under(&dest)?;
        let observations = observe_scoped(
            &dest,
            &manifest,
            &BlockMarkers::new(),
            &self.pruned_components,
        )?;
        Ok(status(&manifest, &observations, self.state_prefix()))
    }

    /// Whether the directory holding the manifest is there.
    ///
    /// Every read treats an absent state directory as the empty
    /// [`Manifest`], which is right for a destination nothing has been
    /// projected onto and wrong for a caller who named the directory and
    /// misspelled it. The two reports are identical, so this states the fact
    /// and leaves the caller — who knows which of the two it named — to
    /// decide what the fact means.
    pub fn state_dir_exists(&self) -> Result<bool> {
        match self.open_state(None) {
            Some(state) => state.map(|_| true),
            None => Ok(false),
        }
    }

    /// The recorded state: what the projection wrote, per path, with its
    /// owners. A missing state directory or manifest file reads as the empty
    /// [`Manifest`].
    pub fn manifest(&self) -> Result<Manifest> {
        match self.open_state(None) {
            Some(state) => self.validate_manifest_scope(load_manifest(&state?)?),
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
            Some(state) => self.validate_manifest_scope(load_manifest(&state?)?),
            None => Ok(Manifest::new()),
        }
    }

    /// Every write, overwrite, removal, and refusal applying `desired` under
    /// `owner` would perform, decided outside the single-writer guard and so
    /// not applicable, with the manifest it was decided against. An empty
    /// tree plans a removal; `origin` is named by every refusal the plan
    /// carries. A name that is not an owner ([`OWNER_RULE`](crate::OWNER_RULE))
    /// fails with [`Error::OwnerNotNamed`] before the destination is opened.
    pub fn plan(&self, owner: &str, desired: &Desired, options: PlanOptions) -> Result<Planned> {
        require_owner(owner)?;
        let dest = self.open_target()?;
        let manifest = self.manifest_under(&dest)?;
        let observations = observe_scoped(
            &dest,
            &manifest,
            &block_markers(desired),
            &self.pruned_components,
        )?;
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
    /// `scope` names ([`RemovalScope`]), with the manifest it was decided
    /// against; a report, on the same terms as [`plan`](Projection::plan).
    pub fn plan_removal(
        &self,
        owner: &str,
        scope: RemovalScope<'_>,
        drift: DriftPolicy,
    ) -> Result<Planned> {
        require_owner(owner)?;
        let dest = self.open_target()?;
        let manifest = self.manifest_under(&dest)?;
        let observations = observe_scoped(
            &dest,
            &manifest,
            &BlockMarkers::new(),
            &self.pruned_components,
        )?;
        let plan = decide_removal(
            owner,
            scope,
            &manifest,
            &observations,
            self.state_prefix(),
            drift,
        );
        Ok(Planned { plan, manifest })
    }

    /// A handle on the destination directory, which must already exist.
    pub(crate) fn open_target(&self) -> Result<Dir> {
        Dir::open_ambient_dir(&self.target, ambient_authority()).map_err(|source| Error::Io {
            role: IoRole::Destination,
            path: self.target.clone(),
            source,
        })
    }

    /// A handle on the state directory, or `None` where there is none yet.
    /// An in-dest state directory is opened through `dest` where the caller
    /// holds one, and otherwise through a fresh open of the target, whose
    /// absence also reads as `None`.
    fn open_state(&self, dest: Option<&Dir>) -> Option<Result<StateDir>> {
        let Some(prefix) = self.state_prefix() else {
            return StateDir::open(&self.state_dir);
        };
        match dest {
            Some(dest) => StateDir::open_under(dest, &self.target, prefix),
            None => match self.open_target() {
                Ok(dest) => StateDir::open_under(&dest, &self.target, prefix),
                Err(Error::Io { source, .. }) if source.kind() == NotFound => None,
                Err(error) => Some(Err(error)),
            },
        }
    }
}

/// The plan a read decided and the manifest it was decided against, so a
/// caller renders the report from what the verdicts were classified against
/// rather than reading the manifest again.
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
