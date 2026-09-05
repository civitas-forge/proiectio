//! Thin CLI adapters between clap and `libproiectio`.

#![allow(non_snake_case)]

use std::collections::BTreeSet;

use camino::{Utf8Path, Utf8PathBuf};
use clapfig::ConfigAction;
use libproiectio::{
    Aborted, Desired, DriftPolicy, Dropped, Error, ExternalTargetPolicy, Limits, Manifest, Plan,
    PlanOptions, PlannedAction, Projection, RemovalScope, Report, Run, Status, Stopped, load_files,
    load_mapping, load_source,
};
use standout::cli::{CommandContext, CommandContextInput, ExitStatus, Output};
use standout::handler;

use crate::app::Forced;
use crate::exit;
use crate::settings;
use crate::views::{AbortedRun, ConfigView, PlannedRun, RunView, refused_rows};

#[handler]
#[expect(
    clippy::too_many_arguments,
    reason = "one parameter per option the command line carries"
)]
pub(crate) fn write(
    #[arg] dest: String,
    #[arg(name = "state-dir")] state_dir: Option<String>,
    #[arg] paths: Vec<Utf8PathBuf>,
    #[arg] tree: Option<Utf8PathBuf>,
    #[arg] strip: Option<u32>,
    #[arg] owner: Option<String>,
    #[arg(name = "max-source-size")] max_source_size: Option<u64>,
    #[flag(name = "dry-run")] dry_run: bool,
    #[flag] force: bool,
    #[flag(name = "allow-external-targets")] allow_external_targets: bool,
    #[ctx] ctx: &CommandContext,
) -> Result<Output<RunView>, anyhow::Error> {
    ctx.app_state.get_required::<Forced>()?.record(force);
    let (owner, limits) = write_settings(owner, max_source_size)?;
    let desired = desired(&paths, tree.as_deref(), strip, limits).map_err(exit::failure)?;
    let options = PlanOptions {
        drift: drift(force),
        external_targets: if allow_external_targets {
            ExternalTargetPolicy::Allow
        } else {
            ExternalTargetPolicy::Refuse
        },
    };

    let projection = projection(&dest, state_dir.as_deref())?;
    if dry_run {
        let planned = projection
            .plan(&owner, &desired, options)
            .map_err(exit::failure)?;
        return Ok(planned_report(&planned.plan, planned.report()));
    }
    let mut run = projection.begin().map_err(exit::failure)?;
    run.plan(&owner, &desired, options)
        .map(|_| ())
        .map_err(exit::failure)?;
    apply(run)
}

/// Removes what the manifest records, warning on stderr where the manifest
/// read empty because a named `--state-dir` is not there. The absence is read
/// before the run (a real removal creates the directory it was told to use)
/// and the warning written after it (a removal that could not open the
/// destination fails as an operational failure alone).
#[handler]
pub(crate) fn rm(
    #[arg] dest: String,
    #[arg(name = "state-dir")] state_dir: Option<String>,
    #[arg] paths: Vec<Utf8PathBuf>,
    #[arg] owner: Option<String>,
    #[flag(name = "dry-run")] dry_run: bool,
    #[flag] force: bool,
    #[ctx] ctx: &CommandContext,
) -> Result<Output<RunView>, anyhow::Error> {
    let named: BTreeSet<Utf8PathBuf> = paths.into_iter().collect();
    let scope = if named.is_empty() {
        RemovalScope::Everything
    } else {
        RemovalScope::Paths(&named)
    };
    ctx.app_state.get_required::<Forced>()?.record(force);
    let owner = owner_or_configured(owner)?;
    let drift = drift(force);

    let projection = projection(&dest, state_dir.as_deref())?;
    let named_but_absent = named_state_dir_is_absent(&projection, state_dir.as_deref())?;
    let reported = if dry_run {
        let planned = projection
            .plan_removal(&owner, scope, drift)
            .map_err(exit::failure)?;
        planned_report(&planned.plan, planned.report())
    } else {
        let mut run = projection.begin().map_err(exit::failure)?;
        run.plan_removal(&owner, scope, drift)
            .map(|_| ())
            .map_err(exit::failure)?;
        apply(run)?
    };
    if named_but_absent {
        warn_absent_state_dir(ctx, &projection);
    }
    Ok(reported)
}

/// Whether the command line named a state directory the filesystem does not
/// have. The default one's absence is not this: a destination nothing has
/// been projected onto has no state directory yet.
fn named_state_dir_is_absent(
    projection: &Projection,
    state_dir: Option<&str>,
) -> Result<bool, anyhow::Error> {
    match state_dir {
        Some(_) => Ok(!projection.state_dir_exists().map_err(exit::failure)?),
        None => Ok(false),
    }
}

