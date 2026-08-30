//! Clapfig's `ConfigResult` as this CLI renders and serializes it.

use std::path::PathBuf;

use camino::Utf8PathBuf;
use clapfig::ConfigResult;
use clapfig::runtime::LeafType;
use libproiectio::Error;
use serde::Serialize;

use crate::settings;

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
        path: Utf8PathBuf,
    },
    ValueUnset {
        key: String,
        path: Utf8PathBuf,
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

impl ConfigView {
    /// `scope` is the file the invocation's scope persists to: the results that
    /// edited it name it, and the rest ignore it.
    ///
    /// The written-file variants name a path of clapfig's own, and a path this
    /// CLI cannot render is the one thing clapfig can hand back that no output
    /// mode can carry. Reading `--file` as UTF-8 refuses such a path at the
    /// command line, so what is left here is a path clapfig chose itself.
    pub(crate) fn of(result: ConfigResult, scope: Utf8PathBuf) -> Result<Self, Error> {
        Ok(match result {
            ConfigResult::Listing { entries, .. } => {
                let entries: Vec<(String, String)> = entries
                    .into_iter()
                    .filter(|(key, _)| !settings::is_comment_key(key))
                    .collect();
                Self::Listing {
                    rendered: entries
                        .iter()
                        .map(|(key, value)| assignment(key, value))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    entries: entries
                        .into_iter()
                        .map(|(key, value)| ConfigEntryView { key, value })
                        .collect(),
                }
            }
            ConfigResult::KeyValue {
                key, value, doc, ..
            } => Self::KeyValue {
                rendered: documented(&key, &value, &doc),
                key,
                value,
                doc,
            },
            ConfigResult::ValueSet { key, value, .. } => Self::ValueSet {
                rendered: assignment(&key, &value),
                key,
                value,
                path: scope,
            },
            ConfigResult::ValueUnset { key } => Self::ValueUnset { key, path: scope },
            ConfigResult::Template(body) => Self::Template { body },
            ConfigResult::TemplateWritten { path } => Self::TemplateWritten { path: utf8(path)? },
            ConfigResult::Schema(body) => Self::Schema { body },
            ConfigResult::SchemaWritten { path } => Self::SchemaWritten { path: utf8(path)? },
        })
    }
}

fn documented(key: &str, value: &str, doc: &[String]) -> String {
    doc.iter()
        .map(|line| format!("# {line}"))
        .chain(std::iter::once(assignment(key, value)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One line of the config file the reader can paste back into it: clapfig
/// spells a value for a human to read, which leaves a string that needs quotes
/// bare.
fn assignment(key: &str, value: &str) -> String {
    match settings::leaf_type(key) {
        Some(LeafType::String) => format!("{key} = {}", toml::Value::from(value)),
        _ => format!("{key} = {value}"),
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
