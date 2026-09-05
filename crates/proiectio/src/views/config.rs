//! Clapfig's `ConfigResult` as this CLI renders and serializes it.

use std::path::PathBuf;

use camino::Utf8PathBuf;
use clapfig::ConfigResult;
use clapfig::value::Value;
use libproiectio::Error;
use serde::Serialize;

use crate::settings::{self, Edit};

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
        wrote: bool,
    },
    ValueUnset {
        key: String,
        path: Utf8PathBuf,
        wrote: bool,
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
    /// `edit` reports what the invocation did to the file its scope persists
    /// to; only the two results that edited one ask for it, so a read costs
    /// no platform lookup and succeeds where none resolves.
    pub(crate) fn of(
        result: ConfigResult,
        edit: impl FnOnce() -> Result<Edit, Error>,
    ) -> Result<Self, Error> {
        Ok(match result {
            ConfigResult::Listing { entries, .. } => {
                let entries: Vec<(String, Value)> = entries
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
                        .map(|(key, value)| ConfigEntryView {
                            key,
                            value: stated(&value),
                        })
                        .collect(),
                }
            }
            ConfigResult::KeyValue {
                key, value, doc, ..
            } => Self::KeyValue {
                rendered: documented(&key, &value, &doc),
                key,
                value: stated(&value),
                doc,
            },
            // Clapfig's set creates the file it persists to, so a `ValueSet`
            // in hand is the write itself; only an unset can come back from a
            // file that was never there.
            ConfigResult::ValueSet { key, value, .. } => Self::ValueSet {
                rendered: assignment(&key, &Value::String(value.clone())),
                key,
                value,
                path: edit()?.path,
                wrote: true,
            },
            ConfigResult::ValueUnset { key } => {
                let edit = edit()?;
                Self::ValueUnset {
                    key,
                    path: edit.path,
                    wrote: edit.present,
                }
            }
            ConfigResult::Template(body) => Self::Template { body },
            ConfigResult::TemplateWritten { path } => Self::TemplateWritten { path: utf8(path)? },
            ConfigResult::Schema(body) => Self::Schema { body },
            ConfigResult::SchemaWritten { path } => Self::SchemaWritten { path: utf8(path)? },
        })
    }
}

fn documented(key: &str, value: &Value, doc: &[String]) -> String {
    doc.iter()
        .map(|line| format!("# {line}"))
        .chain(std::iter::once(assignment(key, value)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One line of the config file the reader can paste back into it. Clapfig
/// carries a typed value now, whose `Display` is the TOML notation for it —
/// a string quoted, everything else bare — so the only half left to spell is
/// the key, which a bare TOML key cannot always carry unquoted.
fn assignment(key: &str, value: &Value) -> String {
    format!("{} = {value}", dotted(key))
}

/// The value as a document states it rather than as a file spells it: a
/// string is itself, so a reader of the serialized document gets the value
/// and not its quoting, and everything else is its own notation.
fn stated(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The key as a document spells it: one segment per dot, each quoted where a
/// bare TOML key cannot carry it.
fn dotted(key: &str) -> String {
    key.split('.')
        .map(|segment| {
            if !segment.is_empty() && segment.chars().all(is_bare_key_char) {
                segment.to_owned()
            } else {
                toml::Value::from(segment).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn is_bare_key_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_' || character == '-'
}

fn utf8(path: PathBuf) -> Result<Utf8PathBuf, Error> {
    Utf8PathBuf::from_path_buf(path).map_err(|path| Error::PathNotUtf8 {
        path: path.to_string_lossy().into_owned(),
    })
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