fn warn_absent_state_dir(ctx: &CommandContext, projection: &Projection) {
    ctx.warn(exit::warning(&format!(
        "state dir {} does not exist; treating manifest as empty",
        projection.state_dir()
    )));
}

fn owner_or_configured(owner: Option<String>) -> Result<String, anyhow::Error> {
    match owner {
        Some(named) => Ok(named),
        None => settings::require_owner(settings::builder().load().map_err(exit::stated)?.owner),
    }
}

/// The two settings a write layers a flag over; a run whose flags name both
/// never reads the configuration at all.
fn write_settings(
    owner: Option<String>,
    max_source_size: Option<u64>,
) -> Result<(String, Limits), anyhow::Error> {
    let (owner, max_source_bytes) = match (owner, max_source_size) {
        (Some(owner), Some(bytes)) => (owner, bytes),
        (owner, max_source_size) => {
            let configured = settings::builder().load().map_err(exit::stated)?;
            let owner = match owner {
                Some(named) => named,
                None => settings::require_owner(configured.owner)?,
            };
            (owner, max_source_size.unwrap_or(configured.max_source_size))
        }
    };
    Ok((
        owner,
        Limits::default().with_max_source_bytes(max_source_bytes),
    ))
}

fn drift(force: bool) -> DriftPolicy {
    if force {
        DriftPolicy::Overwrite
    } else {
        DriftPolicy::Refuse
    }
}

fn projection(dest: &str, state_dir: Option<&str>) -> Result<Projection, anyhow::Error> {
    Projection::new(Utf8Path::new(dest), state_dir.map(Utf8Path::new)).map_err(exit::failure)
}

/// Reports the whole plan, refused rows and all; a refusal records the exit
/// status rather than replacing the report with a diagnostic.
fn planned_report(plan: &Plan, report: Report<PlannedAction>) -> Output<RunView> {
    let stated = PlannedRun {
        report,
        dropped: plan.dropped.clone(),
    };
    if plan.refusals().next().is_some() {
        return refusal(RunView::Planned(stated));
    }
    Output::Render(RunView::Planned(stated))
}

/// A real run acts unless something refuses: a plan carrying refusals writes
/// nothing and reports itself, as a dry run of the same invocation would.
pub(crate) fn apply(run: Run) -> Result<Output<RunView>, anyhow::Error> {
    if let Some(plan) = run
        .planned()
        .filter(|plan| plan.refusals().next().is_some())
    {
        return Ok(planned_report(plan, plan.report(run.manifest())));
    }
    match run.apply() {
        Ok(applied) => Ok(Output::Render(RunView::Applied(Box::new(applied.into())))),
        Err(aborted) => stopped(aborted),
    }
}

/// What a run that could not finish reports. One that applied nothing states
/// a refusal as the planning stages state theirs, and a failure replaces the
/// output with its diagnostic. One that applied rows renders them whatever
/// stopped it, under a document marked as stopped: this output must never
/// claim a destination nobody touched, and dropping applied rows for a
/// failure would claim it just as loudly.
fn stopped(aborted: Box<Aborted>) -> Result<Output<RunView>, anyhow::Error> {
    let Aborted { stopped, applied } = *aborted;
    match stopped {
        Stopped::Applying(error) if applied.report.is_empty() => {
            refusal_or_failure(error, &applied.manifest, applied.dropped)
        }
        stopped => {
            let refused = match stopped.error() {
                Error::Refused(refused) => refused_rows(refused, &applied.manifest),
                _ => Report::default(),
            };
            let status = ExitStatus::from(exit::of_error(stopped.error()));
            Ok(Output::Render(RunView::Aborted(Box::new(AbortedRun::new(
                applied, refused, &stopped,
            ))))
            .with_exit_status(status))
        }
    }
}

/// A refusal a run met without acting on anything states the keys it
/// declined on the terms a plan's own rows are stated on; every other error
/// replaces the output with its diagnostic. The drops come from the plan the
/// refusal cut short rather than from the error, which names no archive.
/// Visible past this module because no command line reaches an apply-time
/// refusal on its own — the disk has to move between plan and apply — so
/// `app_tests` drives that half of the contract through this function.
pub(crate) fn refusal_or_failure(
    error: Error,
    manifest: &Manifest,
    dropped: BTreeSet<Dropped>,
) -> Result<Output<RunView>, anyhow::Error> {
    match error {
        Error::Refused(refused) => Ok(refusal(RunView::Planned(PlannedRun::refused(
            &refused, manifest, dropped,
        )))),
        failure => Err(exit::failure(failure)),
    }
}

