use super::*;

use std::collections::BTreeSet;

use camino::Utf8PathBuf;
use libproiectio::{
    ApplyOutcome, BlockFault, Dropped, EntryKind, Error, ManifestEntry, Origin, OverwriteReason,
    Refusal, RefusalKind,
};
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

/// The same row, with the facts a refused row carries: the source that named
/// the path, and no shape.
fn refused_by(refusal: &JsonValue, origin: &Origin) -> RunLines {
    planned(json!({
        "one": {
            "facts": { "shape": null, "owners": [], "origin": serialized(origin) },
            "verdict": { "Refuse": { "refusal": refusal } },
        },
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
        (json!("Forget"), "removed", "would forget"),
        (json!("Forgot"), "removed", "forgot"),
        (json!("Release"), "removed", "would release"),
        (json!("Released"), "removed", "released"),
        (json!("NotRecorded"), "skipped", "no record"),
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

/// The verdicts above are spelled from string literals, which agree with the
/// library only as long as nobody renames a variant. This drives the same
/// mapping from the enums themselves: the `match` fails to compile when a
/// verdict is added, and the assertion fails when one is renamed, since a
/// name `spelling` does not know renders as itself.
#[test]
fn every_verdict_the_library_declares_reads_as_one_spelling() {
    for verdict in [
        PlannedAction::Write,
        PlannedAction::Overwrite {
            reason: OverwriteReason::ContentChanged,
        },
        PlannedAction::Skip,
        PlannedAction::Remove,
        PlannedAction::Forget,
        PlannedAction::Release,
        PlannedAction::NotRecorded,
        PlannedAction::Refuse {
            refusal: Refusal::Drift,
        },
    ] {
        let spelled = match &verdict {
            PlannedAction::Write => ("wrote", "would write"),
            PlannedAction::Overwrite { .. } => ("overwrote", "would overwrite"),
            PlannedAction::Skip => ("skipped", "would skip"),
            PlannedAction::Remove => ("removed", "would remove"),
            PlannedAction::Forget => ("removed", "would forget"),
            PlannedAction::Release => ("removed", "would release"),
            PlannedAction::NotRecorded => ("skipped", "no record"),
            PlannedAction::Refuse { .. } => ("refused", "would refuse"),
        };
        let row = only(planned(json!({ "one": file(serialized(&verdict)) })));

        assert_eq!((row.style, row.verb.as_str()), spelled, "{verdict:?}");
    }
    for verdict in [
        ApplyOutcome::Written,
        ApplyOutcome::Overwritten,
        ApplyOutcome::Skipped,
        ApplyOutcome::Removed,
        ApplyOutcome::Forgot,
        ApplyOutcome::Released,
        ApplyOutcome::NotRecorded,
    ] {
        let spelled = match verdict {
            ApplyOutcome::Written => ("wrote", "wrote"),
            ApplyOutcome::Overwritten => ("overwrote", "overwrote"),
            ApplyOutcome::Skipped => ("skipped", "skipped"),
            ApplyOutcome::Removed => ("removed", "removed"),
            ApplyOutcome::Forgot => ("removed", "forgot"),
            ApplyOutcome::Released => ("removed", "released"),
            ApplyOutcome::NotRecorded => ("skipped", "no record"),
        };
        let document = applied(json!({ "one": file(serialized(verdict)) }));
        let row = only(RunLines {
            rows: document.rows,
            ..RunLines::default()
        });

        assert_eq!((row.style, row.verb.as_str()), spelled, "{verdict:?}");
        // The second stringly mapping: a verdict `counted` does not know
        // falls out of the tally, and a run of one row reads as idle.
        assert_ne!(
            document.summary.as_deref(),
            Some("nothing to do"),
            "{verdict:?} is counted"
        );
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
            "(block) (the marker is empty)",
        ),
    ] {
        let row = only(refused(&serialized(&refusal)));

        assert_eq!(row.note.as_deref(), Some(note), "{refusal:?}");
    }
}

/// Every fault the library declares reads as the sentence its own message
/// spells rather than the name it serializes under; the arms are matched over
/// `BlockFault` itself, so a fault added there stops this compiling until this
/// list carries it, and each fault is fed in as the library serializes it, so
/// a renamed one fails here rather than reaching the view's unknown arm.
#[test]
fn every_block_fault_reads_as_the_message_the_library_spells() {
    for fault in [
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
    ] {
        let spelled = match fault {
            BlockFault::MarkerEmpty
            | BlockFault::MarkerNotOneLine
            | BlockFault::MarkerEdgeWhitespace
            | BlockFault::BodyCarriesMarker
            | BlockFault::BodyNotNewlineTerminated
            | BlockFault::ContainerNotNewlineTerminated
            | BlockFault::ContainerMissing
            | BlockFault::KindChange
            | BlockFault::SignatureNotRecorded
            | BlockFault::MarkerInAuthorText => fault.to_string(),
        };
        let row = only(refused(&serialized(Refusal::Block { fault })));

        assert_eq!(
            row.note.as_deref(),
            Some(format!("(block) ({spelled})").as_str()),
            "{fault:?}"
        );
    }
}

/// A fault this CLI does not know reads as the name the library spelled,
/// escaped.
#[test]
fn an_unknown_block_fault_reads_as_its_own_name() {
    let row = only(refused(&json!({ "Block": { "fault": "[Pondered]" } })));

    assert_eq!(row.note.as_deref(), Some("(block) (\\[Pondered\\])"));
}

/// A refused row names the source that named the path, in the phrase the
/// library's own refusal message names it with: the arms are matched over
/// `Origin` itself, so a source added there stops this compiling, and the
/// phrase is the origin's own message rather than a copy of it. A path the
/// caller named itself states only its refusal.
#[test]
fn a_refused_row_names_the_source_that_named_the_path() {
    for origin in [
        Origin::Caller,
        Origin::Mapping {
            path: Utf8PathBuf::from("/etc/deploy.toml"),
        },
        Origin::Tree {
            path: Utf8PathBuf::from("/srv/skeleton"),
        },
        Origin::Archive {
            path: Utf8PathBuf::from("/srv/app.tgz"),
            via: None,
        },
        Origin::Archive {
            path: Utf8PathBuf::from("/srv/app.tgz"),
            via: Some(Utf8PathBuf::from("/etc/deploy.toml")),
        },
        Origin::Files,
    ] {
        let phrase = match &origin {
            Origin::Caller => String::new(),
            named @ (Origin::Mapping { .. }
            | Origin::Tree { .. }
            | Origin::Archive { .. }
            | Origin::Files) => format!(" ({named})"),
        };
        let row = only(refused_by(&json!("Drift"), &origin));

        assert_eq!(
            row.note.as_deref(),
            Some(format!("(drifted){phrase}").as_str()),
            "{origin:?}"
        );
    }
}

/// A source path spelled like markup reaches the terminal as the characters
/// it is.
#[test]
fn a_refused_rows_source_path_is_escaped() {
    let row = only(refused_by(
        &json!("Drift"),
        &Origin::Tree {
            path: Utf8PathBuf::from("/srv/[x]"),
        },
    ));

    assert_eq!(
        row.note.as_deref(),
        Some("(drifted) (from tree /srv/\\[x\\])")
    );
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
        (vec!["Removed", "Forgot"], "1 removed, 1 forgotten"),
        (vec!["Forgot"], "1 forgotten"),
        // A pass that did nothing at every path it was handed counts the
        // paths rather than reading `nothing to do`, which is what an owner
        // removing a path it never recorded would otherwise be told.
        (vec!["NotRecorded"], "1 not recorded"),
        (vec!["Removed", "NotRecorded"], "1 removed, 1 not recorded"),
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

/// A member `strip` left no path prints its own row naming the archive that
/// carried it and the strip count that erased it. The path column is the
/// member as the archive spells it, not a location in the destination.
///
/// A plan flattens its rows beside `dropped` and an apply nests them under
/// `report`, so the drop reads from the top level in both tenses.
#[test]
fn a_dropped_member_prints_a_row_naming_the_archive() {
    let dropped = json!([serialized(Dropped {
        member: Utf8PathBuf::from("._pkg"),
        prefix: Utf8PathBuf::new(),
        strip: 1,
        origin: Origin::Archive {
            path: Utf8PathBuf::from("/assets/vendor.tar.gz"),
            via: None,
        },
    })]);
    for document in [
        json!({ "rows": {}, "dropped": dropped }),
        json!({ "report": { "rows": {} }, "dropped": dropped }),
    ] {
        let row = only(lines(&document, AmbiguousWidth::Narrow));
        assert_eq!(row.style, "skipped");
        assert_eq!(row.verb, "dropped");
        assert_eq!(row.path, "._pkg");
        assert_eq!(
            row.note.as_deref(),
            Some("(no path left after strip 1) (from archive /assets/vendor.tar.gz)")
        );
    }
}

/// Two archives dropping the same member name print two rows, each naming
/// the archive that carried it: a member name is unique only inside its own
/// archive, so one drop cannot stand for the other.
#[test]
fn two_archives_dropping_the_same_member_print_both_rows() {
    let carried_by = |archive: &str| {
        serialized(Dropped {
            member: Utf8PathBuf::from("._pkg"),
            prefix: Utf8PathBuf::new(),
            strip: 1,
            origin: Origin::Archive {
                path: Utf8PathBuf::from(archive),
                via: None,
            },
        })
    };
    let document = json!({
        "rows": {},
        "dropped": [carried_by("/assets/plugins.tar.gz"), carried_by("/assets/vendor.tar.gz")],
    });

    let rows = lines(&document, AmbiguousWidth::Narrow).rows;
    assert_eq!(
        rows.iter()
            .map(|row| (row.path.as_str(), row.note.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            (
                "._pkg",
                Some("(no path left after strip 1) (from archive /assets/plugins.tar.gz)")
            ),
            (
                "._pkg",
                Some("(no path left after strip 1) (from archive /assets/vendor.tar.gz)")
            ),
        ]
    );
}

/// One archive named by two mapping entries drops the same member twice, and
/// the two rows differ only in what the entries asked for: where the archive
/// was bound for, and how much of each member name it stripped.
#[test]
fn one_archive_under_two_prefixes_prints_a_row_per_entry() {
    let asked_by = |prefix: &str, strip: u32| {
        serialized(Dropped {
            member: Utf8PathBuf::from("._pkg"),
            prefix: Utf8PathBuf::from(prefix),
            strip,
            origin: Origin::Archive {
                path: Utf8PathBuf::from("/assets/vendor.tar.gz"),
                via: Some(Utf8PathBuf::from("/srv/deploy.toml")),
            },
        })
    };
    let document = json!({
        "rows": {},
        "dropped": [asked_by("backup", 2), asked_by("vendor", 1)],
    });

    let rows = lines(&document, AmbiguousWidth::Narrow).rows;
    assert_eq!(
        rows.iter()
            .map(|row| row.note.as_deref())
            .collect::<Vec<_>>(),
        vec![
            Some(
                "(no path left after strip 2) (from archive /assets/vendor.tar.gz \
                 into backup, named by mapping /srv/deploy.toml)"
            ),
            Some(
                "(no path left after strip 1) (from archive /assets/vendor.tar.gz \
                 into vendor, named by mapping /srv/deploy.toml)"
            ),
        ]
    );
}

/// A dropped member takes the same path column as the rows beside it, so the
/// notes line up.
#[test]
fn dropped_members_share_the_path_column_with_the_rows() {
    let document = json!({
        "rows": { "a/very/long/path": file(json!("Write")) },
        "dropped": [serialized(Dropped {
            member: Utf8PathBuf::from("._pkg"),
            prefix: Utf8PathBuf::new(),
            strip: 1,
            origin: Origin::Archive {
                path: Utf8PathBuf::from("/assets/vendor.tar.gz"),
                via: Some(Utf8PathBuf::from("/srv/deploy.toml")),
            },
        })],
    });
    let rows = lines(&document, AmbiguousWidth::Narrow).rows;
    assert_eq!(rows.len(), 2);
    let width = |row: &RowView| row.path.len() + row.path_pad.len();
    assert_eq!(width(&rows[0]), width(&rows[1]));
    assert_eq!(
        rows[1].note.as_deref(),
        Some(
            "(no path left after strip 1) (from archive /assets/vendor.tar.gz, \
             named by mapping /srv/deploy.toml)"
        )
    );
}

/// The document a refusal met past the plan renders: one row per refused key,
/// carrying the refusal and the source the error names, in the same shape a
/// plan's own refused row has — a null shape, and a verdict under `Refuse`.
#[test]
fn a_refusal_renders_the_row_shape_a_refused_plan_renders() {
    let origin = Origin::Mapping {
        path: Utf8PathBuf::from("/srv/deploy.toml"),
    };
    let refused = Refused::one(
        Utf8PathBuf::from("bin/tool"),
        Refusal::Drift,
        origin.clone(),
    );

    let document = serialized(PlannedRun::refused(&refused, &Manifest::new()));

    assert_eq!(
        document,
        json!({
            "rows": {
                "bin/tool": {
                    "facts": { "shape": null, "owners": [], "origin": serialized(&origin) },
                    "verdict": { "Refuse": { "refusal": "Drift" } },
                },
            },
        })
    );
    let row = only(lines(&document, AmbiguousWidth::Narrow));
    assert_eq!((row.style, row.verb.as_str()), ("refused", "would refuse"));
    assert_eq!(row.path, "bin/tool");
    assert_eq!(
        row.note.as_deref(),
        Some("(drifted) (from mapping /srv/deploy.toml)")
    );
}

/// Every refused key the error names gets a row, each stating the source that
/// named it, and a refusal strips no archive so the document carries no
/// `dropped`.
#[test]
fn a_refusal_of_several_keys_renders_a_row_for_each() {
    let held_by = |owner: &str| Refusal::OwnerConflict {
        owners: BTreeSet::from([owner.to_owned()]),
    };
    let aggregated = Refused::aggregate([
        (
            Utf8PathBuf::from("bin/tool"),
            held_by("site"),
            Origin::Files,
        ),
        (
            Utf8PathBuf::from("config/settings.toml"),
            held_by("other"),
            Origin::Caller,
        ),
    ])
    .expect("a refusal over two keys");

    let document = serialized(PlannedRun::refused(&aggregated, &Manifest::new()));

    assert_eq!(document.get("dropped"), None);
    let rows = lines(&document, AmbiguousWidth::Narrow).rows;
    assert_eq!(
        rows.iter()
            .map(|row| (row.path.as_str(), row.verb.as_str(), row.note.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            (
                "bin/tool",
                "would refuse",
                Some("(owner conflict) (held by site) (from individually named files)")
            ),
            (
                "config/settings.toml",
                "would refuse",
                Some("(owner conflict) (held by other)")
            ),
        ]
    );
}

/// A refused row states the owners the manifest records at the path, which is
/// what a plan's own refused row states: a caller reading the two documents
/// reads one shape, whichever stage refused.
#[test]
fn a_refused_row_states_the_owners_the_manifest_records() {
    let refused = Refused::one(
        Utf8PathBuf::from("bin/tool"),
        Refusal::Drift,
        Origin::Caller,
    );

    let document = serialized(PlannedRun::refused(
        &refused,
        &holding("bin/tool", &["site"]),
    ));

    assert_eq!(
        document["rows"]["bin/tool"]["facts"]["owners"],
        json!(["site"])
    );
}

/// A run that stopped part-way renders both halves at once, in one table laid
/// out in path order: a refused key sorting before an applied one prints
/// before it, as it would in any other report. Each row reads in the tense of
/// what happened to it, and the summary says the run stopped.
#[test]
fn a_run_that_stopped_part_way_states_what_it_applied_and_what_it_refused() {
    let refused = Refused::one(
        Utf8PathBuf::from("bin/tool"),
        Refusal::Drift,
        Origin::Caller,
    );

    let document = serialized(AbortedRun::new(
        wrote(&["config/settings.toml"]),
        refused_rows(&refused, &Manifest::new()),
        &Stopped::Applying(Error::Refused(refused.clone())),
    ));

    assert_eq!(document["aborted"], json!(true));
    assert_eq!(document["recorded"], json!(true));
    assert_eq!(document.get("stopped"), None);
    let rendered = lines(&document, AmbiguousWidth::Narrow);
    assert_eq!(
        rendered
            .rows
            .iter()
            .map(|row| (row.path.as_str(), row.verb.as_str(), row.style))
            .collect::<Vec<_>>(),
        vec![
            ("bin/tool", "refused", "refused"),
            ("config/settings.toml", "wrote", "wrote"),
        ]
    );
    assert_eq!(
        rendered.summary.as_deref(),
        Some(
            "1 written, 0 skipped, 1 refused — the run stopped part-way \
             through the plan, and what it applied stands"
        )
    );
    assert!(rendered.stopped.is_empty());
}

/// A failure rather than a refusal stops a run on the same terms: the rows it
/// applied, and the diagnostic that would otherwise have replaced them, which
/// reaches the reader in the document because a rendered run leaves nothing on
/// stderr.
#[test]
fn a_run_a_failure_stopped_states_its_rows_and_the_failure() {
    let document = serialized(AbortedRun::new(
        wrote(&["bin/tool"]),
        Report::default(),
        &Stopped::Applying(held("lock")),
    ));

    assert_eq!(document["aborted"], json!(true));
    assert_eq!(document["recorded"], json!(true));
    assert_eq!(document.get("refused"), None);
    let rendered = lines(&document, AmbiguousWidth::Narrow);
    assert_eq!(rendered.rows.len(), 1);
    assert_eq!(
        rendered.stopped,
        vec!["state lock lock is held by another writer"]
    );
}

/// A run whose manifest never reached the state directory says so in the
/// document and in `recorded`: the destination holds writes nothing on disk
/// records, which is the one thing a reader of these rows must not assume
/// away.
#[test]
fn a_run_that_could_not_record_what_it_applied_says_so() {
    let document = serialized(AbortedRun::new(
        wrote(&["bin/tool"]),
        Report::default(),
        &Stopped::Recording(held("state.lock")),
    ));

    assert_eq!(document["recorded"], json!(false));
    let rendered = lines(&document, AmbiguousWidth::Narrow);
    assert_eq!(
        rendered.stopped,
        vec![
            "state lock state.lock is held by another writer",
            "the state directory does not record what the run applied: \
             state lock state.lock is held by another writer",
        ]
    );
}

/// A run that lost both halves states both: the keys it refused as rows, and
/// the record it could not write as a line. The refusal states itself in the
/// rows, so only the record is spelled out.
#[test]
fn a_run_that_refused_and_could_not_record_states_the_rows_and_the_record() {
    let refused = Refused::one(
        Utf8PathBuf::from("bin/tool"),
        Refusal::Drift,
        Origin::Caller,
    );

    let document = serialized(AbortedRun::new(
        wrote(&["config/settings.toml"]),
        refused_rows(&refused, &Manifest::new()),
        &Stopped::ApplyingAndRecording {
            applying: Error::Refused(refused.clone()),
            recording: held("state.lock"),
        },
    ));

    assert_eq!(document["recorded"], json!(false));
    let rendered = lines(&document, AmbiguousWidth::Narrow);
    assert_eq!(rendered.rows.len(), 2);
    assert_eq!(
        rendered.stopped,
        vec![
            "the state directory does not record what the run applied: \
             state lock state.lock is held by another writer"
        ]
    );
}

/// An `ApplyReport` writing the paths named.
fn wrote(paths: &[&str]) -> ApplyReport {
    ApplyReport {
        report: Report {
            rows: paths
                .iter()
                .map(|path| {
                    (
                        Utf8PathBuf::from(*path),
                        Row {
                            facts: None,
                            verdict: ApplyOutcome::Written,
                        },
                    )
                })
                .collect(),
        },
        dropped: BTreeSet::new(),
        manifest: Manifest::new(),
    }
}

/// A failure that is not a refusal, and carries no `io::Error` to build.
fn held(path: &str) -> Error {
    Error::LockHeld {
        path: Utf8PathBuf::from(path),
    }
}

/// A manifest recording one path under the owners named.
fn holding(path: &str, owners: &[&str]) -> Manifest {
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        Utf8PathBuf::from(path),
        ManifestEntry {
            kind: EntryKind::File,
            hash: String::new(),
            executable: false,
            owners: owners.iter().map(|owner| (*owner).to_owned()).collect(),
        },
    );
    manifest
}
