//! The document a write pass renders — `write`'s and `rm`'s alike — and the
//! lines its template lays out.
//!
//! The lines reach the template through Standout's context injection, which
//! structured modes skip, so `--output json` stays the library's own report.

use libproiectio::{ApplyReport, PlannedAction, Report};
use serde::Serialize;
use serde_json::Value as JsonValue;
use standout::AmbiguousWidth;
use standout::tabular::visible_width_with_policy;

use crate::app::verbatim;

/// A plan on a dry run, what apply did on a real one; untagged, so structured
/// output is the library's own either way.
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum RunView {
    Planned(Report<PlannedAction>),
    Applied(Box<ApplyReport>),
}

/// One printed line: the verb in its style, the path, and what qualifies it.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct RowView {
    pub(crate) style: &'static str,
    pub(crate) verb: String,
    /// Spaces aligning the path column.
    pub(crate) verb_pad: String,
    pub(crate) path: String,
    /// Spaces aligning the note column.
    pub(crate) path_pad: String,
    pub(crate) note: Option<String>,
}

/// What `run.jinja` prints: one row per path, and the count a real run
/// closes with.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct RunLines {
    pub(crate) rows: Vec<RowView>,
    pub(crate) summary: Option<String>,
}

/// The style and verb one verdict reads as, given whether the path is a
/// symlink; `None` for a verdict this CLI does not know.
fn spelling(verdict: &str, symlink: bool) -> Option<(&'static str, &'static str)> {
    Some(match (verdict, symlink) {
        ("Write", false) => ("wrote", "would write"),
        ("Write", true) => ("linked", "would link"),
        ("Written", false) => ("wrote", "wrote"),
        ("Written", true) => ("linked", "linked"),
        ("Overwrite", _) => ("overwrote", "would overwrite"),
        ("Overwritten", _) => ("overwrote", "overwrote"),
        ("Skip", _) => ("skipped", "would skip"),
        ("Skipped", _) => ("skipped", "skipped"),
        ("Remove", _) => ("removed", "would remove"),
        ("Removed", _) => ("removed", "removed"),
        ("Release", _) => ("removed", "would release"),
        ("Released", _) => ("removed", "released"),
        _ => return None,
    })
}

/// The column a real run counts a verdict under; a plan counts nothing.
fn counted(verdict: &str) -> Option<Counted> {
    Some(match verdict {
        "Written" | "Overwritten" => Counted::Wrote,
        "Skipped" => Counted::Skipped,
        "Removed" => Counted::Removed,
        "Released" => Counted::Released,
        _ => return None,
    })
}

