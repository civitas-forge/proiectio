//! Thin CLI adapters between clap and `libproiectio`.

#![allow(non_snake_case)]

use std::collections::BTreeSet;

use camino::{Utf8Path, Utf8PathBuf};
use clapfig::ConfigAction;
use libproiectio::{
    Desired, DriftPolicy, Error, ExternalTargetPolicy, Plan, PlanOptions, PlannedAction,
    Projection, RemovalScope, Report, Run, Status, load_files, load_mapping, load_source,
};
use standout::cli::{CommandContext, Output};
use standout::handler;

use crate::exit;
use crate::settings;
use crate::views::{ConfigView, PlannedRun, RunView};

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
    #[flag(name = "dry-run")] dry_run: bool,
    #[flag] force: bool,
    #[flag(name = "allow-external-targets")] allow_external_targets: bool,
    #[ctx] ctx: &CommandContext,
) -> Result<Output<RunView>, anyhow::Error> {
    let desired = desired(&paths, tree.as_deref(), strip).map_err(exit::failure)?;
    let owner = owner_or_configured(owner)?;
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
        return match projection.plan(&owner, &desired, options) {
            Ok(planned) => planned_report(&planned.plan, planned.report(), ctx),
            Err(error) => refusal_or_failure(error, ctx),
        };
    }
    let mut run = projection.begin().map_err(exit::failure)?;
    if let Err(error) = run.plan(&owner, &desired, options).map(|_| ()) {
        return refusal_or_failure(error, ctx);
    }
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
        return match projection.plan_removal(&owner, scope, drift) {
            Ok(planned) => planned_report(&planned.plan, planned.report(), ctx),
            Err(error) => refusal_or_failure(error, ctx),
        };
    }
    let mut run = projection.begin().map_err(exit::failure)?;
    if let Err(error) = run.plan_removal(&owner, scope, drift).map(|_| ()) {
        return refusal_or_failure(error, ctx);
    }
    apply(run, ctx)
}

/// The owner the invocation names, and otherwise the configured one.
fn owner_or_configured(owner: Option<String>) -> Result<String, anyhow::Error> {
    match owner {
        Some(named) => Ok(named),
        None => Ok(settings::builder().load()?.owner),
    }
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
        return refusal(stated, ctx);
    }
    Ok(Output::Render(RunView::Planned(stated)))
}

/// A real run acts unless something refuses. A plan carrying refusals writes
/// nothing and reports itself, which is the document a dry run of the same
/// invocation reports; a refusal apply meets past that reports the keys the
/// error names and no others, the error being all the run knows by then.
fn apply(run: Run, ctx: &CommandContext) -> Result<Output<RunView>, anyhow::Error> {
    if let Some(plan) = run
        .planned()
        .filter(|plan| plan.refusals().next().is_some())
    {
        return planned_report(plan, plan.report(run.manifest()), ctx);
    }
    match run.apply() {
        Ok(applied) => Ok(Output::Render(RunView::Applied(Box::new(applied)))),
        Err(error) => refusal_or_failure(error, ctx),
    }
}

/// A refusal the library reports as an error states the keys it declined, on
/// the terms a plan's own refused rows are stated on; every other error
/// replaces the output with its diagnostic.
///
/// Deciding and applying run back to back over one destination, so no command
/// line reaches an apply-time refusal on its own — the disk has to move in
/// between — and that half of the contract is driven from `app_tests` over
/// this function, which is what it is visible past this module for.
pub(crate) fn refusal_or_failure(
    error: Error,
    ctx: &CommandContext,
) -> Result<Output<RunView>, anyhow::Error> {
    match error {
        Error::Refused(refused) => refusal(PlannedRun::refused(&refused), ctx),
        failure => Err(exit::failure(failure)),
    }
}

/// Renders the rows a refusal leaves the run with, recording the refusal so
/// the process leaves with 2 though the run rendered rather than failed.
fn refusal(stated: PlannedRun, ctx: &CommandContext) -> Result<Output<RunView>, anyhow::Error> {
    ctx.app_state
        .get_required::<exit::Verdict>()?
        .record(exit::REFUSAL);
    Ok(Output::Render(RunView::Planned(stated)))
}

/// `--tree` names the tree, one positional a mapping file, two or more the
/// files to project under their own basenames.
fn desired(
    paths: &[Utf8PathBuf],
    tree: Option<&Utf8Path>,
    strip: Option<u32>,
) -> libproiectio::Result<Desired> {
    match (tree, paths) {
        (Some(tree), _) => load_source(tree, strip),
        (None, [mapping]) => load_mapping(mapping),
        (None, files) => load_files(files),
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
    let result = settings::builder().handle(&action)?;
    ConfigView::try_from(result)
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
    run_config(ConfigAction::Get { key, scope })
}

#[handler]
pub(crate) fn config_set(
    #[arg] key: String,
    #[arg] value: String,
    #[arg] scope: Option<String>,
) -> Result<Output<ConfigView>, anyhow::Error> {
    run_config(ConfigAction::Set { key, value, scope })
}

#[handler]
pub(crate) fn config_unset(
    #[arg] key: String,
    #[arg] scope: Option<String>,
) -> Result<Output<ConfigView>, anyhow::Error> {
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
