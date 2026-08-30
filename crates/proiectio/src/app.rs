//! App wiring: the composition root naming every command, template and style.

use anyhow::Result;
use standout::cli::App;
use standout::{EmbeddedTemplates, MiniJinjaEngine, embed_styles, embed_templates};

use crate::handlers;

pub(crate) fn templates() -> EmbeddedTemplates {
    embed_templates!("src/templates")
}

/// Spells the characters a terminal acts on rather than shows: every control
/// character as its Rust escape, and `[` and `]` as the escapes Standout's
/// markup pass reads back as literal brackets. A value cannot then forge a
/// row, restyle the display, or reach the terminal as a command.
pub(crate) fn verbatim(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '[' | ']' => {
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
    value
        .split('\n')
        .map(verbatim)
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

pub(crate) fn build() -> Result<App> {
    Ok(App::builder()
        .version(env!("CARGO_PKG_VERSION"))
        .template_engine(Box::new(engine()))
        .templates(templates())
        .styles(embed_styles!("src/styles"))
        .default_theme("proiectio")
        .command_with("status", handlers::status__handler, |cfg| {
            cfg.template("status.jinja")
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
