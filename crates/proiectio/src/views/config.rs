//! Clapfig's `ConfigResult` as this CLI renders and serializes it.

use std::path::PathBuf;

use clapfig::ConfigResult;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub(crate) struct ConfigEntryView {
    pub(crate) key: String,
    pub(crate) value: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ConfigView {
    Listing {
        entries: Vec<ConfigEntryView>,
        rendered: String,
    },
    KeyValue {
        key: String,
        value: String,
        doc: Vec<String>,
        rendered: String,
    },
    ValueSet {
        key: String,
        value: String,
        rendered: String,
    },
    ValueUnset {
        key: String,
    },
    Template {
        body: String,
    },
    TemplateWritten {
        path: PathBuf,
    },
    Schema {
        body: String,
    },
    SchemaWritten {
        path: PathBuf,
    },
}

impl From<ConfigResult> for ConfigView {
    fn from(result: ConfigResult) -> Self {
        match result {
            ConfigResult::Listing { entries, rendered } => Self::Listing {
                entries: entries
                    .into_iter()
                    .map(|(key, value)| ConfigEntryView { key, value })
                    .collect(),
                rendered,
            },
            ConfigResult::KeyValue {
                key,
                value,
                doc,
                rendered,
            } => Self::KeyValue {
                key,
                value,
                doc,
                rendered,
            },
            ConfigResult::ValueSet {
                key,
                value,
                rendered,
            } => Self::ValueSet {
                key,
                value,
                rendered,
            },
            ConfigResult::ValueUnset { key } => Self::ValueUnset { key },
            ConfigResult::Template(body) => Self::Template { body },
            ConfigResult::TemplateWritten { path } => Self::TemplateWritten { path },
            ConfigResult::Schema(body) => Self::Schema { body },
            ConfigResult::SchemaWritten { path } => Self::SchemaWritten { path },
        }
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
