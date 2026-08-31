//! The clap surface: the one place every command and option is named.

use camino::Utf8PathBuf;
use clap::{CommandFactory, Parser, Subcommand};
use clapfig::ConfigCommand;

#[derive(Parser)]
#[command(
    name = "proiectio",
    about = "Projects files onto a directory",
    long_about = "Projects files onto a directory.\n\n\
        Exit codes: 0 success, 1 error, 2 refused. A refusal (2) is a deliberate \
        safety \"no\" — drift, a foreign path, or a containment violation — and is \
        distinct from an error (1). Where a refusal has an override, re-run with \
        --force (drift) or --allow-external-targets (a symlink leaving the destination). \
        `status --check` reports those same findings on the same 2 without acting on them."
)]
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
    /// Projects a mapping file, loose files, or a tree onto the destination.
    #[command(long_about = WRITE_ABOUT)]
    Write {
        /// A mapping file, or two or more files to project by basename.
        #[arg(
            value_name = "PATH",
            required_unless_present = "tree",
            conflicts_with = "tree",
            value_parser = clap::value_parser!(Utf8PathBuf)
        )]
        paths: Vec<Utf8PathBuf>,

        /// Project a directory or an archive as the desired tree.
        #[arg(long, value_name = "PATH", value_parser = clap::value_parser!(Utf8PathBuf))]
        tree: Option<Utf8PathBuf>,

        /// Drop N leading path components from archive members (archives only,
        /// not directory trees).
        #[arg(long, value_name = "N", requires = "tree", conflicts_with = "paths")]
        strip: Option<u32>,

        /// Manifest owner; default from the configuration.
        ///
        /// Owners group entries so independent producers can share a
        /// destination; a path is deleted only when its last owner releases it.
        #[arg(long, value_name = "NAME")]
        owner: Option<String>,

        /// Most bytes one run may read from its sources; default from the
        /// configuration.
        ///
        /// One bound across every source the run reads, in bytes: an
        /// archive counts what it expands to rather than its size on disk.
        /// A zip counts both — its index is read whole before any member,
        /// so the zip file itself has to fit too.
        #[arg(long, id = "max-source-size", value_name = "BYTES")]
        max_source_size: Option<u64>,

        /// Plan and report, write nothing.
        #[arg(long, id = "dry-run")]
        dry_run: bool,

        /// Overwrite drifted files.
        #[arg(long)]
        force: bool,

        /// Permit symlink targets outside the destination.
        #[arg(long, id = "allow-external-targets")]
        allow_external_targets: bool,
    },

    /// Removes what the manifest records under an owner: everything it
    /// holds, or the recorded paths named as positionals.
    Rm {
        /// The recorded paths to remove; none names everything the owner
        /// holds.
        #[arg(value_name = "PATH", value_parser = clap::value_parser!(Utf8PathBuf))]
        paths: Vec<Utf8PathBuf>,

        /// Manifest owner; default from the configuration.
        ///
        /// Owners group entries so independent producers can share a
        /// destination; a path is deleted only when its last owner releases it.
        #[arg(long, value_name = "NAME")]
        owner: Option<String>,

        /// Plan and report, remove nothing.
        #[arg(long, id = "dry-run")]
        dry_run: bool,

        /// Remove drifted files.
        #[arg(long)]
        force: bool,
    },

    /// Classifies the manifest's paths and everything else under the
    /// destination, writing nothing.
    ///
    /// Verdicts: clean (matches the manifest), drifted (edited on disk),
    /// missing (recorded but absent from disk), foreign (present but
    /// unrecorded). Plain `status` exits 0 whatever the verdicts; --check
    /// exits 2 on anything but a clean destination, so a CI job can fail on
    /// drift without running a write.
    Status {
        /// Exit 2 unless every path is clean.
        ///
        /// A drifted, missing or foreign path exits 2, and so does a
        /// --state-dir that is not there, which reads as the empty manifest
        /// and would otherwise report a whole destination as foreign and
        /// pass. Everything clean exits 0. The report itself is the same
        /// either way; the exit code is the verdict.
        #[arg(long)]
        check: bool,
    },
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
            .long_about(CONFIG_ABOUT)
            .mut_arg("scope", |scope| {
                scope.help("Persist scope to target [possible values: user] [default: user].")
            })
            .mut_subcommand("gen", utf8_destination)
            .mut_subcommand("schema", utf8_destination)
            .mut_subcommand("get", name_the_key)
            .mut_subcommand("set", name_the_key)
            .mut_subcommand("unset", name_the_key),
    )
}

/// Clapfig's key-argument help names a `database.url` example from its own docs;
/// proiectio's keys are flat words, so name one of those instead.
///
/// `mut_arg` drops a positional's index, and clap then re-derives every
/// positional index on the command — so once one is set by hand they all must
/// be, or the derived indices collide. `key` is first on every leaf that takes
/// one; `set` names its value second.
fn name_the_key(command: clap::Command) -> clap::Command {
    let named = command.mut_arg("key", |key| {
        key.index(1).help("Config key (e.g. \"owner\").")
    });
    if named.get_name() == "set" {
        named.mut_arg("value", |value| value.index(2))
    } else {
        named
    }
}

/// Clapfig parses `--file` as a `PathBuf` and writes the file before it
/// reports the path, so a path the CLI cannot render would be created and
/// then fail. Reading it as a `Utf8PathBuf` refuses it at the command line,
/// before clapfig is asked for anything.
fn utf8_destination(command: clap::Command) -> clap::Command {
    command.mut_arg("output", |argument| {
        argument.value_parser(clap::value_parser!(Utf8PathBuf))
    })
}

const WRITE_ABOUT: &str = "\
Projects a mapping file, loose files, or a tree onto the destination.

A write declares an owner's COMPLETE set for the destination: paths the owner \
recorded on an earlier run but does not name this time are released, and a path \
released by its last owner is REMOVED from disk. Re-running the same inputs is \
otherwise a no-op.

Inputs: one file positional is a mapping (TOML); two or more positionals are \
loose files projected under their own basenames; --tree names a directory or an \
archive (.tar, .tar.gz, .tgz, .tar.zst, .zip). A single loose file has no \
positional spelling — one positional is always read as a mapping — so project \
it through a mapping's `source` or a --tree directory.

Mapping files are TOML:

    version = 1
    [files.\"path/in/dest\"]        # exactly one of source / contents:
    source     = \"path/to/file\"   #   copy a file (relative to the mapping's dir)
    contents   = \"inline text\"    #   or write these literal bytes
    executable = true             #   optional
    [links.\"path/in/dest\"]
    target = \"relative/target\"    # inside the destination unless --allow-external-targets
    [archives.\"prefix/in/dest\"]
    source = \"pkg.tar.gz\"
    strip  = 1";

const CONFIG_ABOUT: &str = "\
Reads, writes and documents proiectio's configuration.

Values resolve as compiled defaults, then files, then PROIECTIO__* \
environment variables (a doubled underscore separates key segments; keys are \
case-insensitive, so PROIECTIO__OWNER sets `owner`). The `user` scope is the \
platform config directory, which is also the search path; `set` and `unset` \
with no --scope write there.

Available keys (2): owner, max_source_size.";

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
