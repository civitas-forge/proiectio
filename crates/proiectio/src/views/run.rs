//! The document a write pass renders — `write`'s and `rm`'s alike — the lines
//! its template lays out, and the columns `--output csv` writes.
//!
//! The lines reach the template through Standout's context injection, which
//! structured modes skip, so `--output json` stays the library's own rows.

use std::collections::{BTreeMap, BTreeSet};
use std::iter;

use camino::Utf8PathBuf;
use libproiectio::{
    ApplyOutcome, ApplyReport, BlockFault, Dropped, Error, Manifest, PathFacts, PlannedAction,
    RefusalKind, Refused, Report, Row, Stopped,
};
use serde::Serialize;
use serde_json::Value as JsonValue;
use standout::AmbiguousWidth;
use standout::tabular::visible_width_with_policy;
use standout::{CsvProjection, StructuredOutputProjection};

use crate::app::verbatim;
use crate::views::cells;
use crate::views::pad;

/// Rows a run states without having acted on them, what apply did, or — where
/// a run stopped part-way — both at once.
///
/// One shape for the three: `phase` names the tense, and every arm states its
/// rows at `rows` — every path the pass has a verdict for, refusals included.
/// A reader branches on the field rather than on which keys the document
/// happens to carry, and one CSV projection selects the rows of all three. The
/// verdict vocabularies stay per-tense — a plan says `Write` where a run says
/// `Written` — which is what `phase` is there to tell apart.
#[derive(Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub(crate) enum RunView {
    Planned(PlannedRun),
    Applied(Box<AppliedRun>),
    Aborted(Box<AbortedRun>),
}

/// The tense [`RunView::Planned`] names itself by, which is the one tense whose
/// rows nothing has acted on.
const PLANNED: &str = "planned";

/// The rows a pass states rather than performs — a dry run's whole plan, or
/// the paths a refusal declined — and the archive members `strip` erased on
/// the way to the desired tree.
#[derive(Serialize)]
pub(crate) struct PlannedRun {
    #[serde(flatten)]
    pub(crate) report: Report<PlannedAction>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) dropped: BTreeSet<Dropped>,
}

impl PlannedRun {
    /// The document a run that acted on nothing renders for a refusal the
    /// library reported as an error: the refused rows, and the archive members
    /// `strip` erased on the way to the desired tree. A refusal the deciding
    /// stages raise strips nothing, so its document carries no `dropped`; one
    /// an apply raises before its first action lands does, the archive having
    /// been expanded to decide the plan that then refused.
    pub(crate) fn refused(
        refused: &Refused,
        manifest: &Manifest,
        dropped: BTreeSet<Dropped>,
    ) -> PlannedRun {
        PlannedRun {
            report: refused_rows(refused, manifest),
            dropped,
        }
    }
}

/// What a run applied: the rows, the archive members `strip` erased, and the
/// manifest the run decided on. [`ApplyReport`] nests its rows a level down,
/// under `report`; here they flatten, so an applied document states its rows
/// where a planned one states them.
#[derive(Serialize)]
pub(crate) struct AppliedRun {
    #[serde(flatten)]
    pub(crate) report: Report<ApplyOutcome>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) dropped: BTreeSet<Dropped>,
    pub(crate) manifest: Manifest,
}

/// The one place an [`ApplyReport`] becomes a CLI document, for the applied
/// tense and the stopped one alike: it destructures the library's report whole,
/// so a field the library adds fails to compile here rather than going missing
/// from both documents.
impl From<ApplyReport> for AppliedRun {
    fn from(applied: ApplyReport) -> AppliedRun {
        let ApplyReport {
            report,
            dropped,
            manifest,
        } = applied;
        AppliedRun {
            report,
            dropped,
            manifest,
        }
    }
}

/// The rows a refusal states on its own, for the stages that report one as an
/// error rather than as a plan: one row per refused key, carrying the refusal,
/// the source that named the key, and the owners the manifest records at it —
/// the same owners a plan's own refused row states, so the two stages state a
/// refusal alike. A refusal names no shape, as a plan's refused rows do not.
pub(crate) fn refused_rows(refused: &Refused, manifest: &Manifest) -> Report<PlannedAction> {
    Report {
        rows: refused
            .paths()
            .iter()
            .map(|(path, declined)| {
                let row = Row {
                    facts: Some(PathFacts {
                        shape: None,
                        owners: manifest
                            .entries
                            .get(path)
                            .map(|recorded| recorded.owners.clone())
                            .unwrap_or_default(),
                        origin: Some(declined.origin.clone()),
                    }),
                    verdict: PlannedAction::Refuse {
                        refusal: declined.refusal.clone(),
                    },
                };
                (path.clone(), row)
            })
            .collect(),
    }
}

