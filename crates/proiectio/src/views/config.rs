//! Clapfig's `ConfigResult` as this CLI renders and serializes it.

use std::path::PathBuf;

use camino::Utf8PathBuf;
use clapfig::ConfigResult;
use clapfig::runtime::LeafType;
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
    /// to. Only the two results that edited one ask for it, so a read costs no
    /// platform lookup and succeeds where none resolves.
    ///
    /// The written-file variants name a path of clapfig's own, and a path this
    /// CLI cannot render is the one thing clapfig can hand back that no output
    /// mode can carry. Reading `--file` as UTF-8 refuses such a path at the
    /// command line, so what is left here is a path clapfig chose itself.
    pub(crate) fn of(
        result: ConfigResult,
        edit: impl FnOnce() -> Result<Edit, Error>,
    ) -> Result<Self, Error> {
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
            // Clapfig's set creates the file it persists to, so a `ValueSet`
            // in hand is the write itself; only an unset can come back from a
            // file that was never there, and that is read off the path.
            ConfigResult::ValueSet { key, value, .. } => Self::ValueSet {
                rendered: assignment(&key, &value),
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

fn documented(key: &str, value: &str, doc: &[String]) -> String {
    doc.iter()
        .map(|line| format!("# {line}"))
        .chain(std::iter::once(assignment(key, value)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One line of the config file the reader can paste back into it: clapfig
/// spells both halves for a human to read, which leaves a string value that
/// needs quotes bare, and a key a bare TOML key cannot carry unquoted.
fn assignment(key: &str, value: &str) -> String {
    format!("{} = {}", dotted(key), spelled(key, value))
}

/// The value as a document spells it. The schema names the type for a key it
/// declares. A scoped listing reads the file rather than the schema, so it
/// also carries keys the schema does not declare, and clapfig has already
/// stringified those: what is left to ask is whether the spelling stands as a
/// value at all. One that parses keeps its spelling; one that does not is the
/// string it can only have been.
///
/// A string that reads as another type — `"true"`, `"12"` — is the one thing
/// this cannot recover for an undeclared key, because clapfig stringified it
/// before the view saw it.
fn spelled(key: &str, value: &str) -> String {
    let quoted = || toml::Value::from(value).to_string();
    match settings::leaf_type(key) {
        Some(LeafType::String) => quoted(),
        Some(_) => value.to_owned(),
        None if parses_as_value(value) => value.to_owned(),
        None => quoted(),
    }
}

fn parses_as_value(value: &str) -> bool {
    format!("v = {value}").parse::<toml::Table>().is_ok()
}

/// The key as a document spells it: one segment per dot, each quoted where a
/// bare TOML key cannot carry it. A scoped listing reads the file itself, so
/// the keys reaching here are the writer's rather than the schema's.
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
