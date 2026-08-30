//! App wiring: the composition root naming every command, template and style.

use anyhow::Result;
use standout::cli::App;
use standout::{EmbeddedTemplates, MiniJinjaEngine, embed_styles, embed_templates};

use crate::handlers;

pub(crate) fn templates() -> EmbeddedTemplates {
    embed_templates!("src/templates")
}

/// Spells `[` and `]` as the escapes Standout's markup pass reads back as
/// literal brackets, so a value that reads like a style tag is not one.
pub(crate) fn verbatim(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if character == '[' || character == ']' {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

pub(crate) fn engine() -> MiniJinjaEngine {
    let mut engine = MiniJinjaEngine::new();
    engine
        .environment_mut()
        .add_filter("verbatim", |value: String| verbatim(&value));
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
