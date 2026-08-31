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
    /// Spaces aligning the path column.
    pub(crate) state_pad: String,
    pub(crate) path: String,
}

/// What `status.jinja` prints: one line per classified path.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct StatusLines {
    pub(crate) rows: Vec<StateView>,
}

/// The classification column: the widest word a state the library declares
/// reads as, which is the least the column is ever wide. A verdict this CLI
/// has no word for reads as its own name, and a name longer than this widens
/// the column for every row rather than spilling one path out of line. The
/// tests drive every state the library declares through `spelling` and check
/// the word still fits the constant.
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

/// A verdict is a name, or a name carrying fields; a row reads both, the way
/// `run.rs` reads its own verdicts, so a verdict that ever grows a payload
/// keeps its line instead of dropping out of the listing. A row stating no
/// verdict is a row all the same, and reads as the empty name — unknown.
fn verdict_name(verdict: Option<&JsonValue>) -> &str {
    match verdict {
        Some(JsonValue::String(name)) => name,
        Some(JsonValue::Object(fields)) => fields.keys().next().map_or("", String::as_str),
        _ => "",
    }
}

/// The lines `status.jinja` prints for one status document.
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

/// The columns `--output csv` writes, one row per classified path: the same
/// header line for every destination, whatever its rows carry.
///
/// The verdict cell is the name the printed line reads, not the raw field: a
/// verdict that ever carried fields would otherwise arrive as a JSON object in
/// a column every other row spells as a word, and the two outputs would
/// disagree about the same row. What such a payload said would need a column
/// of its own.
///
/// The row states two things no column here reads. A status row's facts come
/// from the manifest, which records what a path is and who holds it, not which
/// input named it and not where a link points, so `origin` and a link's target
/// are null in every status row of every destination; a column for either
/// would promise what the document never carries. A `write` or `rm` row states
/// both, and its own projection carries them.
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
            .derived_column(cells::header("owners"), |row, _| {
                cells::cell(cells::owners(row))
            })
            .build(),
    )
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
