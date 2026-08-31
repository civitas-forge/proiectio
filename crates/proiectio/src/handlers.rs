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
use standout::cli::{CommandContext, Output};
use standout::handler;

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
        return planned_report(&planned.plan, planned.report(), ctx);
    }
    let mut run = projection.begin().map_err(exit::failure)?;
    run.plan(&owner, &desired, options)
        .map(|_| ())
        .map_err(exit::failure)?;
    apply(run, ctx)
}

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
    let owner = owner_or_configured(owner)?;
    let drift = drift(force);

    let projection = projection(&dest, state_dir.as_deref())?;
    if dry_run {
        let planned = projection
            .plan_removal(&owner, scope, drift)
            .map_err(exit::failure)?;
        return planned_report(&planned.plan, planned.report(), ctx);
    }
    let mut run = projection.begin().map_err(exit::failure)?;
    run.plan_removal(&owner, scope, drift)
        .map(|_| ())
        .map_err(exit::failure)?;
    apply(run, ctx)
}

/// The owner the invocation names, and otherwise the configured one.
fn owner_or_configured(owner: Option<String>) -> Result<String, anyhow::Error> {
    match owner {
        Some(named) => Ok(named),
        None => Ok(settings::builder().load()?.owner),
    }
}

/// The two settings a write layers a flag over: `--owner` above `owner`, and
/// `--max-source-size` above `max_source_size`. One load answers both, and a
/// run whose flags name both never reads the configuration at all.
fn write_settings(
    owner: Option<String>,
    max_source_size: Option<u64>,
) -> Result<(String, Limits), anyhow::Error> {
    let (owner, max_source_bytes) = match (owner, max_source_size) {
        (Some(owner), Some(bytes)) => (owner, bytes),
        (owner, max_source_size) => {
            let configured = settings::builder().load()?;
            (
                owner.unwrap_or(configured.owner),
                max_source_size.unwrap_or(configured.max_source_size),
            )
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

/// A run reports the whole plan, refused rows and all — a dry run because the
/// rows are what it is for, a real one because a plan that refuses acts on
/// nothing and has only the plan to report. Either way a refusal records the
/// status the run leaves with rather than replacing the report with a
/// diagnostic.
fn planned_report(
    plan: &Plan,
    report: Report<PlannedAction>,
    ctx: &CommandContext,
) -> Result<Output<RunView>, anyhow::Error> {
    let stated = PlannedRun {
        report,
        dropped: plan.dropped.clone(),
    };
    if plan.refusals().next().is_some() {
        return refusal(RunView::Planned(stated), ctx);
    }
    Ok(Output::Render(RunView::Planned(stated)))
}

/// A real run acts unless something refuses. A plan carrying refusals writes
/// nothing and reports itself, which is the document a dry run of the same
/// invocation reports; what a run that started applying reports is
/// [`stopped`]'s to say.
pub(crate) fn apply(run: Run, ctx: &CommandContext) -> Result<Output<RunView>, anyhow::Error> {
    if let Some(plan) = run
        .planned()
        .filter(|plan| plan.refusals().next().is_some())
    {
        return planned_report(plan, plan.report(run.manifest()), ctx);
    }
    match run.apply() {
        Ok(applied) => Ok(Output::Render(RunView::Applied(Box::new(applied)))),
        Err(aborted) => stopped(aborted, ctx),
    }
}

/// What a run that could not finish reports, which turns on whether it had
/// applied anything when it stopped.
///
/// One that had not states the refusal the way the planning stages state
/// theirs — the keys it declined, the archive members the plan stripped, and
/// nothing acted on — and a failure there replaces the output with its
/// diagnostic, the run having nothing to report. Such a run wrote no manifest
/// either, which is why only a stop at an action reaches that branch: a stop
/// naming the record has rows the record was written for.
///
/// One that had renders the rows it applied whatever stopped it, in the tense
/// it applied them in and under a document marked as stopped: a refusal adds
/// the keys it declined, a failure the diagnostic that would otherwise have
/// replaced the rows, and either adds what the state directory does not
/// record. A run part-way through a plan reading as a plan would claim a
/// destination nobody touched, which is the one thing this output must never
/// claim; dropping the rows for a failure would claim it just as loudly.
fn stopped(aborted: Box<Aborted>, ctx: &CommandContext) -> Result<Output<RunView>, anyhow::Error> {
    let Aborted { stopped, applied } = *aborted;
    match stopped {
        Stopped::Applying(error) if applied.report.is_empty() => {
            refusal_or_failure(error, &applied.manifest, applied.dropped, ctx)
        }
        stopped => {
            let refused = match stopped.error() {
                Error::Refused(refused) => refused_rows(refused, &applied.manifest),
                _ => Report::default(),
            };
            ctx.app_state
                .get_required::<exit::Verdict>()?
                .record(exit::of_error(stopped.error()));
            Ok(Output::Render(RunView::Aborted(Box::new(AbortedRun::new(
                applied, refused, &stopped,
            )))))
        }
    }
}

/// A refusal a run met without acting on anything states the keys it declined
/// and the archive members its plan stripped, on the terms a plan's own rows
/// are stated on; every other error replaces the output with its diagnostic.
///
/// The drops come from the plan the refusal cut short rather than from the
/// error, which names no archive: a mapping expanding one is stripped to
/// decide the plan, so a refusal met before the first action lands has drops
/// to state and a document omitting them would say the archive arrived whole.
///
/// Deciding and applying run back to back over one destination, so no command
/// line reaches an apply-time refusal on its own — the disk has to move in
/// between — and that half of the contract is driven from `app_tests` over
/// this function, which is what it is visible past this module for.
pub(crate) fn refusal_or_failure(
    error: Error,
    manifest: &Manifest,
    dropped: BTreeSet<Dropped>,
    ctx: &CommandContext,
) -> Result<Output<RunView>, anyhow::Error> {
    match error {
        Error::Refused(refused) => refusal(
            RunView::Planned(PlannedRun::refused(&refused, manifest, dropped)),
            ctx,
        ),
        failure => Err(exit::failure(failure)),
    }
}

/// Renders the rows a refusal leaves the run with, recording the refusal so
/// the process leaves with 2 though the run rendered rather than failed.
fn refusal(stated: RunView, ctx: &CommandContext) -> Result<Output<RunView>, anyhow::Error> {
    ctx.app_state
        .get_required::<exit::Verdict>()?
        .record(exit::REFUSAL);
    Ok(Output::Render(stated))
}

/// `--tree` names the tree, one positional a mapping file, two or more the
/// files to project under their own basenames.
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

#[handler]
pub(crate) fn status(
    #[arg] dest: String,
    #[arg(name = "state-dir")] state_dir: Option<String>,
) -> Result<Output<Status>, anyhow::Error> {
    let projection = projection(&dest, state_dir.as_deref())?;
    Ok(Output::Render(projection.status().map_err(exit::failure)?))
}

fn run_config(action: ConfigAction) -> Result<Output<ConfigView>, anyhow::Error> {
    settings::check_edit_path(&action).map_err(exit::failure)?;
    let result = settings::builder().handle(&action)?;
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
    settings::require_key(&key)?;
    run_config(ConfigAction::Get { key, scope })
}

#[handler]
pub(crate) fn config_set(
    #[arg] key: String,
    #[arg] value: String,
    #[arg] scope: Option<String>,
) -> Result<Output<ConfigView>, anyhow::Error> {
    settings::require_key(&key)?;
    run_config(ConfigAction::Set { key, value, scope })
}

#[handler]
pub(crate) fn config_unset(
    #[arg] key: String,
    #[arg] scope: Option<String>,
) -> Result<Output<ConfigView>, anyhow::Error> {
    settings::require_key(&key)?;
    run_config(ConfigAction::Unset { key, scope })
}

#[handler]
pub(crate) fn config_gen(
    #[arg] output: Option<Utf8PathBuf>,
) -> Result<Output<ConfigView>, anyhow::Error> {
    run_config(ConfigAction::Gen {
        output: output.map(Utf8PathBuf::into_std_path_buf),
    })
}

#[handler]
pub(crate) fn config_schema(
    #[arg] output: Option<Utf8PathBuf>,
) -> Result<Output<ConfigView>, anyhow::Error> {
    run_config(ConfigAction::Schema {
        output: output.map(Utf8PathBuf::into_std_path_buf),
    })
}

#[cfg(test)]
#[path = "handlers_tests.rs"]
mod tests;