/// Why a plan would overwrite a path.
fn overwriting(reason: &str) -> &'static str {
    match reason {
        "ContentChanged" => "(content changed)",
        "ExecutableChanged" => "(executable changed)",
        _ => "(drifted, forced)",
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Counted {
    Wrote,
    Skipped,
    Removed,
    Released,
}

#[derive(Default)]
struct Tally {
    wrote: usize,
    skipped: usize,
    removed: usize,
    released: usize,
}

impl Tally {
    fn count(&mut self, counted: Counted) {
        let column = match counted {
            Counted::Wrote => &mut self.wrote,
            Counted::Skipped => &mut self.skipped,
            Counted::Removed => &mut self.removed,
            Counted::Released => &mut self.released,
        };
        *column += 1;
    }

    /// A pass that projected reports what it wrote and skipped, and names a
    /// cleared column only where it holds something; one that only cleared
    /// paths reports what it cleared and nothing else. A pass that left every
    /// path alone reports what it left alone, and one that touched nothing
    /// says so.
    fn summary(&self) -> String {
        let cleared = self.removed + self.released;
        if self.wrote == 0 && cleared == 0 {
            return match self.skipped {
                0 => "nothing to do".to_owned(),
                skipped => format!("{skipped} unchanged"),
            };
        }
        let mut columns = Vec::new();
        if self.wrote > 0 || self.skipped > 0 {
            columns.push(format!("{} written, {} skipped", self.wrote, self.skipped));
        }
        for (count, column) in [(self.removed, "removed"), (self.released, "released")] {
            if count > 0 {
                columns.push(format!("{count} {column}"));
            }
        }
        columns.join(", ")
    }
}

/// The verb column: the widest verb a plan spells, and the widest a real run
/// spells.
const PLANNED_VERBS: usize = "would overwrite".len();
const APPLIED_VERBS: usize = "overwrote".len();

/// The lines `run.jinja` prints for one write-pass document.
pub(crate) fn lines(document: &JsonValue, width: AmbiguousWidth) -> RunLines {
    let (rows, planning) = match document.get("report") {
        Some(applied) => (applied.get("rows"), false),
        None => (document.get("rows"), true),
    };
    let Some(rows) = rows.and_then(JsonValue::as_object) else {
        return RunLines::default();
    };

    let paths: Vec<(String, &JsonValue)> = rows
        .iter()
        .map(|(path, row)| (verbatim(path), row))
        .collect();
    let column = paths
        .iter()
        .map(|(path, _)| visible_width_with_policy(path, width))
        .max()
        .unwrap_or_default();
    let verbs = if planning {
        PLANNED_VERBS
    } else {
        APPLIED_VERBS
    };

    let mut tally = Tally::default();
    let mut lines = Vec::with_capacity(paths.len());
    for (path, row) in paths {
        let shape = row.get("facts").and_then(|facts| facts.get("shape"));
        let target = shape
            .and_then(|shape| shape.get("Symlink"))
            .and_then(|symlink| symlink.get("target"))
            .and_then(JsonValue::as_str);
        let (verdict, fields) = named(row.get("verdict"));
        let (style, verb) = match spelling(verdict, target.is_some()) {
            Some((style, verb)) => (style, verb.to_owned()),
            None => ("unknown", verbatim(verdict)),
        };
        if let Some(counted) = counted(verdict) {
            tally.count(counted);
        }
        let qualifier = fields
            .and_then(|fields| fields.get("reason"))
            .and_then(JsonValue::as_str)
            .map(overwriting);
        let note = note(target, qualifier, executable(shape));

        lines.push(RowView {
            style,
            verb_pad: pad(verbs, &verb, width),
            verb,
            path_pad: pad(column, &path, width),
            path,
            note,
        });
    }

    RunLines {
        rows: lines,
        summary: (!planning).then(|| tally.summary()),
    }
}

/// A link names its target, an overwrite says why, and a file left with
/// neither says only that it is executable.
fn note(target: Option<&str>, qualifier: Option<&str>, executable: bool) -> Option<String> {
    match (target, qualifier) {
        (Some(target), Some(qualifier)) => Some(format!("-> {}  {qualifier}", verbatim(target))),
        (Some(target), None) => Some(format!("-> {}", verbatim(target))),
        (None, Some(qualifier)) => Some(qualifier.to_owned()),
        (None, None) => executable.then(|| "(exec)".to_owned()),
    }
}

/// A verdict is a name, or a name carrying fields; a row reads both.
fn named(verdict: Option<&JsonValue>) -> (&str, Option<&JsonValue>) {
    match verdict {
        Some(JsonValue::String(name)) => (name, None),
        Some(JsonValue::Object(fields)) => fields
            .iter()
            .next()
            .map_or(("", None), |(name, body)| (name.as_str(), Some(body))),
        _ => ("", None),
    }
}

fn executable(shape: Option<&JsonValue>) -> bool {
    shape
        .and_then(|shape| shape.get("File"))
        .and_then(|file| file.get("executable"))
        .and_then(JsonValue::as_bool)
        .unwrap_or_default()
}

fn pad(column: usize, cell: &str, width: AmbiguousWidth) -> String {
    " ".repeat(column.saturating_sub(visible_width_with_policy(cell, width)))
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
