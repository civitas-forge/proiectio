//! App wiring: the composition root naming every command, template and style.

use std::cell::Cell;
use std::rc::Rc;

use anyhow::Result;
use clap::ArgMatches;
use standout::cli::{App, CommandConfig, CommandContext, CommandContextInput, HookError};
use standout::context::RenderContext;
use standout::{EmbeddedTemplates, RenderData, embed_styles, embed_templates};

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
///
/// The encoding comes from the context, which reports what the run resolved.
pub(crate) fn stated_on_stderr(
    _matches: &ArgMatches,
    ctx: &CommandContext,
    document: RenderData,
) -> Result<RenderData, HookError> {
    if ctx.representation().is_structured() {
        for stated in views::run_warnings(&document.to_json()) {
            ctx.warn(crate::exit::warning(&stated));
        }
    }
    Ok(document)
}

pub(crate) fn build() -> Result<App> {
    let forced = Forced::default();
    let hints = forced.clone();
    Ok(App::builder()
        .name(env!("CARGO_PKG_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .usage_exit_status(crate::exit::USAGE)
        .app_state(forced)
        .templates(templates())
        .styles(embed_styles!("src/styles"))
        .default_theme("proiectio")
        .context_fn("run", move |context: &RenderContext| {
            RenderData::from_serialize(views::run_lines(
                &context.data.to_json(),
                context.ambiguous_width(),
                hints.get(),
            ))
            .unwrap_or(RenderData::Null)
        })
        .context_fn("status", |context: &RenderContext| {
            RenderData::from_serialize(views::status_lines(
                &context.data.to_json(),
                context.ambiguous_width(),
            ))
            .unwrap_or(RenderData::Null)
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
