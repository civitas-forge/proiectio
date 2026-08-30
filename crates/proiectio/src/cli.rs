//! The clap surface: the one place every command and option is named.

use clap::{CommandFactory, Parser, Subcommand};
use clapfig::ConfigCommand;

#[derive(Parser)]
#[command(name = "proiectio", about = "Projects files onto a directory")]
pub(crate) struct Cli {
    /// Target directory; default cwd.
    #[arg(long, global = true, default_value = ".", value_name = "DIR")]
    pub(crate) dest: String,

    /// Manifest location; default .proiectio inside the destination.
    #[arg(long, id = "state-dir", global = true, value_name = "DIR")]
    pub(crate) state_dir: Option<String>,

    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Classifies every recorded path, writing nothing.
    Status,
}

/// Clapfig's config command group (list, get, set, unset, gen, schema).
///
/// Standout owns the global `--output` flag, so the file destination on
/// `gen`/`schema` is spelled `--file`/`-f`.
pub(crate) fn config_command() -> ConfigCommand {
    ConfigCommand::new()
        .output_long("file")
        .output_short(Some('f'))
}

pub(crate) fn command() -> clap::Command {
    Cli::command().subcommand(
        config_command()
            .as_command("config")
            .visible_alias("conf")
            .long_about(CONFIG_ABOUT),
    )
}

const CONFIG_ABOUT: &str = "\
Reads, writes and documents proiectio's configuration.

Values resolve as compiled defaults, then files, then PROIECTIO__* \
environment variables. The `user` scope is the platform config directory, \
which is also the search path.

A key is a dotted path to a single value: owner.";

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
