//! App wiring: the composition root naming every command, template and style.

use std::cell::Cell;
use std::rc::Rc;

use anyhow::Result;
use clap::ArgMatches;
use minijinja::Value;
use serde_json::Value as JsonValue;
use standout::cli::{App, CommandConfig, CommandContext, HookError};
use standout::context::RenderContext;
use standout::{EmbeddedTemplates, MiniJinjaEngine, OutputMode, embed_styles, embed_templates};

use crate::exit::Verdict;
use crate::handlers;
use crate::views;

/// Whether the invocation carried `--force`.
///
/// The hint lines need it and the document cannot carry it: what a run
/// serializes is the library's own report, and the drift policy is the command
/// line's, not the report's. So it reaches rendering the way [`Verdict`] does —
/// a cell this composition root owns, the handler records into, and the `run`
/// context function reads back.
///
/// It suppresses the drift hint rather than rewording it. The hint names the
/// flag that lifts drift; a reader who passed that flag and was refused anyway
/// has met the drift no policy lifts, and repeating the flag at them is advice
/// they have already taken.
#[derive(Clone, Default)]
pub(crate) struct Forced(Rc<Cell<bool>>);

impl Forced {
    /// Records what the command line carried, before the run it describes is
    /// rendered.
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

/// How `write` and `rm` present one write pass: the template that lays their
/// rows out, the projection that writes the same rows as CSV records, and the
/// stderr channel a stopped run's run-level facts take.
///
/// Both commands render one [`views::RunView`], so both are configured here
/// rather than at each call: a channel added to one of them and forgotten at
/// the other is a difference no reader of either command's output could
/// explain.
pub(crate) fn run_command<H>(config: CommandConfig<H>) -> CommandConfig<H> {
    config
        .template("run.jinja")
        .structured_output_projection(views::run_csv())
        .post_dispatch(stated_on_stderr)
}

/// Writes what a stopped run's records cannot carry — how far the run got,
/// what stopped it, and whether the state directory records what it applied —
/// as warnings, which `main` drains to stderr after the run's own output.
///
/// Only for the modes that serialize the document: the template already lays
/// these sentences out for a reader of the rendered output, and saying them
/// twice would have that reader looking for two failures.
fn stated_on_stderr(
    matches: &ArgMatches,
    _ctx: &CommandContext,
    document: JsonValue,
) -> Result<JsonValue, HookError> {
    if serializing(matches) {
        for stated in views::run_warnings(&document) {
            standout::warnings::push_warning(stated);
        }
    }
    Ok(document)
}

/// Whether `--output` named a mode that serializes the document rather than
/// rendering the template.
///
/// The mode is the framework's own argument, read back off the parsed command
/// line because a handler and its hooks are handed no other way to it. The
/// tests over an aborted run under `--output json` and `--output csv` are what
/// hold this to the name Standout parses the flag under.
fn serializing(matches: &ArgMatches) -> bool {
    matches
        .try_get_one::<String>(OUTPUT_MODE)
        .ok()
        .flatten()
        .is_some_and(|named| mode(named).is_structured())
}

/// The argument Standout parses `--output` into.
const OUTPUT_MODE: &str = "_output_mode";

fn mode(named: &str) -> OutputMode {
    match named {
        "term" => OutputMode::Term,
        "text" => OutputMode::Text,
        "term-debug" => OutputMode::TermDebug,
        "json" => OutputMode::Json,
        "yaml" => OutputMode::Yaml,
        "xml" => OutputMode::Xml,
        "csv" => OutputMode::Csv,
        _ => OutputMode::Auto,
    }
}

pub(crate) fn build(verdict: Verdict) -> Result<App> {
    let forced = Forced::default();
    let hints = forced.clone();
    Ok(App::builder()
        .version(env!("CARGO_PKG_VERSION"))
        .app_state(verdict)
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
        .command_with("write", handlers::write__handler, run_command)?
        .command_with("rm", handlers::rm__handler, run_command)?
        .command_with("status", handlers::status__handler, |cfg| {
            cfg.template("status.jinja")
                .structured_output_projection(views::status_csv())
        })?
        .command_with("config", handlers::config_root__handler, |cfg| {
            cfg.template("config.jinja")
        })?
        .command_with("config.list", handlers::config_list__handler, |cfg| {
            cfg.template("config.jinja")
        })?
        .command_with("config.get", handlers::config_get__handler, |cfg| {
            cfg.template("config.jinja")
        })?
        .command_with("config.set", handlers::config_set__handler, |cfg| {
            cfg.template("config.jinja")
        })?
        .command_with("config.unset", handlers::config_unset__handler, |cfg| {
            cfg.template("config.jinja")
        })?
        .command_with("config.gen", handlers::config_gen__handler, |cfg| {
            cfg.template("config.jinja")
        })?
        .command_with("config.schema", handlers::config_schema__handler, |cfg| {
            cfg.template("config.jinja")
        })?
        .build()?)
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
