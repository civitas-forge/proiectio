use super::*;

use std::collections::BTreeSet;

use camino::Utf8PathBuf;
use libproiectio::{BlockFault, Refusal, RefusalKind};
use serde::Serialize;
use serde_json::json;

fn planned(rows: JsonValue) -> RunLines {
    lines(&json!({ "rows": rows }), AmbiguousWidth::Narrow)
}

fn applied(rows: JsonValue) -> RunLines {
    lines(
        &json!({ "report": { "rows": rows } }),
        AmbiguousWidth::Narrow,
    )
}

fn file(verdict: JsonValue) -> JsonValue {
    json!({ "facts": { "shape": { "File": { "executable": false } } }, "verdict": verdict })
}

fn link(verdict: JsonValue, target: &str) -> JsonValue {
    json!({ "facts": { "shape": { "Symlink": { "target": target } } }, "verdict": verdict })
}

/// The one row a plan refusing a single path prints.
fn refused(refusal: &JsonValue) -> RunLines {
    planned(json!({
        "one": { "facts": null, "verdict": { "Refuse": { "refusal": refusal } } },
    }))
}

/// A library value as the context provider hands it over.
fn serialized(value: impl Serialize) -> JsonValue {
    serde_json::to_value(value).expect("a serializable library value")
}

fn only(document: RunLines) -> RowView {
    let mut rows = document.rows;
    assert_eq!(rows.len(), 1, "one row");
    rows.remove(0)
}

/// Every verdict a plan and a real run report reads as one style and one verb.
#[test]
fn each_verdict_spells_one_style_and_one_verb() {
    for (verdict, style, verb) in [
        (json!("Write"), "wrote", "would write"),
        (json!("Written"), "wrote", "wrote"),
        (
            json!({ "Overwrite": { "reason": "ContentChanged" } }),
            "overwrote",
            "would overwrite",
        ),
        (json!("Overwritten"), "overwrote", "overwrote"),
        (json!("Skip"), "skipped", "would skip"),
        (json!("Skipped"), "skipped", "skipped"),
        (json!("Remove"), "removed", "would remove"),
        (json!("Removed"), "removed", "removed"),
        (json!("Release"), "removed", "would release"),
        (json!("Released"), "removed", "released"),
        (
            json!({ "Refuse": { "refusal": "Drift" } }),
            "refused",
            "would refuse",
        ),
    ] {
        let row = only(planned(json!({ "one": file(verdict.clone()) })));
        assert_eq!((row.style, row.verb.as_str()), (style, verb), "{verdict}");
    }
}

/// A symlink this run writes reads as a link; every other verdict keeps the
/// style its own name earned.
#[test]
fn a_symlink_reads_as_a_link_only_where_the_run_writes_it() {
    for (verdict, style, verb) in [
        (json!("Write"), "linked", "would link"),
        (json!("Written"), "linked", "linked"),
        (json!("Skipped"), "skipped", "skipped"),
        (json!("Removed"), "removed", "removed"),
    ] {
        let row = only(planned(json!({ "one": link(verdict.clone(), "x") })));
        assert_eq!((row.style, row.verb.as_str()), (style, verb), "{verdict}");
    }
}

/// A verdict this CLI does not know renders its own name, unstyled by any
/// family the stylesheet spells.
#[test]
fn an_unknown_verdict_renders_its_own_name() {
    let row = only(planned(json!({ "one": file(json!("Ponder")) })));

    assert_eq!((row.style, row.verb.as_str()), ("unknown", "Ponder"));
}

/// A refused row names the refusal it carries, in the vocabulary the exit
/// table names the kinds with; one this CLI does not know reads as the name
/// the library spelled, escaped.
#[test]
fn a_refused_row_names_the_refusal() {
    for (refusal, note) in [
        (json!("Drift"), "(drifted)"),
        (json!("Foreign"), "(foreign)"),
        (json!("Containment"), "(containment)"),
        (json!("[wrote]"), "(\\[wrote\\])"),
    ] {
        let row = only(refused(&refusal));

        assert_eq!(row.note.as_deref(), Some(note), "{refusal}");
    }
}

