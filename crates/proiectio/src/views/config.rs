//! Clapfig's `ConfigResult` as this CLI renders and serializes it.

use std::path::PathBuf;

use camino::Utf8PathBuf;
use clapfig::ConfigResult;
use libproiectio::Error;
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
        path: Utf8PathBuf,
    },
    Schema {
        body: String,
    },
    SchemaWritten {
        path: Utf8PathBuf,
    },
}

/// The written-file variants name a path, and a path this CLI cannot render is
/// the one thing clapfig can hand back that no output mode can carry. Reading
/// `--file` as UTF-8 refuses such a path at the command line, so what is left
/// here is a path clapfig chose itself.
impl TryFrom<ConfigResult> for ConfigView {
    type Error = Error;

    fn try_from(result: ConfigResult) -> Result<Self, Error> {
        Ok(match result {
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
            ConfigResult::TemplateWritten { path } => Self::TemplateWritten { path: utf8(path)? },
            ConfigResult::Schema(body) => Self::Schema { body },
            ConfigResult::SchemaWritten { path } => Self::SchemaWritten { path: utf8(path)? },
        })
    }
}

fn utf8(path: PathBuf) -> Result<Utf8PathBuf, Error> {
    Utf8PathBuf::from_path_buf(path).map_err(|path| Error::PathNotUtf8 {
        path: path.to_string_lossy().into_owned(),
    })
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
