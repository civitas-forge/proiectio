use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use cap_std::ambient_authority;
use cap_std::fs_utf8::Dir;

use crate::{
    ApplyReport, Entry, Error, Manifest, Origin, Plan, PlanOptions, Projection, RemovalScope,
    Result, StateLock, apply, decide, decide_removal, load_manifest, observe,
};

/// One write pass over a projection, holding the single-writer guard from
/// [`Projection::begin`] until it is dropped (`docs/design.lex` section 3).
///
/// A run decides and executes under one guard, and it can execute nothing
/// else: [`apply`](Run::apply) takes no plan, so the only plan it can run is
/// one [`plan`](Run::plan) or [`plan_removal`](Run::plan_removal) decided
/// here, from the manifest this run loaded. Deciding a plan outside the guard
/// and applying it inside — where the plan says `Remove` for a path a
/// concurrent run has since given a second owner, and applying it deletes
/// that owner's file — is not a hazard to document but a shape that does not
/// compile.
///
/// The guard lasts the whole life of the run, so a caller that prompts a
/// human between deciding and applying holds it across the prompt and other
/// runs meet [`Error::LockHeld`]. Read
/// [`Projection::plan`] instead where a plan is only to be shown.
///
/// # The critical section
///
/// [`begin`](Projection::begin) opens the destination, creates and opens the
/// state directory, takes the lock, and loads the manifest — in that order.
/// The manifest's read-modify-write starts at the load, so the load is inside
/// the guard: a run that loaded first would persist over whatever a writer
/// that finished in between had recorded. The section ends when the run is
/// dropped, which is after [`apply`](Run::apply) has persisted the manifest.
///
/// # What a refusal carries
///
/// Applying a plan that carries any refusal executes nothing and returns the
/// matching refusal variant of [`Error`], aggregating every refused path.
/// The four whose offending value the desired tree chose —
/// [`Containment`](Error::Containment),
/// [`TreeConflict`](Error::TreeConflict),
/// [`ExternalTarget`](Error::ExternalTarget) and
/// [`InvalidTarget`](Error::InvalidTarget) — name the [`Origin`] the plan was
/// decided with, so the message says which mapping, tree, or archive to go
/// and edit. Everything else on [`Error`] is a runtime failure;
/// [`Error::is_refusal`] is the split a CLI's 0/1/2 exit contract matches on.
#[derive(Debug)]
pub struct Run {
    projection: Projection,
    dest: Dir,
    state: Dir,
    /// Held for the run's lifetime; dropping it releases the lock.
    _lock: StateLock,
    manifest: Manifest,
    plan: Option<Plan>,
}

impl Projection {
    /// Starts a write pass: opens the destination, creates and opens the
    /// state directory, takes the single-writer lock, and loads the manifest
    /// — in that order ([`Run`]).
    ///
    /// The destination must already exist. The state directory is created if
    /// it does not, together with any parent of it inside the target, because
    /// a first run has nothing to record into yet.
    ///
    /// Fails with [`Error::LockHeld`] where another writer holds the lock:
    /// acquisition is try-lock, never blocking.
    pub fn begin(&self) -> Result<Run> {
        let dest = self.open_target()?;
        let state = self.open_or_create_state()?;
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
    /// above it — where a first run finds none.
    fn open_or_create_state(&self) -> Result<Dir> {
        let io = |source| Error::Io {
            path: self.state_dir().to_owned(),
            source,
        };
        if let Err(source) = std::fs::create_dir_all(self.state_dir()) {
            return Err(io(source));
        }
        Dir::open_ambient_dir(self.state_dir(), ambient_authority()).map_err(io)
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
    /// as the plan [`apply`](Run::apply) will execute.
    ///
    /// `origin` says where the tree came from; every refusal names it
    /// ([`Origin`]). Deciding again replaces the kept plan, over the same
    /// manifest and a fresh observation of the destination.
    pub fn plan(
        &mut self,
        owner: &str,
        desired: &BTreeMap<Utf8PathBuf, Entry>,
        origin: Origin,
        options: PlanOptions,
    ) -> Result<&Plan> {
        let observations = observe(&self.dest, &self.manifest)?;
        self.plan = Some(decide(
            owner,
            desired,
            origin,
            &self.manifest,
            &observations,
            self.projection.state_prefix(),
            options,
        ));
        Ok(self.planned().expect("just decided"))
    }

    /// Decides the removal of everything `owner` holds, or of the recorded
    /// paths `scope` names ([`RemovalScope`]), and keeps it as the plan
    /// [`apply`](Run::apply) will execute.
    pub fn plan_removal(
        &mut self,
        owner: &str,
        scope: RemovalScope<'_>,
        options: PlanOptions,
    ) -> Result<&Plan> {
        let observations = observe(&self.dest, &self.manifest)?;
        self.plan = Some(decide_removal(
            owner,
            scope,
            &self.manifest,
            &observations,
            self.projection.state_prefix(),
            options,
        ));
        Ok(self.planned().expect("just decided"))
    }

    /// The plan this run decided, or `None` before it has decided one.
    pub fn planned(&self) -> Option<&Plan> {
        self.plan.as_ref()
    }

    /// Executes the plan this run decided and persists the manifest,
    /// releasing the guard as the run is consumed.
    ///
    /// There is no plan parameter, and that is the point: a `Run` executes
    /// only what it decided itself, from the manifest it loaded under its own
    /// guard ([`Run`]).
    ///
    /// ```no_run
    /// # use camino::Utf8PathBuf;
    /// # use libproiectio::{Origin, PlanOptions, Projection, Result};
    /// # fn write(projection: &Projection) -> Result<()> {
    /// let desired = Default::default();
    /// let mut run = projection.begin()?;
    /// run.plan("harness", &desired, Origin::Caller, PlanOptions::default())?;
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
    /// # use libproiectio::{Origin, PlanOptions, Projection, Result};
    /// # fn write(projection: &Projection) -> Result<()> {
    /// let desired = Default::default();
    /// let plan = projection.plan("harness", &desired, Origin::Caller, PlanOptions::default())?;
    /// let run = projection.begin()?;
    /// let report = run.apply(&plan)?;
    /// # let _ = report;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// A run that decided no plan applies the empty one: nothing is executed,
    /// nothing is written, and the report carries the manifest as loaded.
    pub fn apply(self) -> Result<ApplyReport> {
        let Some(plan) = &self.plan else {
            return Ok(ApplyReport {
                outcomes: BTreeMap::new(),
                manifest: self.manifest,
            });
        };
        apply(&self.dest, &self.state, &self.manifest, plan)
    }
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