/// Every kind the library declares reads as one spelling: the arms are matched
/// over `RefusalKind` itself, so a kind added there stops this compiling until
/// the view spells it, and each kind is fed in as the library serializes it, so
/// a renamed one fails here rather than falling through to the unknown arm.
#[test]
fn every_refusal_kind_the_library_declares_reads_as_one_spelling() {
    for kind in [
        RefusalKind::Containment,
        RefusalKind::TreeConflict,
        RefusalKind::Foreign,
        RefusalKind::Drift,
        RefusalKind::OwnerConflict,
        RefusalKind::ExternalTarget,
        RefusalKind::InvalidTarget,
        RefusalKind::Block,
    ] {
        let spelled = match kind {
            RefusalKind::Containment => "containment",
            RefusalKind::TreeConflict => "tree conflict",
            RefusalKind::Foreign => "foreign",
            RefusalKind::Drift => "drifted",
            RefusalKind::OwnerConflict => "owner conflict",
            RefusalKind::ExternalTarget => "external target",
            RefusalKind::InvalidTarget => "invalid target",
            RefusalKind::Block => "block",
        };
        let row = only(refused(&serialized(kind)));

        assert_eq!(row.note.as_deref(), Some(format!("({spelled})").as_str()));
    }
}

/// A refusal carrying a payload renders it, in the words the library's own
/// message renders it with, every payload string escaped like any other value
/// a run did not write.
#[test]
fn a_refused_row_renders_the_payload_its_refusal_carries() {
    for (refusal, note) in [
        (
            Refusal::TreeConflict {
                paths: BTreeSet::from([Utf8PathBuf::from("other"), Utf8PathBuf::from("third")]),
            },
            "(tree conflict) (with other, third)",
        ),
        (
            Refusal::OwnerConflict {
                owners: BTreeSet::from(["base".to_owned(), "site".to_owned()]),
            },
            "(owner conflict) (held by base+site)",
        ),
        (
            Refusal::ExternalTarget {
                target: "/opt/x".to_owned(),
            },
            "(external target) -> /opt/x",
        ),
        (
            Refusal::ExternalTarget {
                target: "[x]".to_owned(),
            },
            "(external target) -> \\[x\\]",
        ),
        (
            Refusal::InvalidTarget {
                target: String::new(),
            },
            "(invalid target) -> \"\"",
        ),
        (
            Refusal::Block {
                fault: BlockFault::MarkerEmpty,
            },
            "(block) (MarkerEmpty)",
        ),
    ] {
        let row = only(refused(&serialized(&refusal)));

        assert_eq!(row.note.as_deref(), Some(note), "{refusal:?}");
    }
}

/// A name spelled like markup reaches the terminal as the characters it is.
#[test]
fn an_unknown_verdict_spelled_like_a_tag_is_escaped() {
    let row = only(planned(json!({ "one": file(json!("[wrote]")) })));

    assert_eq!(row.verb, "\\[wrote\\]");
}

/// A link names its target, an overwrite says why, and a file left with
/// neither says only that it is executable.
#[test]
fn a_note_states_the_target_the_reason_or_the_executable_bit() {
    let target = only(planned(
        json!({ "one": link(json!("Write"), "releases/1") }),
    ));
    assert_eq!(target.note.as_deref(), Some("-> releases/1"));

    let reason = only(planned(json!({
        "one": file(json!({ "Overwrite": { "reason": "ExecutableChanged" } })),
    })));
    assert_eq!(reason.note.as_deref(), Some("(executable changed)"));

    let forced = only(planned(json!({
        "one": file(json!({ "Overwrite": { "reason": "ForcedDrift" } })),
    })));
    assert_eq!(forced.note.as_deref(), Some("(drifted, forced)"));

    let executable = only(planned(json!({
        "one": json!({
            "facts": { "shape": { "File": { "executable": true } } },
            "verdict": "Written",
        }),
    })));
    assert_eq!(executable.note.as_deref(), Some("(exec)"));

    let plain = only(planned(json!({ "one": file(json!("Written")) })));
    assert_eq!(plain.note, None);
}