/// Which half of a run stopped it, as the library's own [`Stopped`] splits
/// them: an action stopping the run leaves the actions after it unapplied,
/// while a record stopping it leaves the destination holding the whole plan.
/// A reader tells the two apart from this rather than from the wording of a
/// line, and a run that applied everything is never told it stopped part-way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StoppedAt {
    /// An action refused or failed; the actions after it never ran, and the
    /// state directory records the rows before it.
    Applying,
    /// An action refused or failed and the record of the rows before it failed
    /// as well.
    ApplyingAndRecording,
    /// Every action applied and only the record failed.
    Recording,
}

/// One verdict of a run that could not finish: what an action did to the path,
/// or — for a key the run declined — the refusal, in the words the planning
/// stages state a refusal in, so one refusal reads alike whichever stage met
/// it.
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum StoppedVerdict {
    Applied(ApplyOutcome),
    Refused(PlannedAction),
}

/// The document a run that could not finish renders: every path it has a
/// verdict for — what it applied, and the keys a refusal declined — and, past
/// the rows, what stopped it. The `aborted` phase says the destination holds
/// the applied rows, and `stopped_at` says whether any action is missing from
/// them.
///
/// The refused keys are rows of this one sequence rather than a second one
/// beside it: a reader of any format, `--output csv` included, reads the whole
/// per-path story of the run from `rows` alone. What stopped the run is not a
/// path, so `stopped`, `recorded` and `stopped_at` stay beside the rows and out
/// of the CSV; a structured caller reads those from the stderr sentences
/// [`warnings`] states them in, and the exit code says which verdict the run
/// left with.
#[derive(Serialize)]
pub(crate) struct AbortedRun {
    /// Every path the run has a verdict for, in path order: the actions that
    /// landed, and the keys the run refused and acted on none of.
    #[serde(flatten)]
    pub(crate) report: Report<StoppedVerdict>,
    /// The archive members `strip` erased, which reached no path and so are no
    /// rows.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) dropped: BTreeSet<Dropped>,
    /// The manifest the run decided on.
    pub(crate) manifest: Manifest,
    /// Whether the state directory records the applied rows, which only a run
    /// stopped at an action leaves it doing: either other phase failed writing
    /// the manifest, so what the destination holds went unrecorded and a later
    /// run judges it against the record that stood before this one.
    pub(crate) recorded: bool,
    /// What stopped the run and what stopped its record, where the rows do
    /// not already say it: the failure a non-refusal stopped with, and the
    /// failure to write the manifest — each stated once. Empty for a refusal
    /// that recorded what it applied, whose refused rows say the whole of it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) stopped: Vec<String>,
    /// Which half of the run stopped it.
    pub(crate) stopped_at: StoppedAt,
}

impl AbortedRun {
    pub(crate) fn new(
        applied: ApplyReport,
        refused: Report<PlannedAction>,
        stopped: &Stopped,
    ) -> AbortedRun {
        let (stopped_at, stated) = match stopped {
            Stopped::Applying(error) => (StoppedAt::Applying, Vec::from_iter(failing(error))),
            Stopped::ApplyingAndRecording {
                applying,
                recording,
            } => (
                StoppedAt::ApplyingAndRecording,
                failing(applying)
                    .into_iter()
                    .chain([unrecorded(recording)])
                    .collect(),
            ),
            Stopped::Recording(error) => (StoppedAt::Recording, vec![unrecorded(error)]),
        };
        let AppliedRun {
            report,
            dropped,
            manifest,
        } = AppliedRun::from(applied);
        AbortedRun {
            report: Report {
                rows: ran(report, refused),
            },
            dropped,
            manifest,
            recorded: stopped.recorded(),
            stopped: stated,
            stopped_at,
        }
    }
}

