//! Thin CLI adapters between clap and `libproiectio`.

#![allow(non_snake_case)]

use std::collections::BTreeSet;

use camino::{Utf8Path, Utf8PathBuf};
use clapfig::ConfigAction;
use libproiectio::{
    Desired, DriftPolicy, Error, ExternalTargetPolicy, Limits, Plan, PlanOptions, PlannedAction,
    Projection, RemovalScope, Report, Status, load_files, load_mapping, load_source,
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
    let plan = run.plan(&owner, &desired, options).map_err(exit::failure)?;
    refusals(plan)?;
    run.apply()
        .map(|applied| Output::Render(RunView::Applied(Box::new(applied))))
        .map_err(exit::failure)
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
    let plan = run
        .plan_removal(&owner, scope, drift)
        .map_err(exit::failure)?;
    refusals(plan)?;
    run.apply()
        .map(|applied| Output::Render(RunView::Applied(Box::new(applied))))
        .map_err(exit::failure)
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
    Ok((owner, Limits { max_source_bytes }))
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

/// A dry run reports the whole plan, refused rows and all: the rows are what
/// the run is for, so a refusal records the status the run leaves with rather
/// than replacing the report with a diagnostic.
fn planned_report(
    plan: &Plan,
    report: Report<PlannedAction>,
    ctx: &CommandContext,
) -> Result<Output<RunView>, anyhow::Error> {
    if plan.refusals().next().is_some() {
        ctx.app_state
            .get_required::<exit::Verdict>()?
            .record(exit::REFUSAL);
    }
    Ok(Output::Render(RunView::Planned(PlannedRun {
        report,
        dropped: plan.dropped.clone(),
    })))
}

/// A real run acts, so a plan carrying refusals reaches the shell as
/// `Error::Refused`, which spends the refusal status; one carrying none
/// passes.
fn refusals(plan: &Plan) -> Result<(), anyhow::Error> {
    match plan.refused() {
        Some(refused) => Err(exit::failure(Error::Refused(refused))),
        None => Ok(()),
    }
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
