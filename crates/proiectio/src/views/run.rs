//! The document a write pass renders — `write`'s and `rm`'s alike — and the
//! lines its template lays out.
//!
//! The lines reach the template through Standout's context injection, which
//! structured modes skip, so `--output json` stays the library's own report.

use std::collections::BTreeSet;

use libproiectio::{ApplyReport, BlockFault, Dropped, PlannedAction, Report};
use serde::Serialize;
use serde_json::Value as JsonValue;
use standout::AmbiguousWidth;
use standout::tabular::visible_width_with_policy;

use crate::app::verbatim;
use crate::views::pad;

/// A plan on a dry run, what apply did on a real one; untagged, so structured
/// output is the library's own either way.
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum RunView {
    Planned(PlannedRun),
    Applied(Box<ApplyReport>),
}

/// What a dry run has to report: the plan's rows, and the archive members
/// `strip` erased on the way to the desired tree. Apply pairs the same two on
/// [`ApplyReport`]; a plan has no such struct to sit on, so the rows flatten
/// into this one and both documents carry `dropped` at their top level.
#[derive(Serialize)]
pub(crate) struct PlannedRun {
    #[serde(flatten)]
    pub(crate) report: Report<PlannedAction>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) dropped: BTreeSet<Dropped>,
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
        ("Forget", _) => ("removed", "would forget"),
        ("Forgot", _) => ("removed", "forgot"),
        ("Release", _) => ("removed", "would release"),
        ("Released", _) => ("removed", "released"),
        // The one verdict a dry run and a real run spell alike: neither one
        // did anything, and the row is there to say the path was named.
        ("NotRecorded", _) => ("skipped", "no record"),
        ("Refuse", _) => ("refused", "would refuse"),
        _ => return None,
    })
}

/// The column a real run counts a verdict under; a plan counts nothing.
fn counted(verdict: &str) -> Option<Counted> {
    Some(match verdict {
        "Written" | "Overwritten" => Counted::Wrote,
        "Skipped" => Counted::Skipped,
        "Removed" => Counted::Removed,
        "Forgot" => Counted::Forgot,
        "Released" => Counted::Released,
        "NotRecorded" => Counted::NotRecorded,
        _ => return None,
    })
}

/// What a verdict's payload says about the row: why a plan would overwrite a
/// path, or why it would refuse one and which source named it.
fn qualifying(fields: &JsonValue, facts: Option<&JsonValue>) -> Option<String> {
    if let Some(reason) = fields.get("reason").and_then(JsonValue::as_str) {
        return Some(overwriting(reason).to_owned());
    }
    let refused = refusing(fields.get("refusal")?);
    Some(match sourcing(facts) {
        Some(source) => format!("{refused} {source}"),
        None => refused,
    })
}

/// Which source named a refused path, in the phrase `Origin`'s own message
/// spells it with; `None` where the caller named the path itself, or the row
/// states no source.
fn sourcing(facts: Option<&JsonValue>) -> Option<String> {
    origin_phrase(facts?.get("origin"), None)
}

/// Which source named something, in the phrase `Origin`'s own message spells
/// it with. `into` is the place in the destination an expansion puts an
/// archive, which only a dropped member has to say.
fn origin_phrase(origin: Option<&JsonValue>, into: Option<&str>) -> Option<String> {
    let (kind, payload) = named(origin);
    let string = |field| payload?.get(field).and_then(JsonValue::as_str);
    let phrase = match kind {
        "Mapping" => format!("from mapping {}", verbatim(string("path")?)),
        "Tree" => format!("from tree {}", verbatim(string("path")?)),
        "Archive" => {
            let mut phrase = format!("from archive {}", verbatim(string("path")?));
            if let Some(prefix) = into {
                phrase.push_str(&format!(" into {}", verbatim(prefix)));
            }
            if let Some(mapping) = string("via") {
                phrase.push_str(&format!(", named by mapping {}", verbatim(mapping)));
            }
            phrase
        }
        "Files" => "from individually named files".to_owned(),
        _ => return None,
    };
    Some(format!("({phrase})"))
}