/// The two halves of a stopped run's per-path story as one sequence: the rows
/// its actions landed, and the rows it refused, each keeping the verdict its
/// own stage stated. The two name disjoint paths — a run acts on nothing it
/// refuses — and a key that reached both stages reads as the refusal, which is
/// what the run left the path at.
fn ran(
    applied: Report<ApplyOutcome>,
    refused: Report<PlannedAction>,
) -> BTreeMap<Utf8PathBuf, Row<StoppedVerdict>> {
    let mut rows: BTreeMap<Utf8PathBuf, Row<StoppedVerdict>> =
        carried(applied, StoppedVerdict::Applied).collect();
    rows.extend(carried(refused, StoppedVerdict::Refused));
    rows
}

/// One stage's rows under the [`StoppedVerdict`] arm that stage's verdicts
/// state themselves in.
fn carried<V>(
    report: Report<V>,
    stated: fn(V) -> StoppedVerdict,
) -> impl Iterator<Item = (Utf8PathBuf, Row<StoppedVerdict>)> {
    report.rows.into_iter().map(move |(path, row)| {
        (
            path,
            Row {
                verdict: stated(row.verdict),
                facts: row.facts,
            },
        )
    })
}

/// The sentence a failure reaches the reader by, and `None` for a refusal,
/// whose refused rows state it in the table instead.
fn failing(error: &Error) -> Option<String> {
    (!error.is_refusal()).then(|| error.to_string())
}

/// The sentence a manifest that never reached the state directory reaches the
/// reader by.
fn unrecorded(error: &Error) -> String {
    format!("the state directory does not record what the run applied: {error}")
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

/// What `run.jinja` prints: one row per path, what lifts the refusals among
/// them, the count a real run closes with, and what a run that stopped says
/// past that count.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct RunLines {
    pub(crate) rows: Vec<RowView>,
    /// One line per refusal kind the rows carry that something lifts, in the
    /// order the kinds first appear. Once per kind rather than once per row:
    /// fifty drifted paths are one `--force` away, and saying so fifty times
    /// buries the paths.
    pub(crate) hints: Vec<String>,
    pub(crate) summary: Option<String>,
    pub(crate) stopped: Vec<String>,
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
        ("Refused", _) => ("refused", "refused"),
        _ => return None,
    })
}

/// A refusal reads in the tense of the pass that met it: a plan says what it
/// would refuse, a run that had already applied rows says what it refused.
fn tensed(verdict: &str, planning: bool) -> &str {
    match (verdict, planning) {
        ("Refuse", false) => "Refused",
        _ => verdict,
    }
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
        "Refused" => Counted::Refused,
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
        "DirectoryInTheWay" => "directory in the way",
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
        "Containment" => Some(format!(
            "(below the symlink {})",
            verbatim(string("through")?)
        )),
        "TreeConflict" => Some(format!("(with {})", listed(payload.get("paths")?, ", ")?)),
        "OwnerConflict" => Some(format!(
            "(held by {})",
            listed(payload.get("owners")?, "+")?
        )),
        "DirectoryInTheWay" => {
            let mut clauses = Vec::new();
            if let Some(held) = payload.get("holding").and_then(holding) {
                clauses.push(format!("holding {held}, which --force does not remove"));
            }
            if let Some(names) = payload.get("unreadable").and_then(|it| listed(it, ", ")) {
                clauses.push(format!("holding names that are not UTF-8 in {names}"));
            }
            (!clauses.is_empty()).then(|| format!("({})", clauses.join(", and ")))
        }
        "ExternalTarget" => Some(format!("-> {}", verbatim(string("target")?))),
        "InvalidTarget" => Some(format!(
            "-> {}",
            verbatim(&format!("{:?}", string("target")?))
        )),
        "Block" => Some(format!("({})", faulting(string("fault")?))),
        _ => None,
    }
}

/// Every kind a path is refused for, as the library declares them.
const REFUSAL_KINDS: [RefusalKind; 9] = [
    RefusalKind::Containment,
    RefusalKind::TreeConflict,
    RefusalKind::Foreign,
    RefusalKind::Drift,
    RefusalKind::DirectoryInTheWay,
    RefusalKind::OwnerConflict,
    RefusalKind::ExternalTarget,
    RefusalKind::InvalidTarget,
    RefusalKind::Block,
];

