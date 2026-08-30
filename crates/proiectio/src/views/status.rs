//! The lines `status.jinja` lays out, and the columns `--output csv` writes.
//!
//! The lines reach the template through Standout's context injection, which
//! structured modes skip, so `--output json` stays the library's own `Status`.
//! The CSV columns are a projection over that same document: Standout's CSV
//! flattener writes one record per element of a top-level array and one row
//! for anything else, so without a projection a document whose rows sit under
//! a field flattens to a single row.

use serde::Serialize;
use serde_json::Value as JsonValue;
use standout::AmbiguousWidth;
use standout::tabular::{Column, Width};
use standout::{CsvProjection, StructuredOutputProjection};

use crate::app::verbatim;
use crate::views::pad;

/// One printed line: the classification in its own style, and the path it
/// classifies.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct StateView {
    pub(crate) style: &'static str,
    pub(crate) state: String,
    /// Spaces aligning the path column.
    pub(crate) state_pad: String,
    pub(crate) path: String,
}

/// What `status.jinja` prints: one line per classified path.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct StatusLines {
    pub(crate) rows: Vec<StateView>,
}

/// The classification column: the widest word a state reads as.
const STATES: usize = "drifted".len();

/// The style and the word one classification reads as; a verdict this CLI does
/// not know reads as its own name, in the style unknown verdicts share.
fn spelling(verdict: &str) -> (&'static str, String) {
    match verdict {
        "Clean" => ("clean", "clean".to_owned()),
        "Drifted" => ("drifted", "drifted".to_owned()),
        "Missing" => ("missing", "missing".to_owned()),
        "Foreign" => ("foreign", "foreign".to_owned()),
        unknown => ("unknown", verbatim(unknown)),
    }
}

/// The lines `status.jinja` prints for one status document.
pub(crate) fn lines(document: &JsonValue, width: AmbiguousWidth) -> StatusLines {
    let Some(rows) = document.get("rows").and_then(JsonValue::as_array) else {
        return StatusLines::default();
    };

    let lines = rows
        .iter()
        .filter_map(|row| {
            let path = row.get("path")?.as_str()?;
            let verdict = row.get("verdict").and_then(JsonValue::as_str)?;
            let (style, state) = spelling(verdict);
            Some(StateView {
                state_pad: pad(STATES, &state, width),
                style,
                state,
                path: verbatim(path),
            })
        })
        .collect();

    StatusLines { rows: lines }
}

/// The columns `--output csv` writes, one row per classified path: the same
/// header line for every destination, whatever its rows carry.
pub(crate) fn csv() -> StructuredOutputProjection {
    StructuredOutputProjection::csv(
        CsvProjection::builder("rows")
            .column(column("path"))
            .column(column("verdict"))
            .derived_column(named("shape"), |row, _| cell(shape(row)))
            .derived_column(named("executable"), |row, _| cell(executable(row)))
            .derived_column(named("owners"), |row, _| cell(owners(row)))
            .build(),
    )
}

/// A column reading one field of the row.
fn column(key: &str) -> Column {
    named(key).key(key)
}

/// A column a callback fills, named for what it states.
fn named(header: &str) -> Column {
    Column::new(Width::fill()).header(header)
}

/// A cell holding what a row states, and an empty one where it states nothing.
fn cell(value: Option<String>) -> JsonValue {
    JsonValue::String(value.unwrap_or_default())
}

/// The shape the manifest records for the path, in one word.
fn shape(row: &JsonValue) -> Option<String> {
    let shape = row.get("facts")?.get("shape")?;
    let named = match shape {
        JsonValue::String(name) => name.as_str(),
        JsonValue::Object(fields) => fields.keys().next()?.as_str(),
        _ => return None,
    };
    Some(named.to_lowercase())
}

/// Whether the recorded file carries the executable bit; nothing for a path
/// whose shape has no such bit.
fn executable(row: &JsonValue) -> Option<String> {
    let executable = row
        .get("facts")?
        .get("shape")?
        .get("File")?
        .get("executable")?
        .as_bool()?;
    Some(executable.to_string())
}

/// The owners holding the path, joined the way the library joins them.
fn owners(row: &JsonValue) -> Option<String> {
    let owners: Vec<&str> = row
        .get("facts")?
        .get("owners")?
        .as_array()?
        .iter()
        .filter_map(JsonValue::as_str)
        .collect();
    (!owners.is_empty()).then(|| owners.join("+"))
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