/// Why a plan would overwrite a path.
fn overwriting(reason: &str) -> &'static str {
    match reason {
        "ContentChanged" => "(content changed)",
        "ExecutableChanged" => "(executable changed)",
        _ => "(drifted, forced)",
    }
}

/// Why a plan refuses a path: the refusal's own name, in the vocabulary the
/// exit table names the refusal kinds with, and what the refusal carries after
/// it, in the words `Refusal`'s own message spells the payload with.
fn refusing(refusal: &JsonValue) -> String {
    let (kind, payload) = named(Some(refusal));
    let spelled = match kind {
        "Containment" => "containment",
        "TreeConflict" => "tree conflict",
        "Foreign" => "foreign",
        "Drift" => "drifted",
        "OwnerConflict" => "owner conflict",
        "ExternalTarget" => "external target",
        "InvalidTarget" => "invalid target",
        "Block" => "block",
        unknown => return format!("({})", verbatim(unknown)),
    };
    match payload.and_then(|payload| detailing(kind, payload)) {
        Some(detail) => format!("({spelled}) {detail}"),
        None => format!("({spelled})"),
    }
}

/// What a refusal's payload says beyond its name: the keys claiming the same
/// location, the owners holding the path, the offending target — quoted, as
/// the library quotes it, where it is not a path — or the rule a block entry
/// broke. `None` where the kind carries nothing, or where the payload is not
/// the shape the library serializes.
fn detailing(kind: &str, payload: &JsonValue) -> Option<String> {
    let string = |field| payload.get(field).and_then(JsonValue::as_str);
    match kind {
        "TreeConflict" => Some(format!("(with {})", listed(payload.get("paths")?, ", ")?)),
        "OwnerConflict" => Some(format!(
            "(held by {})",
            listed(payload.get("owners")?, "+")?
        )),
        "ExternalTarget" => Some(format!("-> {}", verbatim(string("target")?))),
        "InvalidTarget" => Some(format!(
            "-> {}",
            verbatim(&format!("{:?}", string("target")?))
        )),
        "Block" => Some(format!("({})", faulting(string("fault")?))),
        _ => None,
    }
}

/// Every fault a block entry is refused for, as the library declares them.
const BLOCK_FAULTS: [BlockFault; 10] = [
    BlockFault::MarkerEmpty,
    BlockFault::MarkerNotOneLine,
    BlockFault::MarkerEdgeWhitespace,
    BlockFault::BodyCarriesMarker,
    BlockFault::BodyNotNewlineTerminated,
    BlockFault::ContainerNotNewlineTerminated,
    BlockFault::ContainerMissing,
    BlockFault::KindChange,
    BlockFault::SignatureNotRecorded,
    BlockFault::MarkerInAuthorText,
];

/// What a block refusal's fault says: the sentence `BlockFault`'s own message
/// spells, found by the name the library serializes the fault under; a fault
/// this CLI does not know reads as that name, escaped.
fn faulting(name: &str) -> String {
    BLOCK_FAULTS
        .into_iter()
        .find(|fault| fault_name(*fault).as_deref() == Some(name))
        .map_or_else(|| verbatim(name), |fault| fault.to_string())
}

/// The name the library serializes one fault under, taken from the fault.
fn fault_name(fault: BlockFault) -> Option<String> {
    match serde_json::to_value(fault) {
        Ok(JsonValue::String(name)) => Some(name),
        _ => None,
    }
}