/// What lifts a refusal of the kind the library serializes under `name`, in
/// the library's own words; `None` for a kind nothing lifts, and for one this
/// CLI does not know.
///
/// `forced` says the invocation carried `--force`, which drops the drift hint:
/// that hint names the flag that lifts drift, and a run refusing drift with the
/// flag already on has met the drift no policy lifts. The reader took the
/// advice; printing it again would send them back to where they are.
fn hinting(name: &str, forced: bool) -> Option<&'static str> {
    let kind = REFUSAL_KINDS
        .into_iter()
        .find(|kind| kind_name(*kind).as_deref() == Some(name))?;
    if forced && kind == RefusalKind::Drift {
        return None;
    }
    kind.override_hint()
}

/// The name the library serializes one refusal kind under, taken from the kind.
fn kind_name(kind: RefusalKind) -> Option<String> {
    match serde_json::to_value(kind) {
        Ok(JsonValue::String(name)) => Some(name),
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

/// What a directory holds, in the words the library spells it with: each
/// node, escaped, with the owners recording it where any do. `None` where the
/// directory holds nothing, which the library says by saying nothing.
fn holding(nodes: &JsonValue) -> Option<String> {
    let items: Vec<String> = nodes
        .as_object()?
        .iter()
        .map(|(node, owners)| match listed(owners, "+") {
            Some(owners) => format!("{} (held by {owners})", verbatim(node)),
            None => verbatim(node),
        })
        .collect();
    (!items.is_empty()).then(|| items.join(", "))
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
    Refused,
}

#[derive(Default)]
struct Tally {
    wrote: usize,
    skipped: usize,
    removed: usize,
    forgot: usize,
    released: usize,
    not_recorded: usize,
    refused: usize,
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
            Counted::Refused => &mut self.refused,
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
    /// prevent. So are the paths a run that stopped part-way refused.
    fn summary(&self) -> String {
        let cleared = self.removed + self.forgot + self.released;
        if self.wrote == 0 && cleared == 0 && self.not_recorded == 0 && self.refused == 0 {
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
            (self.refused, "refused"),
        ] {
            if count > 0 {
                columns.push(format!("{count} {column}"));
            }
        }
        columns.join(", ")
    }
}

/// The verb column: the widest verb a plan spells, and the widest a real run
/// spells. Either is the least that tense's column is ever wide — a verdict
/// this CLI has no word for reads as its own name, and a longer one widens the
/// column. The tests drive every verdict the library declares through
/// `spelling` and check the verb still fits its tense's constant.
const PLANNED_VERBS: usize = "would overwrite".len();
const APPLIED_VERBS: usize = "overwrote".len();

const DROPPED: &str = "dropped";

/// The lines `run.jinja` prints for one write-pass document.
pub(crate) fn lines(document: &JsonValue, width: AmbiguousWidth, forced: bool) -> RunLines {
    // The document names its own tense, and every tense states its rows at
    // `rows`: a plan says `Write` where a run says `Written`, and nothing about
    // where a key sits says which of the two this is.
    let planning = document.get("phase").and_then(JsonValue::as_str) == Some(PLANNED);
    let Some(rows) = document.get("rows").and_then(JsonValue::as_array) else {
        return RunLines::default();
    };
    // The keys a stopped run refused are rows of that one sequence, in the
    // path order the library states every report in, so they lay out in the
    // same table as the rows it applied; only the tense tells the two apart.
    let paths: Vec<(String, &JsonValue)> = rows
        .iter()
        .filter_map(|row| Some((verbatim(row.get("path")?.as_str()?), row)))
        .collect();
    // A dropped member reached no path, so it is no row of the report; it
    // rides beside the rows in every tense.
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
    let tense = if planning {
        PLANNED_VERBS
    } else {
        APPLIED_VERBS
    };

    let mut tally = Tally::default();
    let mut hints: Vec<String> = Vec::new();
    let mut lines = Vec::with_capacity(paths.len());
    for (path, row) in paths {
        let facts = row.get("facts");
        let shape = facts.and_then(|facts| facts.get("shape"));
        let target = shape
            .and_then(|shape| shape.get("Symlink"))
            .and_then(|symlink| symlink.get("target"))
            .and_then(JsonValue::as_str);
        let (verdict, fields) = named(row.get("verdict"));
        let verdict = tensed(verdict, planning);
        let (style, verb) = match spelling(verdict, target.is_some()) {
            Some((style, verb)) => (style, verb.to_owned()),
            None => ("unknown", verbatim(verdict)),
        };
        if let Some(counted) = counted(verdict) {
            tally.count(counted);
        }
        let qualifier = fields.and_then(|fields| qualifying(fields, facts));
        let note = note(target, qualifier.as_deref(), executable(shape));
        if let Some(hint) = fields
            .and_then(|fields| fields.get("refusal"))
            .and_then(|refusal| hinting(named(Some(refusal)).0, forced))
            .map(str::to_owned)
            && !hints.contains(&hint)
        {
            hints.push(hint);
        }

        lines.push(RowView {
            style,
            verb_pad: String::new(),
            verb,
            path_pad: pad(column, &path, width),
            path,
            note,
        });
    }
    for (member, note) in dropped {
        lines.push(RowView {
            style: "skipped",
            verb_pad: String::new(),
            verb: DROPPED.to_owned(),
            path_pad: pad(column, &member, width),
            path: member,
            note: Some(note),
        });
    }
    // A verdict this CLI has no word for reads as its own name, which can run
    // longer than either tense's widest verb, so the column takes the widest
    // verb it actually holds rather than spilling one path out of line.
    let verbs = lines
        .iter()
        .map(|row| visible_width_with_policy(&row.verb, width))
        .chain(iter::once(tense))
        .max()
        .unwrap_or(tense);
    for row in &mut lines {
        row.verb_pad = pad(verbs, &row.verb, width);
    }

    RunLines {
        rows: lines,
        hints,
        summary: (!planning).then(|| closing(&tally, document)),
        stopped: stopping(document),
    }
}

/// The line a real run closes with: its counts, and — where the run could not
/// finish — how far it got, in the split the library's own `Stopped` makes. A
/// run an action stopped left the plan half applied and the rows above are
/// what stands of it; a run only the record stopped applied every one of them,
/// and telling that reader it stopped part-way would send them looking for
/// actions no destination is missing.
fn closing(tally: &Tally, document: &JsonValue) -> String {
    let counts = tally.summary();
    match reached(document) {
        Some(stage) => format!("{counts} — {stage}"),
        None => counts,
    }
}

/// How far a run that could not finish got, in the split the library's own
/// `Stopped` makes; `None` for a document stating no stage, which is every
/// document but a stopped run's.
fn reached(document: &JsonValue) -> Option<&'static str> {
    match document.get("stopped_at").and_then(JsonValue::as_str)? {
        "applying" | "applying_and_recording" => Some(PART_WAY),
        "recording" => Some(WHOLE_PLAN),
        _ => None,
    }
}

const PART_WAY: &str = "the run stopped part-way through the plan, and what it applied stands";
const WHOLE_PLAN: &str = "the run applied its whole plan and could not record it";

/// What a run whose manifest never reached the state directory leaves behind,
/// which no row of the report says: the record in the state directory is the
/// one the run found there, so it says nothing of what this run applied, and
/// that older record is what the next run judges the destination against.
///
/// Which classification each path then reads as is not stated here. The save
/// replaces the manifest whole, so a path this run overwrote, skipped or
/// removed can be recorded already and read as clean, drifted or missing; only
/// a path nothing recorded before reads as foreign. Deriving the per-path
/// answer would mean keeping the pre-run manifest to replay the next run's
/// deciding against, which is recovery machinery this CLI does not carry.
const UNRECORDED: &str = "nothing in the state directory records what this run applied, \
     so the next run over this destination judges it against the record that stood \
     before the run";

/// The run-level facts a stopped run states on stderr rather than in the rows:
/// how far it got, what stopped it, and — where the manifest never landed —
/// what the destination is left holding. Empty for every document that is not
/// a stopped run's.
///
/// These are the facts the records cannot carry. A CSV record is one path and
/// these are about the run, and the exit code separates a failure from a
/// refusal without saying which half of the run met it; a caller reading only
/// stdout would take a run that wrote its plan and lost the manifest for one
/// that finished.
pub(crate) fn warnings(document: &JsonValue) -> Vec<String> {
    if document.get("stopped_at").is_none() {
        return Vec::new();
    }
    let mut stated: Vec<String> = reached(document).map(str::to_owned).into_iter().collect();
    stated.extend(stopped(document).map(str::to_owned));
    if document.get("recorded").and_then(JsonValue::as_bool) == Some(false) {
        stated.push(UNRECORDED.to_owned());
    }
    stated
}

/// What a run that stopped says past its counts: the failure that stopped it
/// where no refused row states it, and what the state directory does not
/// record. These reach a reader of the rendered output here, in the body,
/// rather than on the stderr channel [`warnings`] states them on: only a
/// structured mode leaves the body unable to carry them.
fn stopping(document: &JsonValue) -> Vec<String> {
    stopped(document).map(verbatim).collect()
}

/// The sentences a stopped run states past its rows, as the document spells
/// them: the failure that stopped it, and the record it could not write.
fn stopped(document: &JsonValue) -> impl Iterator<Item = &str> {
    document
        .get("stopped")
        .and_then(JsonValue::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(JsonValue::as_str)
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

/// The columns `--output csv` writes for a write pass, one record per path
/// under a header that does not move: the rows sit at `rows` in every tense, so
/// one projection selects a plan's, an apply's and a stopped run's alike — and
/// a stopped run states the keys it refused among those rows, so the CSV names
/// the paths the run failed on rather than only the ones it got through.
///
/// The verdict cell is the variant name, and `detail` is what that variant
/// carries — `Overwrite` its reason, `Refuse` its refusal — which has no cell
/// of its own. It is empty for the verdicts that carry nothing.
///
/// The last cell is the one run-level fact every record carries: `phase`, the
/// tense the document names itself by, the same word in every record of one
/// run. It is here because it is what reads the verdict cell: the vocabularies
/// are per-tense and both of them spell `NotRecorded`, so a dry run's record
/// and an applied run's can otherwise be the same bytes.
///
/// The rest of what a document states that is not about one path takes no
/// record here. A dropped archive member reached no path in the destination, so
/// it has no path cell to fill. `manifest` states the destination rather than
/// this run. And `stopped`, `recorded` and `stopped_at` are diagnostics of the
/// run, which reach a structured caller on stderr, in the sentences
/// [`warnings`] states — beside the exit code, 1 for a failure and 2 for a
/// refusal whatever the output mode.
pub(crate) fn csv() -> StructuredOutputProjection {
    StructuredOutputProjection::csv(
        CsvProjection::builder("rows")
            .column(cells::column("path"))
            .derived_column(cells::header("verdict"), |row, _| {
                cells::cell(Some(named(row.get("verdict")).0.to_owned()))
            })
            .derived_column(cells::header("detail"), |row, _| cells::cell(detail(row)))
            .derived_column(cells::header("shape"), |row, _| {
                cells::cell(cells::shape(row))
            })
            .derived_column(cells::header("executable"), |row, _| {
                cells::cell(cells::executable(row))
            })
            .derived_column(cells::header("target"), |row, _| cells::cell(target(row)))
            .derived_column(cells::header("owners"), |row, _| {
                cells::cell(cells::owners(row))
            })
            .derived_column(cells::header("origin"), |row, _| cells::cell(origin(row)))
            .derived_column(cells::header("phase"), |_, document| {
                cells::cell(
                    document
                        .get("phase")
                        .and_then(JsonValue::as_str)
                        .map(str::to_owned),
                )
            })
            .build(),
    )
}

/// What the row's verdict carries past its name; nothing for a verdict that is
/// a name alone.
fn detail(row: &JsonValue) -> Option<String> {
    flat(named(row.get("verdict")).1?)
}

/// Where the row's link points, for a row stating a link that names one.
fn target(row: &JsonValue) -> Option<String> {
    Some(
        row.get("facts")?
            .get("shape")?
            .get("Symlink")?
            .get("target")?
            .as_str()?
            .to_owned(),
    )
}

/// Which source named the path; nothing for a row stating none.
fn origin(row: &JsonValue) -> Option<String> {
    flat(row.get("facts")?.get("origin")?)
}

/// One structured value in one cell: a bare name as the name, and anything
/// carrying fields as the JSON it is stated in. Nothing spells the fields out,
/// because what a payload carries differs by variant and a column cannot hold
/// one shape per variant; the JSON keeps every one of them readable back.
fn flat(value: &JsonValue) -> Option<String> {
    match value {
        JsonValue::Null => None,
        JsonValue::String(name) => Some(name.clone()),
        carried => serde_json::to_string(carried).ok(),
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
