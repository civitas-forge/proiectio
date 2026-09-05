//! App wiring: the composition root naming every command, template and style.

use std::cell::Cell;
use std::rc::Rc;

use anyhow::Result;
use clap::ArgMatches;
use minijinja::Value;
use serde_json::Value as JsonValue;
use standout::cli::{App, CommandConfig, CommandContext, CommandContextInput, HookError};
use standout::context::RenderContext;
use standout::{EmbeddedTemplates, MiniJinjaEngine, Representation, embed_styles, embed_templates};

use crate::cli::Commands;
use crate::handlers;
use crate::views;

/// Whether the invocation carried `--force`, which suppresses the drift hint.
///
/// The hint lines need it and the document cannot carry it: what a run
/// serializes is the library's own report, and the drift policy is the
/// command line's. So it travels as app state — a cell this composition root
/// owns, the handler records into, and the `run` context function reads back.
#[derive(Clone, Default)]
pub(crate) struct Forced(Rc<Cell<bool>>);

impl Forced {
    pub(crate) fn record(&self, forced: bool) {
        self.0.set(forced);
    }

    fn get(&self) -> bool {
        self.0.get()
    }
}

pub(crate) fn templates() -> EmbeddedTemplates {
    embed_templates!("src/templates")
}

/// Spells the characters a terminal acts on rather than shows: every control
/// character as its Rust escape, and `[` and `]` as the escapes Standout's
/// markup pass reads back as literal brackets. A value cannot then forge a
/// row, restyle the display, or reach the terminal as a command.
pub(crate) fn verbatim(value: &str) -> String {
    escape(value, Brackets::Escaped)
}

/// The control-character half of [`verbatim_block`], for the diagnostics this
/// CLI writes about a run: no markup pass reads those, so a bracket is already
/// itself, and a message clap spelled over several lines keeps them.
pub(crate) fn control_escaped_block(value: &str) -> String {
    block(value, |line| escape(line, Brackets::Literal))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Brackets {
    Escaped,
    Literal,
}

fn escape(value: &str, brackets: Brackets) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '[' | ']' if brackets == Brackets::Escaped => {
                escaped.push('\\');
                escaped.push(character);
            }
            control if control.is_control() => escaped.extend(control.escape_debug()),
            other => escaped.push(other),
        }
    }
    escaped
}

/// The same escaping over a block whose lines this CLI asked for: the line
/// breaks are the layout, everything inside one is data.
pub(crate) fn verbatim_block(value: &str) -> String {
    block(value, verbatim)
}

fn block(value: &str, line: impl Fn(&str) -> String) -> String {
    value
        .split('\n')
        .map(line)
        .collect::<Vec<String>>()
        .join("\n")
}

pub(crate) fn engine() -> MiniJinjaEngine {
    let mut engine = MiniJinjaEngine::new();
    let environment = engine.environment_mut();
    environment.add_filter("verbatim", |value: String| verbatim(&value));
    environment.add_filter("verbatim_block", |value: String| verbatim_block(&value));
    engine
}

/// The rows `write` and `rm` write under `--output csv`. Both render one
/// [`views::RunView`], so both name this from their `inputs` key; the rest of
/// what the two share — the template and the post-dispatch hook — has a
/// `#[dispatch]` key of its own on the variant.
pub(crate) fn run_projection<H>(config: CommandConfig<H>) -> CommandConfig<H> {
    config.structured_output_projection(views::run_csv())
}

/// The rows `status` writes under `--output csv`.
pub(crate) fn status_projection<H>(config: CommandConfig<H>) -> CommandConfig<H> {
    config.structured_output_projection(views::status_csv())
}

/// The one entry every `config` leaf renders through: the group branches on
/// the view's tag rather than on which leaf produced it, so convention's
/// per-command name (`config/list`, `config/get`) would ask for seven copies
/// of one template.
pub(crate) fn config_template<H>(config: CommandConfig<H>) -> CommandConfig<H> {
    config.template_name("config")
}

/// A listing states its keys under a field, and `get` states a doc comment as
/// lines; both are arrays, which `--output csv` takes only through a
/// projection naming the rows and the cells. The leaves that state one flat
/// record need none.
pub(crate) fn config_listing<H>(config: CommandConfig<H>) -> CommandConfig<H> {
    config_template(config).structured_output_projection(views::config_listing_csv())
}

pub(crate) fn config_key_value<H>(config: CommandConfig<H>) -> CommandConfig<H> {
    config_template(config).structured_output_projection(views::config_key_value_csv())
}

/// Pushes a stopped run's run-level facts as warnings, which Standout writes
/// past the run's output — only for the modes that serialize the document; the
/// template already lays these sentences out for rendered output.
pub(crate) fn stated_on_stderr(
    matches: &ArgMatches,
    ctx: &CommandContext,
    document: JsonValue,
) -> Result<JsonValue, HookError> {
    if serializing(matches) {
        for stated in views::run_warnings(&document) {
            ctx.warn(crate::exit::warning(&stated));
        }
    }
    Ok(document)
}

/// Whether `--output` named a representation that serializes the document.
/// Read back off the parsed command line because a handler and its hooks are
/// handed no other way to Standout's own argument: the argument's id is
/// documented but its type, its parser and `App::extract_output_mode`'s input
/// are not reachable from a hook. The aborted-run tests under `--output
/// json`/`csv` hold this to the name Standout parses it under.
fn serializing(matches: &ArgMatches) -> bool {
    matches
        .try_get_one::<String>(OUTPUT_MODE)
        .ok()
        .flatten()
        .is_some_and(|named| representation(named).is_structured())
}

/// The argument Standout parses `--output` into.
const OUTPUT_MODE: &str = "_output_mode";

/// `--output` names a structured encoding or the diagnostic `term-debug`;
/// absent, the run renders the human template, which the flag cannot name.
fn representation(named: &str) -> Representation {
    match named {
        "json" => Representation::Json,
        "yaml" => Representation::Yaml,
        "csv" => Representation::Csv,
        "ndjson" => Representation::Ndjson,
        "term-debug" => Representation::TermDebug,
        _ => Representation::Human,
    }
}

pub(crate) fn build() -> Result<App> {
    let forced = Forced::default();
    let hints = forced.clone();
    Ok(App::builder()
        .name(env!("CARGO_PKG_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .app_state(forced)
        .template_engine(Box::new(engine()))
        .templates(templates())
        .styles(embed_styles!("src/styles"))
        .default_theme("proiectio")
        .context_fn("run", move |context: &RenderContext| {
            Value::from_serialize(views::run_lines(
                context.data,
                context.ambiguous_width(),
                hints.get(),
            ))
        })
        .context_fn("status", |context: &RenderContext| {
            Value::from_serialize(views::status_lines(context.data, context.ambiguous_width()))
        })
        .commands(Commands::dispatch_config())?
        .command_with("config", handlers::config_root_Handler, config_listing)?
        .command_with("config.list", handlers::config_list_Handler, config_listing)?
        .command_with("config.get", handlers::config_get_Handler, config_key_value)?
        .command_with("config.set", handlers::config_set_Handler, config_template)?
        .command_with(
            "config.unset",
            handlers::config_unset_Handler,
            config_template,
        )?
        .command_with("config.gen", handlers::config_gen_Handler, config_template)?
        .command_with(
            "config.schema",
            handlers::config_schema_Handler,
            config_template,
        )?
        .build()?)
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