/// One payload's list of strings, each escaped, joined the way the library
/// joins that field. `None` where the field holds no strings.
fn listed(values: &JsonValue, separator: &str) -> Option<String> {
    let items: Vec<String> = values
        .as_array()?
        .iter()
        .filter_map(JsonValue::as_str)
        .map(verbatim)
        .collect();
    (!items.is_empty()).then(|| items.join(separator))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Counted {
    Wrote,
    Skipped,
    Removed,
    Forgot,
    Released,
    NotRecorded,
}

#[derive(Default)]
struct Tally {
    wrote: usize,
    skipped: usize,
    removed: usize,
    forgot: usize,
    released: usize,
    not_recorded: usize,
}

impl Tally {
    fn count(&mut self, counted: Counted) {
        let column = match counted {
            Counted::Wrote => &mut self.wrote,
            Counted::Skipped => &mut self.skipped,
            Counted::Removed => &mut self.removed,
            Counted::Forgot => &mut self.forgot,
            Counted::Released => &mut self.released,
            Counted::NotRecorded => &mut self.not_recorded,
        };
        *column += 1;
    }

    /// A pass that projected reports what it wrote and skipped, and names a
    /// cleared column only where it holds something; one that only cleared
    /// paths reports what it cleared and nothing else. A pass that left every
    /// path alone reports what it left alone, and one that touched nothing
    /// says so. Paths the owner turned out not to hold are counted apart from
    /// all of it: the run did nothing at them, and a summary reading `nothing
    /// to do` over rows naming them would be the thing the count is there to
    /// prevent.
    fn summary(&self) -> String {
        let cleared = self.removed + self.forgot + self.released;
        if self.wrote == 0 && cleared == 0 && self.not_recorded == 0 {
            return match self.skipped {
                0 => "nothing to do".to_owned(),
                skipped => format!("{skipped} unchanged"),
            };
        }
        let mut columns = Vec::new();
        if self.wrote > 0 || self.skipped > 0 {
            columns.push(format!("{} written, {} skipped", self.wrote, self.skipped));
        }
        for (count, column) in [
            (self.removed, "removed"),
            (self.forgot, "forgotten"),
            (self.released, "released"),
            (self.not_recorded, "not recorded"),
        ] {
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

const DROPPED: &str = "dropped";

/// The lines `run.jinja` prints for one write-pass document.
pub(crate) fn lines(document: &JsonValue, width: AmbiguousWidth) -> RunLines {
    let (report, planning) = match document.get("report") {
        Some(applied) => (applied, false),
        None => (document, true),
    };
    let Some(rows) = report.get("rows").and_then(JsonValue::as_array) else {
        return RunLines::default();
    };

    let paths: Vec<(String, &JsonValue)> = rows
        .iter()
        .filter_map(|row| Some((verbatim(row.get("path")?.as_str()?), row)))
        .collect();
    // A plan flattens its rows into the document that carries `dropped`, and
    // an apply nests its rows under `report` beside it, so drops read from
    // the top level in both tenses.
    let dropped: Vec<(String, String)> = document
        .get("dropped")
        .and_then(JsonValue::as_array)
        .map(|records| {
            records
                .iter()
                .filter_map(|record| {
                    let member = record.get("member").and_then(JsonValue::as_str)?;
                    Some((verbatim(member), stripping(record)))
                })
                .collect()
        })
        .unwrap_or_default();
    let column = paths
        .iter()
        .map(|(path, _)| path)
        .chain(dropped.iter().map(|(member, _)| member))
        .map(|cell| visible_width_with_policy(cell, width))
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
        let facts = row.get("facts");
        let shape = facts.and_then(|facts| facts.get("shape"));
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
        let qualifier = fields.and_then(|fields| qualifying(fields, facts));
        let note = note(target, qualifier.as_deref(), executable(shape));

        lines.push(RowView {
            style,
            verb_pad: pad(verbs, &verb, width),
            verb,
            path_pad: pad(column, &path, width),
            path,
            note,
        });
    }
    for (member, note) in dropped {
        lines.push(RowView {
            style: "skipped",
            verb_pad: pad(verbs, DROPPED, width),
            verb: DROPPED.to_owned(),
            path_pad: pad(column, &member, width),
            path: member,
            note: Some(note),
        });
    }

    RunLines {
        rows: lines,
        summary: (!planning).then(|| tally.summary()),
    }
}

/// What one dropped member's row says: the `strip` count that left it with no
/// path, and the archive that carried it — with the place that archive was
/// bound for, which is what tells two entries expanding one archive apart.
fn stripping(record: &JsonValue) -> String {
    let reason = match record.get("strip").and_then(JsonValue::as_u64) {
        Some(strip) => format!("(no path left after strip {strip})"),
        None => "(no path left after strip)".to_owned(),
    };
    let prefix = record
        .get("prefix")
        .and_then(JsonValue::as_str)
        .filter(|prefix| !prefix.is_empty());
    match origin_phrase(record.get("origin"), prefix) {
        Some(source) => format!("{reason} {source}"),
        None => reason,
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

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