/// A target this run would overwrite carries both, and a link target spelled
/// like markup is escaped like any other value.
#[test]
fn a_drifted_link_states_its_target_and_why_it_would_be_overwritten() {
    let row = only(planned(json!({
        "one": link(json!({ "Overwrite": { "reason": "ContentChanged" } }), "[x]"),
    })));

    assert_eq!(row.note.as_deref(), Some("-> \\[x\\]  (content changed)"));
}

/// The path column is measured in terminal columns, so an escaped bracket and
/// a wide character leave every note at the same offset.
#[test]
fn the_path_column_pads_to_the_widest_path_in_display_width() {
    let document = planned(json!({
        "[a]": link(json!("Write"), "x"),
        "\u{65e5}\u{672c}\u{8a9e}": link(json!("Write"), "y"),
    }));

    let widths: Vec<(String, usize)> = document
        .rows
        .iter()
        .map(|row| (row.path.clone(), row.path_pad.len()))
        .collect();
    assert_eq!(
        widths,
        vec![
            ("\\[a\\]".to_owned(), 3),
            ("\u{65e5}\u{672c}\u{8a9e}".to_owned(), 0),
        ]
    );
}

/// The verb column is the widest verb the run can spell: a plan's, then a
/// real run's.
#[test]
fn the_verb_column_pads_to_the_widest_verb_the_run_spells() {
    let plan = only(planned(json!({ "one": file(json!("Write")) })));
    assert_eq!(plan.verb.len() + plan.verb_pad.len(), 15);

    let real = only(applied(json!({ "one": file(json!("Written")) })));
    assert_eq!(real.verb.len() + real.verb_pad.len(), 9);
}

/// A plan states no count; a real run counts what it did. A pass that
/// projected leads with the written/skipped pair, one that only cleared paths
/// counts what it cleared, and one that touched nothing says so.
#[test]
fn a_real_run_counts_what_it_did_and_a_plan_counts_nothing() {
    assert_eq!(
        planned(json!({ "one": file(json!("Write")) })).summary,
        None
    );

    for (verdicts, summary) in [
        (
            vec!["Written", "Overwritten", "Skipped"],
            "2 written, 1 skipped",
        ),
        (vec!["Skipped", "Skipped"], "2 unchanged"),
        (
            vec!["Written", "Removed"],
            "1 written, 0 skipped, 1 removed",
        ),
        (
            vec!["Written", "Released"],
            "1 written, 0 skipped, 1 released",
        ),
        (
            vec!["Written", "Removed", "Released"],
            "1 written, 0 skipped, 1 removed, 1 released",
        ),
        (vec!["Removed"], "1 removed"),
        (
            vec!["Removed", "Removed", "Released"],
            "2 removed, 1 released",
        ),
        (vec!["Released"], "1 released"),
        (
            vec!["Skipped", "Removed"],
            "0 written, 1 skipped, 1 removed",
        ),
        (vec![], "nothing to do"),
    ] {
        let rows: serde_json::Map<String, JsonValue> = verdicts
            .iter()
            .enumerate()
            .map(|(index, verdict)| (index.to_string(), file(json!(verdict))))
            .collect();
        assert_eq!(
            applied(JsonValue::Object(rows)).summary.as_deref(),
            Some(summary),
            "{verdicts:?}"
        );
    }
}

/// A document naming no rows prints nothing: the injected context is resolved
/// for every command, and only `write` and `rm` read it.
#[test]
fn a_document_that_names_no_rows_prints_nothing() {
    assert_eq!(
        lines(&json!({ "kind": "listing" }), AmbiguousWidth::Narrow),
        RunLines::default()
    );
}