/// Renders the refusal's rows, declaring exit status 2 though the run
/// rendered rather than failed: a refused plan is the whole point of the run,
/// so the handler renders it and Standout spends the status on the process.
fn refusal(stated: RunView) -> Output<RunView> {
    Output::Render(stated).with_exit_status(ExitStatus::from(exit::REFUSAL))
}

fn desired(
    paths: &[Utf8PathBuf],
    tree: Option<&Utf8Path>,
    strip: Option<u32>,
    limits: Limits,
) -> libproiectio::Result<Desired> {
    match (tree, paths) {
        (Some(tree), _) => load_source(tree, strip, limits),
        (None, [mapping]) => load_mapping(mapping, limits),
        (None, files) => load_files(files, limits),
    }
}

/// Classifies the destination, and under `--check` records the exit status.
/// A named `--state-dir` that is not there reads as the empty manifest and
/// warns; `--check` spends the refusal status on it too, so a gate fails on
/// a misspelled state directory as it fails on drift.
#[handler]
pub(crate) fn status(
    #[arg] dest: String,
    #[arg(name = "state-dir")] state_dir: Option<String>,
    #[flag] check: bool,
    #[ctx] ctx: &CommandContext,
) -> Result<Output<Status>, anyhow::Error> {
    let projection = projection(&dest, state_dir.as_deref())?;
    let classified = projection.status().map_err(exit::failure)?;
    let named_but_absent = named_state_dir_is_absent(&projection, state_dir.as_deref())?;
    if named_but_absent {
        warn_absent_state_dir(ctx, &projection);
    }
    let refused = check && (named_but_absent || !classified.is_clean());
    let stated = Output::Render(classified);
    Ok(match refused {
        true => stated.with_exit_status(ExitStatus::from(exit::REFUSAL)),
        false => stated,
    })
}

fn run_config(action: ConfigAction) -> Result<Output<ConfigView>, anyhow::Error> {
    settings::check_edit_path(&action).map_err(exit::failure)?;
    let result = settings::builder().handle(&action).map_err(exit::stated)?;
    ConfigView::of(result, settings::persisted_edit)
        .map(Output::Render)
        .map_err(exit::failure)
}

#[handler]
pub(crate) fn config_root(
    #[arg] scope: Option<String>,
) -> Result<Output<ConfigView>, anyhow::Error> {
    run_config(ConfigAction::List { scope })
}

#[handler]
pub(crate) fn config_list(
    #[arg] scope: Option<String>,
) -> Result<Output<ConfigView>, anyhow::Error> {
    run_config(ConfigAction::List { scope })
}

#[handler]
pub(crate) fn config_get(
    #[arg] key: String,
    #[arg] scope: Option<String>,
) -> Result<Output<ConfigView>, anyhow::Error> {
    settings::require_key(&key).map_err(exit::stated)?;
    run_config(ConfigAction::Get { key, scope })
}

#[handler]
pub(crate) fn config_set(
    #[arg] key: String,
    #[arg] value: String,
    #[arg] scope: Option<String>,
) -> Result<Output<ConfigView>, anyhow::Error> {
    settings::require_key(&key).map_err(exit::stated)?;
    settings::require_value(&key, &value)?;
    run_config(ConfigAction::Set { key, value, scope })
}

#[handler]
pub(crate) fn config_unset(
    #[arg] key: String,
    #[arg] scope: Option<String>,
) -> Result<Output<ConfigView>, anyhow::Error> {
    settings::require_key(&key).map_err(exit::stated)?;
    run_config(ConfigAction::Unset { key, scope })
}

#[handler]
pub(crate) fn config_gen(
    #[arg] output: Option<Utf8PathBuf>,
    #[flag] force: bool,
) -> Result<Output<ConfigView>, anyhow::Error> {
    run_config(ConfigAction::Gen {
        output: output.map(Utf8PathBuf::into_std_path_buf),
        force,
    })
}

#[handler]
pub(crate) fn config_schema(
    #[arg] output: Option<Utf8PathBuf>,
    #[flag] force: bool,
) -> Result<Output<ConfigView>, anyhow::Error> {
    run_config(ConfigAction::Schema {
        output: output.map(Utf8PathBuf::into_std_path_buf),
        force,
    })
}

#[cfg(test)]
#[path = "handlers_tests.rs"]
mod tests;
