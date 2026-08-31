//! The lines `status.jinja` lays out, and the columns `--output csv` writes.
//!
//! The lines reach the template through Standout's context injection, which
//! structured modes skip, so `--output json` stays the library's own `Status`.
//! The CSV columns are a projection over that same document: Standout's CSV
//! flattener writes one record per element of a top-level array and one row
//! for anything else, so without a projection a document whose rows sit under
//! a field flattens to a single row.

use std::iter;

use serde::Serialize;
use serde_json::Value as JsonValue;
use standout::AmbiguousWidth;
use standout::tabular::visible_width_with_policy;
use standout::{CsvProjection, StructuredOutputProjection};

use crate::app::verbatim;
use crate::views::cells;
use crate::views::pad;

/// One printed line: the classification in its own style, and the path it
/// classifies.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct StateView {
    pub(crate) style: &'static str,
    pub(crate) state: String,
    pub(crate) state_pad: String,
    pub(crate) path: String,
}

/// What `status.jinja` prints: one line per classified path.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct StatusLines {
    pub(crate) rows: Vec<StateView>,
}

/// The least the classification column is ever wide; an unknown verdict's own
/// name can widen it. The tests drive every state the library declares
/// through `spelling` and check the word still fits the constant.
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

/// A verdict is a bare name, or a name carrying fields; this reads both, so
/// a verdict that ever grows a payload keeps its line.
fn verdict_name(verdict: Option<&JsonValue>) -> &str {
    match verdict {
        Some(JsonValue::String(name)) => name,
        Some(JsonValue::Object(fields)) => fields.keys().next().map_or("", String::as_str),
        _ => "",
    }
}

pub(crate) fn lines(document: &JsonValue, width: AmbiguousWidth) -> StatusLines {
    let Some(rows) = document.get("rows").and_then(JsonValue::as_array) else {
        return StatusLines::default();
    };

    let spelled: Vec<(&'static str, String, String)> = rows
        .iter()
        .filter_map(|row| {
            let path = row.get("path")?.as_str()?;
            let (style, state) = spelling(verdict_name(row.get("verdict")));
            Some((style, state, verbatim(path)))
        })
        .collect();
    let states = spelled
        .iter()
        .map(|(_, state, _)| visible_width_with_policy(state, width))
        .chain(iter::once(STATES))
        .max()
        .unwrap_or(STATES);

    let lines = spelled
        .into_iter()
        .map(|(style, state, path)| StateView {
            state_pad: pad(states, &state, width),
            style,
            state,
            path,
        })
        .collect();

    StatusLines { rows: lines }
}

/// The columns `--output csv` writes, one row per classified path under a
/// header that does not move. The verdict cell is the variant's bare name;
/// there is no `origin` column because a status row never carries one.
pub(crate) fn csv() -> StructuredOutputProjection {
    StructuredOutputProjection::csv(
        CsvProjection::builder("rows")
            .column(cells::column("path"))
            .derived_column(cells::header("verdict"), |row, _| {
                cells::cell(Some(verdict_name(row.get("verdict")).to_owned()))
            })
            .derived_column(cells::header("shape"), |row, _| {
                cells::cell(cells::shape(row))
            })
            .derived_column(cells::header("executable"), |row, _| {
                cells::cell(cells::executable(row))
            })
            .derived_column(cells::header("target"), |row, _| {
                cells::cell(cells::target(row))
            })
            .derived_column(cells::header("owners"), |row, _| {
                cells::cell(cells::owners(row))
            })
            .build(),
    )
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
