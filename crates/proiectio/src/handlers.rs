//! Thin CLI adapters between clap and `libproiectio`.

#![allow(non_snake_case)]

use camino::{Utf8Path, Utf8PathBuf};
use clapfig::ConfigAction;
use libproiectio::{Projection, Status};
use standout::cli::Output;
use standout::handler;

use crate::exit;
use crate::settings;
use crate::views::ConfigView;

#[handler]
pub(crate) fn status(
    #[arg] dest: String,
    #[arg(name = "state-dir")] state_dir: Option<String>,
) -> Result<Output<Status>, anyhow::Error> {
    let projection = Projection::new(
        Utf8Path::new(&dest),
        state_dir.as_deref().map(Utf8Path::new),
    )
    .map_err(exit::failure)?;
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
