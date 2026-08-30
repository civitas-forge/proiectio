use std::collections::BTreeSet;

use cap_std::ambient_authority;
use cap_std::fs_utf8::Dir;

use crate::{
    Aborted, ApplyReport, BlockMarkers, Desired, DriftPolicy, Error, Manifest, Plan, PlanOptions,
    Projection, RemovalScope, Report, Result, StateLock, apply, block_markers, decide,
    decide_removal, load_manifest, observe,
};

/// One write pass over a projection, holding the single-writer guard from
/// [`Projection::begin`] until it is dropped — other runs meet
/// [`Error::LockHeld`] meanwhile. [`apply`](Run::apply) takes no plan, so a
/// run executes only what [`plan`](Run::plan) or
/// [`plan_removal`](Run::plan_removal) decided here.
#[derive(Debug)]
pub struct Run {
    projection: Projection,
    dest: Dir,
    state: Dir,
    _lock: StateLock,
    manifest: Manifest,
    plan: Option<Plan>,
}

impl Projection {
    /// Starts a write pass: opens the destination, which must already exist,
    /// creates and opens the state directory, takes the single-writer lock,
    /// and loads the manifest. Fails with [`Error::LockHeld`] where another
    /// writer holds the lock; acquisition never blocks.
    pub fn begin(&self) -> Result<Run> {
        let dest = self.open_target()?;
        let state = self.open_or_create_state(&dest)?;
        let lock = StateLock::acquire(&state)?;
        let manifest = load_manifest(&state)?;
        Ok(Run {
            projection: self.clone(),
            dest,
            state,
            _lock: lock,
            manifest,
            plan: None,
        })
    }

    /// A handle on the state directory, creating it — and the directories
    /// above it — where a first run finds none. An in-dest state directory
    /// is created and opened through `dest` rather than against ambient
    /// authority.
    fn open_or_create_state(&self, dest: &Dir) -> Result<Dir> {
        let io = |source| Error::Io {
            path: self.state_dir().to_owned(),
            source,
        };
        match self.state_prefix() {
            Some(prefix) => {
                dest.create_dir_all(prefix).map_err(io)?;
                dest.open_dir(prefix).map_err(io)
            }
            None => {
                std::fs::create_dir_all(self.state_dir()).map_err(io)?;
                Dir::open_ambient_dir(self.state_dir(), ambient_authority()).map_err(io)
            }
        }
    }
}

impl Run {
    /// The projection this run writes into.
    pub fn projection(&self) -> &Projection {
        &self.projection
    }

    /// The manifest as this run loaded it, under its own guard.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Decides what applying `desired` under `owner` would do, and keeps it
    /// as the plan [`apply`](Run::apply) will execute; `origin` is named by
    /// every refusal. Deciding again discards the kept plan first, so a
    /// decision that fails partway leaves the run with no plan.
    pub fn plan(&mut self, owner: &str, desired: &Desired, options: PlanOptions) -> Result<&Plan> {
        self.plan = None;
        let observations = observe(&self.dest, &self.manifest, &block_markers(desired))?;
        self.plan = Some(decide(
            owner,
            desired,
            &self.manifest,
            &observations,
            self.projection.state_prefix(),
            options,
        )?);
        Ok(self.planned().expect("just decided"))
    }

    /// Decides the removal of everything `owner` holds, or of the recorded
    /// paths `scope` names ([`RemovalScope`]), and keeps it as the plan
    /// [`apply`](Run::apply) will execute, on the same terms as
    /// [`plan`](Run::plan).
    pub fn plan_removal(
        &mut self,
        owner: &str,
        scope: RemovalScope<'_>,
        drift: DriftPolicy,
    ) -> Result<&Plan> {
        self.plan = None;
        let observations = observe(&self.dest, &self.manifest, &BlockMarkers::new())?;
        self.plan = Some(decide_removal(
            owner,
            scope,
            &self.manifest,
            &observations,
            self.projection.state_prefix(),
            drift,
        ));
        Ok(self.planned().expect("just decided"))
    }

    /// The plan this run decided, or `None` before it has decided one.
    pub fn planned(&self) -> Option<&Plan> {
        self.plan.as_ref()
    }

    /// Executes the plan this run decided and persists the manifest,
    /// releasing the guard as the run is consumed. A run that stops part-way
    /// fails with [`Aborted`], which carries the rows it applied before it
    /// stopped.
    ///
    /// ```no_run
    /// # use camino::Utf8PathBuf;
    /// # use libproiectio::{Desired, PlanOptions, Projection};
    /// # fn write(projection: &Projection) -> Result<(), Box<dyn std::error::Error>> {
    /// let desired = Desired::new();
    /// let mut run = projection.begin()?;
    /// run.plan("harness", &desired, PlanOptions::default())?;
    /// let report = run.apply()?;
    /// # let _ = report;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// The same body handing `apply` a plan decided elsewhere does not
    /// compile:
    ///
    /// ```compile_fail
    /// # use camino::Utf8PathBuf;
    /// # use libproiectio::{Desired, PlanOptions, Projection};
    /// # fn write(projection: &Projection) -> Result<(), Box<dyn std::error::Error>> {
    /// let desired = Desired::new();
    /// let plan = projection.plan("harness", &desired, PlanOptions::default())?.plan;
    /// let run = projection.begin()?;
    /// let report = run.apply(&plan)?;
    /// # let _ = report;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// A run that decided no plan writes nothing and reports the manifest as
    /// loaded.
    pub fn apply(self) -> std::result::Result<ApplyReport, Aborted> {
        let Some(plan) = &self.plan else {
            return Ok(ApplyReport {
                report: Report::default(),
                dropped: BTreeSet::new(),
                manifest: self.manifest,
            });
        };
        apply(&self.dest, &self.state, &self.manifest, plan)
    }
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
