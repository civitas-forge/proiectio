use super::*;

use std::collections::{BTreeMap, BTreeSet};

use camino::Utf8PathBuf;
use libproiectio::{
    ApplyOutcome, BlockFault, Dropped, EntryKind, Error, ManifestEntry, Origin, OverwriteReason,
    Refusal, RefusalKind, Refused,
};
use serde::Serialize;
use serde_json::json;

/// The `rows` sequence a report serializes, from the rows a case states by
/// path: each record carries its path as a field beside what the row states.
fn records(rows: JsonValue) -> JsonValue {
    JsonValue::Array(
        rows.as_object()
            .expect("rows stated by path")
            .iter()
            .map(|(path, row)| {
                let mut record = row.as_object().expect("a row").clone();
                record.insert("path".to_owned(), json!(path));
                JsonValue::Object(record)
            })
            .collect(),
    )
}

fn planned(rows: JsonValue) -> RunLines {
    lines(
        &json!({ "phase": "planned", "rows": records(rows) }),
        AmbiguousWidth::Narrow,
        false,
    )
}

fn applied(rows: JsonValue) -> RunLines {
    lines(
        &json!({ "phase": "applied", "rows": records(rows) }),
        AmbiguousWidth::Narrow,
        false,
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

/// The document a plan publishes, as the renderer and a structured mode read
/// it: the arm's own fields under the phase the arm names.
fn published_plan(planned: PlannedRun) -> JsonValue {
    serialized(RunView::Planned(planned))
}

/// The same for a run that could not finish.
fn published_abort(aborted: AbortedRun) -> JsonValue {
    serialized(RunView::Aborted(Box::new(aborted)))
}

fn only(document: RunLines) -> RowView {
    let mut rows = document.rows;
    assert_eq!(rows.len(), 1, "one row");
    rows.remove(0)
}

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

/// Drives the spelling from the enums themselves: the `match` fails to
/// compile when a verdict is added, and the assertion fails when one is
/// renamed, since a name `spelling` does not know renders as itself.
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
        // The verb column is a constant, and the verbs it has to hold come
        // from these enums; one spelled wider than the constant would leave no
        // pad and push its path out of line.
        assert_eq!(
            row.verb.len() + row.verb_pad.len(),
            PLANNED_VERBS,
            "{verdict:?} fits the verb column"
        );
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
        assert_eq!(
            row.verb.len() + row.verb_pad.len(),
            APPLIED_VERBS,
            "{verdict:?} fits the verb column"
        );
        // The second stringly mapping: a verdict `counted` does not know
        // falls out of the tally, and a run of one row reads as idle.
        assert_ne!(
            document.summary.as_deref(),
            Some("nothing to do"),
            "{verdict:?} is counted"
        );
    }
}

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

#[test]
fn an_unknown_verdict_renders_its_own_name() {
    let row = only(planned(json!({ "one": file(json!("Ponder")) })));

    assert_eq!((row.style, row.verb.as_str()), ("unknown", "Ponder"));
}

/// The view's clauses agree with `Refusal`'s own message only as long as
/// somebody keeps them agreeing; this checks the message ends with them.
#[test]
fn a_directory_refusal_reads_the_same_from_the_view_and_from_the_library() {
    let held = |path: &str| (Utf8PathBuf::from(path), BTreeSet::new());
    for refusal in [
        Refusal::DirectoryInTheWay {
            holding: BTreeMap::from([held("build.sh/notes.md")]),
            unreadable: BTreeSet::new(),
            pruned: false,
        },
        Refusal::DirectoryInTheWay {
            holding: BTreeMap::new(),
            unreadable: BTreeSet::from([Utf8PathBuf::from("build.sh")]),
            pruned: false,
        },
        Refusal::DirectoryInTheWay {
            holding: BTreeMap::from([held("build.sh/notes.md")]),
            unreadable: BTreeSet::from([Utf8PathBuf::from("build.sh/nested")]),
            pruned: true,
        },
    ] {
        let from_library = Refused::one(
            Utf8PathBuf::from("build.sh"),
            refusal.clone(),
            Origin::Caller,
        )
        .to_string();
        let from_view = refusing(&serialized(&refusal));
        let clauses = from_view
            .strip_prefix("(directory in the way)")
            .expect("the view names the kind first")
            .trim();

        assert!(!clauses.is_empty(), "{refusal:?} renders a payload");
        assert!(
            from_library.ends_with(clauses),
            "the view writes {clauses:?}, which the library's {from_library:?} does not end with"
        );
    }
}

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

/// Matched over `RefusalKind` itself, so an added kind stops this compiling
/// and a renamed one fails here rather than falling through to unknown.
#[test]
fn every_refusal_kind_the_library_declares_reads_as_one_spelling() {
    for kind in [
        RefusalKind::Containment,
        RefusalKind::TreeConflict,
        RefusalKind::RecordedLanding,
        RefusalKind::Foreign,
        RefusalKind::Drift,
        RefusalKind::DirectoryInTheWay,
        RefusalKind::OwnerConflict,
        RefusalKind::ExternalTarget,
        RefusalKind::InvalidTarget,
        RefusalKind::Block,
    ] {
        let spelled = match kind {
            RefusalKind::Containment => "containment",
            RefusalKind::TreeConflict => "tree conflict",
            RefusalKind::RecordedLanding => "recorded landing",
            RefusalKind::Foreign => "foreign",
            RefusalKind::Drift => "drifted",
            RefusalKind::DirectoryInTheWay => "directory in the way",
            RefusalKind::OwnerConflict => "owner conflict",
            RefusalKind::ExternalTarget => "external target",
            RefusalKind::InvalidTarget => "invalid target",
            RefusalKind::Block => "block",
        };
        let row = only(refused(&serialized(kind)));

        assert_eq!(row.note.as_deref(), Some(format!("({spelled})").as_str()));
    }
}

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
            Refusal::RecordedLanding {
                through: Utf8PathBuf::from("a"),
                at: Utf8PathBuf::from("real/x.txt"),
                owners: BTreeSet::from(["base".to_owned(), "site".to_owned()]),
            },
            "(recorded landing) (through the symlink a, onto real/x.txt, held by base+site)",
        ),
        (
            Refusal::DirectoryInTheWay {
                holding: BTreeMap::from([
                    (Utf8PathBuf::from("build.sh/notes.md"), BTreeSet::new()),
                    (
                        Utf8PathBuf::from("build.sh/theirs"),
                        BTreeSet::from(["base".to_owned(), "site".to_owned()]),
                    ),
                ]),
                unreadable: BTreeSet::new(),
                pruned: false,
            },
            "(directory in the way) (holding build.sh/notes.md, \
             build.sh/theirs (held by base+site), which --force does not remove)",
        ),
        (
            Refusal::DirectoryInTheWay {
                holding: BTreeMap::new(),
                unreadable: BTreeSet::new(),
                pruned: false,
            },
            "(directory in the way)",
        ),
        (
            Refusal::DirectoryInTheWay {
                holding: BTreeMap::new(),
                unreadable: BTreeSet::from([Utf8PathBuf::from("build.sh/nested")]),
                pruned: false,
            },
            "(directory in the way) (holding names that are not UTF-8 in build.sh/nested)",
        ),
        (
            Refusal::DirectoryInTheWay {
                holding: BTreeMap::from([(
                    Utf8PathBuf::from("build.sh/notes.md"),
                    BTreeSet::new(),
                )]),
                unreadable: BTreeSet::from([Utf8PathBuf::from("build.sh")]),
                pruned: false,
            },
            "(directory in the way) (holding build.sh/notes.md, which --force does not remove, \
             and holding names that are not UTF-8 in build.sh)",
        ),
        (
            Refusal::DirectoryInTheWay {
                holding: BTreeMap::new(),
                unreadable: BTreeSet::new(),
                pruned: true,
            },
            "(directory in the way) (holding pruned contents)",
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

/// Matched over `BlockFault` itself, so an added fault stops this compiling
/// and a renamed one fails here rather than reaching the unknown arm.
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

#[test]
fn an_unknown_block_fault_reads_as_its_own_name() {
    let row = only(refused(&json!({ "Block": { "fault": "[Pondered]" } })));

    assert_eq!(row.note.as_deref(), Some("(block) (\\[Pondered\\])"));
}

/// Matched over `Origin` itself, so an added source stops this compiling;
/// the phrase is the origin's own message rather than a copy of it.
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

#[test]
fn an_unknown_verdict_spelled_like_a_tag_is_escaped() {
    let row = only(planned(json!({ "one": file(json!("[wrote]")) })));

    assert_eq!(row.verb, "\\[wrote\\]");
}

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

#[test]
fn a_drifted_link_states_its_target_and_why_it_would_be_overwritten() {
    let row = only(planned(json!({
        "one": link(json!({ "Overwrite": { "reason": "ContentChanged" } }), "[x]"),
    })));

    assert_eq!(row.note.as_deref(), Some("-> \\[x\\]  (content changed)"));
}

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

#[test]
fn the_verb_column_pads_to_the_widest_verb_the_run_spells() {
    let plan = only(planned(json!({ "one": file(json!("Write")) })));
    assert_eq!(plan.verb.len() + plan.verb_pad.len(), 15);

    let real = only(applied(json!({ "one": file(json!("Written")) })));
    assert_eq!(real.verb.len() + real.verb_pad.len(), 9);
}

#[test]
fn a_verb_longer_than_the_column_widens_it_for_every_row() {
    let document = applied(json!({
        "long.txt": file(json!("SomethingUnheardOf")),
        "short.txt": file(json!("Written")),
    }));

    let widths: Vec<usize> = document
        .rows
        .iter()
        .map(|row| row.verb.len() + row.verb_pad.len())
        .collect();
    assert_eq!(widths, vec!["SomethingUnheardOf".len(); 2]);
}

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

#[test]
fn a_document_that_names_no_rows_prints_nothing() {
    assert_eq!(
        lines(&json!({ "kind": "listing" }), AmbiguousWidth::Narrow, false),
        RunLines::default()
    );
}

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
        json!({ "phase": "planned", "rows": [], "dropped": dropped }),
        json!({ "phase": "applied", "rows": [], "dropped": dropped }),
    ] {
        let row = only(lines(&document, AmbiguousWidth::Narrow, false));
        assert_eq!(row.style, "skipped");
        assert_eq!(row.verb, "dropped");
        assert_eq!(row.path, "._pkg");
        assert_eq!(
            row.note.as_deref(),
            Some("(no path left after strip 1) (from archive /assets/vendor.tar.gz)")
        );
    }
}

/// A member name is unique only inside its own archive, so one drop cannot
/// stand for the other.
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
        "phase": "planned",
        "rows": [],
        "dropped": [carried_by("/assets/plugins.tar.gz"), carried_by("/assets/vendor.tar.gz")],
    });

    let rows = lines(&document, AmbiguousWidth::Narrow, false).rows;
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
        "phase": "planned",
        "rows": [],
        "dropped": [asked_by("backup", 2), asked_by("vendor", 1)],
    });

    let rows = lines(&document, AmbiguousWidth::Narrow, false).rows;
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

#[test]
fn dropped_members_share_the_path_column_with_the_rows() {
    let document = json!({
        "phase": "planned",
        "rows": records(json!({ "a/very/long/path": file(json!("Write")) })),
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
    let rows = lines(&document, AmbiguousWidth::Narrow, false).rows;
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

    let document = published_plan(PlannedRun::refused(
        &refused,
        &Manifest::new(),
        BTreeSet::new(),
    ));

    assert_eq!(
        document,
        json!({
            "phase": "planned",
            "rows": [{
                "path": "bin/tool",
                "verdict": { "Refuse": { "refusal": "Drift" } },
                "facts": { "shape": null, "owners": [], "origin": serialized(&origin) },
            }],
        })
    );
    let row = only(lines(&document, AmbiguousWidth::Narrow, false));
    assert_eq!((row.style, row.verb.as_str()), ("refused", "would refuse"));
    assert_eq!(row.path, "bin/tool");
    assert_eq!(
        row.note.as_deref(),
        Some("(drifted) (from mapping /srv/deploy.toml)")
    );
}

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

    let document = published_plan(PlannedRun::refused(
        &aggregated,
        &Manifest::new(),
        BTreeSet::new(),
    ));

    assert_eq!(document.get("dropped"), None);
    let rows = lines(&document, AmbiguousWidth::Narrow, false).rows;
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

#[test]
fn a_refused_row_states_the_owners_the_manifest_records() {
    let refused = Refused::one(
        Utf8PathBuf::from("bin/tool"),
        Refusal::Drift,
        Origin::Caller,
    );

    let document = published_plan(PlannedRun::refused(
        &refused,
        &holding("bin/tool", &["site"]),
        BTreeSet::new(),
    ));

    assert_eq!(document["rows"][0]["path"], json!("bin/tool"));
    assert_eq!(document["rows"][0]["facts"]["owners"], json!(["site"]));
}

#[test]
fn a_run_that_stopped_part_way_states_what_it_applied_and_what_it_refused() {
    let refused = Refused::one(
        Utf8PathBuf::from("bin/tool"),
        Refusal::Drift,
        Origin::Caller,
    );

    let document = published_abort(AbortedRun::new(
        wrote(&["config/settings.toml"]),
        refused_rows(&refused, &Manifest::new()),
        &Stopped::Applying(Error::Refused(refused.clone())),
    ));

    assert_eq!(document["phase"], json!("aborted"));
    assert_eq!(document["recorded"], json!(true));
    assert_eq!(document["stopped_at"], json!("applying"));
    assert_eq!(document.get("stopped"), None);
    assert_eq!(document.get("refused"), None);
    assert_eq!(
        document["rows"]
            .as_array()
            .expect("the rows")
            .iter()
            .map(|row| (row["path"].clone(), row["verdict"].clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                json!("bin/tool"),
                json!({ "Refuse": { "refusal": "Drift" } })
            ),
            (json!("config/settings.toml"), json!("Written")),
        ]
    );
    let rendered = lines(&document, AmbiguousWidth::Narrow, false);
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

#[test]
fn a_run_a_failure_stopped_states_its_rows_and_the_failure() {
    let document = published_abort(AbortedRun::new(
        wrote(&["bin/tool"]),
        Report::default(),
        &Stopped::Applying(held("lock")),
    ));

    assert_eq!(document["phase"], json!("aborted"));
    assert_eq!(document["recorded"], json!(true));
    // Nothing was refused, so the rows are the applied ones alone.
    assert_eq!(document["rows"][0]["verdict"], json!("Written"));
    let rendered = lines(&document, AmbiguousWidth::Narrow, false);
    assert_eq!(rendered.rows.len(), 1);
    assert_eq!(
        rendered.stopped,
        vec!["state lock lock is held by another writer"]
    );
}

#[test]
fn a_run_that_could_not_record_what_it_applied_says_so() {
    let document = published_abort(AbortedRun::new(
        wrote(&["bin/tool"]),
        Report::default(),
        &Stopped::Recording(held("state.lock")),
    ));

    assert_eq!(document["recorded"], json!(false));
    assert_eq!(document["stopped_at"], json!("recording"));
    let rendered = lines(&document, AmbiguousWidth::Narrow, false);
    assert_eq!(
        rendered.stopped,
        vec![
            "the state directory does not record what the run applied: \
             state lock state.lock is held by another writer",
        ]
    );
}

#[test]
fn a_run_that_only_its_record_stopped_is_not_called_part_way() {
    let document = published_abort(AbortedRun::new(
        wrote(&["bin/tool"]),
        Report::default(),
        &Stopped::Recording(held("state.lock")),
    ));

    assert_eq!(
        lines(&document, AmbiguousWidth::Narrow, false)
            .summary
            .as_deref(),
        Some("1 written, 0 skipped — the run applied its whole plan and could not record it")
    );
}

#[test]
fn a_run_that_refused_and_could_not_record_states_the_rows_and_the_record() {
    let refused = Refused::one(
        Utf8PathBuf::from("bin/tool"),
        Refusal::Drift,
        Origin::Caller,
    );

    let document = published_abort(AbortedRun::new(
        wrote(&["config/settings.toml"]),
        refused_rows(&refused, &Manifest::new()),
        &Stopped::ApplyingAndRecording {
            applying: Error::Refused(refused.clone()),
            recording: held("state.lock"),
        },
    ));

    assert_eq!(document["recorded"], json!(false));
    assert_eq!(document["stopped_at"], json!("applying_and_recording"));
    let rendered = lines(&document, AmbiguousWidth::Narrow, false);
    assert_eq!(rendered.rows.len(), 2);
    // An action stopped this one, so the plan is half applied and the summary
    // says so, whatever became of the record.
    assert!(
        rendered
            .summary
            .as_deref()
            .is_some_and(|summary| summary.contains("stopped part-way through the plan")),
        "{:?}",
        rendered.summary
    );
    assert_eq!(
        rendered.stopped,
        vec![
            "the state directory does not record what the run applied: \
             state lock state.lock is held by another writer"
        ]
    );
}

#[test]
fn a_stopped_run_states_past_its_records_how_far_it_got_and_what_that_left() {
    let refused = Refused::one(
        Utf8PathBuf::from("bin/tool"),
        Refusal::Drift,
        Origin::Caller,
    );
    let unrecorded = "the state directory does not record what the run applied: \
                      state lock state.lock is held by another writer";

    // A refusal an action met, whose keys the records do carry: only how far
    // the run got is left to say.
    assert_eq!(
        warnings(&published_abort(AbortedRun::new(
            wrote(&["config/settings.toml"]),
            refused_rows(&refused, &Manifest::new()),
            &Stopped::Applying(Error::Refused(refused.clone())),
        ))),
        vec!["the run stopped part-way through the plan, and what it applied stands"]
    );

    // A failure the records carry nothing of, which is stated once past them.
    assert_eq!(
        warnings(&published_abort(AbortedRun::new(
            wrote(&["bin/tool"]),
            Report::default(),
            &Stopped::Applying(held("lock")),
        ))),
        vec![
            "the run stopped part-way through the plan, and what it applied stands",
            "state lock lock is held by another writer",
        ]
    );

    // Every action applied and the manifest did not, so the run is not called
    // part-way and the reader is told what the next run will make of the paths.
    assert_eq!(
        warnings(&published_abort(AbortedRun::new(
            wrote(&["bin/tool"]),
            Report::default(),
            &Stopped::Recording(held("state.lock")),
        ))),
        vec![
            "the run applied its whole plan and could not record it",
            unrecorded,
            "nothing in the state directory records what this run applied, so the \
             next run over this destination judges it against the record that stood \
             before the run",
        ]
    );

    // Both halves lost: the stage an action stopped, the record that failed
    // with it, and what the destination is left holding.
    assert_eq!(
        warnings(&published_abort(AbortedRun::new(
            wrote(&["config/settings.toml"]),
            refused_rows(&refused, &Manifest::new()),
            &Stopped::ApplyingAndRecording {
                applying: Error::Refused(refused.clone()),
                recording: held("state.lock"),
            },
        ))),
        vec![
            "the run stopped part-way through the plan, and what it applied stands",
            unrecorded,
            "nothing in the state directory records what this run applied, so the \
             next run over this destination judges it against the record that stood \
             before the run",
        ]
    );
}

#[test]
fn a_plan_and_a_run_that_finished_state_nothing_past_their_rows() {
    let refused = Refused::one(
        Utf8PathBuf::from("bin/tool"),
        Refusal::Drift,
        Origin::Caller,
    );
    for document in [
        published_plan(PlannedRun::refused(
            &refused,
            &Manifest::new(),
            BTreeSet::new(),
        )),
        serialized(RunView::Applied(Box::new(wrote(&["bin/tool"]).into()))),
    ] {
        assert!(warnings(&document).is_empty(), "{document}");
    }
}

#[test]
fn drops_at_the_top_level_leave_each_document_in_its_own_tense() {
    let erased = Dropped {
        member: Utf8PathBuf::from("._pkg"),
        prefix: Utf8PathBuf::new(),
        strip: 1,
        origin: Origin::Archive {
            path: Utf8PathBuf::from("/assets/vendor.tar.gz"),
            via: None,
        },
    };
    let refused = Refused::one(
        Utf8PathBuf::from("bin/tool"),
        Refusal::Drift,
        Origin::Caller,
    );
    let mut applied = wrote(&["bin/tool"]);
    applied.dropped = BTreeSet::from([erased.clone()]);

    let plan = published_plan(PlannedRun::refused(
        &refused,
        &Manifest::new(),
        BTreeSet::from([erased]),
    ));
    let stopped = published_abort(AbortedRun::new(
        applied,
        Report::default(),
        &Stopped::Applying(held("lock")),
    ));

    let verbs = |document: &JsonValue| {
        lines(document, AmbiguousWidth::Narrow, false)
            .rows
            .into_iter()
            .map(|row| row.verb)
            .collect::<Vec<_>>()
    };
    assert_eq!(verbs(&plan), vec!["would refuse", "dropped"]);
    assert_eq!(verbs(&stopped), vec!["wrote", "dropped"]);
}

#[test]
fn every_tense_names_its_phase_and_states_its_rows_at_the_same_key() {
    let refused = Refused::one(
        Utf8PathBuf::from("bin/tool"),
        Refusal::Drift,
        Origin::Caller,
    );
    let documents = [
        published_plan(PlannedRun::refused(
            &refused,
            &Manifest::new(),
            BTreeSet::new(),
        )),
        serialized(RunView::Applied(Box::new(wrote(&["bin/tool"]).into()))),
        published_abort(AbortedRun::new(
            wrote(&["bin/tool"]),
            refused_rows(&refused, &Manifest::new()),
            &Stopped::Applying(Error::Refused(refused.clone())),
        )),
    ];

    let keys = |document: &JsonValue| {
        document
            .as_object()
            .expect("a document")
            .keys()
            .map(String::as_str)
            .map(str::to_owned)
            .collect::<Vec<String>>()
    };
    assert_eq!(keys(&documents[0]), ["phase", "rows"]);
    assert_eq!(keys(&documents[1]), ["phase", "rows", "manifest"]);
    assert_eq!(
        keys(&documents[2]),
        ["phase", "rows", "manifest", "recorded", "stopped_at"]
    );
    for (document, phase) in documents.iter().zip(["planned", "applied", "aborted"]) {
        assert_eq!(document["phase"], json!(phase));
        assert_eq!(
            document["rows"][0]["path"],
            json!("bin/tool"),
            "{phase} states its rows at rows"
        );
    }
}

#[test]
fn csv_writes_one_record_per_path_under_one_header_in_either_tense() {
    let planned = published_plan(PlannedRun {
        report: Report {
            rows: BTreeMap::from([(
                Utf8PathBuf::from("bin/tool"),
                Row {
                    facts: Some(PathFacts {
                        shape: Some(libproiectio::PathShape::File { executable: true }),
                        owners: BTreeSet::from(["site".to_owned()]),
                        origin: Some(Origin::Tree {
                            path: Utf8PathBuf::from("/srv/skeleton"),
                        }),
                    }),
                    verdict: PlannedAction::Write,
                },
            )]),
        },
        dropped: BTreeSet::new(),
    });
    let applied = serialized(RunView::Applied(Box::new(wrote(&["bin/tool"]).into())));

    assert_eq!(
        projected(&planned),
        "path,verdict,detail,shape,executable,target,owners,origin,phase\n\
         bin/tool,Write,,file,true,,\"[\"\"site\"\"]\",\
         \"{\"\"Tree\"\":{\"\"path\"\":\"\"/srv/skeleton\"\"}}\",planned\n"
    );
    assert_eq!(
        projected(&applied),
        "path,verdict,detail,shape,executable,target,owners,origin,phase\n\
         bin/tool,Written,,,,,,,applied\n"
    );
}

#[test]
fn the_phase_cell_parts_the_two_tenses_that_spell_a_verdict_alike() {
    let record = |phase: &str, verdict: JsonValue| {
        projected(&json!({
            "phase": phase,
            "rows": [{ "path": "bin/tool", "verdict": verdict, "facts": null }],
        }))
        .lines()
        .nth(1)
        .expect("one record")
        .to_owned()
    };

    assert_eq!(
        record("planned", serialized(PlannedAction::NotRecorded)),
        "bin/tool,NotRecorded,,,,,,,planned"
    );
    assert_eq!(
        record("applied", serialized(ApplyOutcome::NotRecorded)),
        "bin/tool,NotRecorded,,,,,,,applied"
    );
}

#[test]
fn csv_over_a_stopped_run_states_the_keys_it_refused() {
    let refused = Refused::one(
        Utf8PathBuf::from("bin/tool"),
        Refusal::OwnerConflict {
            owners: BTreeSet::from(["site".to_owned()]),
        },
        Origin::Caller,
    );

    let document = published_abort(AbortedRun::new(
        wrote(&["config/settings.toml"]),
        refused_rows(&refused, &holding("bin/tool", &["site"])),
        &Stopped::Applying(Error::Refused(refused)),
    ));

    assert_eq!(
        projected(&document),
        "path,verdict,detail,shape,executable,target,owners,origin,phase\n\
         bin/tool,Refuse,\"{\"\"refusal\"\":{\"\"OwnerConflict\"\":{\"\"owners\"\":\
         [\"\"site\"\"]}}}\",,,,\"[\"\"site\"\"]\",Caller,aborted\n\
         config/settings.toml,Written,,,,,,,aborted\n"
    );
}

#[test]
fn the_phase_the_renderer_matches_on_is_the_one_serde_writes() {
    let refused = Refused::one(
        Utf8PathBuf::from("bin/tool"),
        Refusal::Drift,
        Origin::Caller,
    );
    let planned = published_plan(PlannedRun::refused(
        &refused,
        &Manifest::new(),
        BTreeSet::new(),
    ));
    let acting = [
        serialized(RunView::Applied(Box::new(wrote(&["bin/tool"]).into()))),
        published_abort(AbortedRun::new(
            wrote(&["bin/tool"]),
            Report::default(),
            &Stopped::Applying(held("lock")),
        )),
    ];

    assert_eq!(planned["phase"], json!(PLANNED));
    for document in &acting {
        assert_ne!(document["phase"], json!(PLANNED), "{document}");
    }
}

#[test]
fn a_verdict_carrying_a_payload_names_itself_and_states_what_it_carries() {
    let detailed = |verdict: JsonValue| {
        let document =
            json!({ "phase": "planned", "rows": [{ "path": "one", "verdict": verdict }] });
        projected(&document)
            .lines()
            .nth(1)
            .expect("one record")
            .to_owned()
    };

    assert_eq!(detailed(json!("Write")), "one,Write,,,,,,,planned");
    assert_eq!(
        detailed(serialized(PlannedAction::Overwrite {
            reason: OverwriteReason::ContentChanged,
        })),
        "one,Overwrite,\"{\"\"reason\"\":\"\"ContentChanged\"\"}\",,,,,,planned"
    );
    assert_eq!(
        detailed(serialized(PlannedAction::Refuse {
            refusal: Refusal::OwnerConflict {
                owners: BTreeSet::from(["site".to_owned()]),
            },
        })),
        "one,Refuse,\"{\"\"refusal\"\":{\"\"OwnerConflict\"\":{\"\"owners\"\":[\"\"site\"\"]}}}\",\
         ,,,,,planned"
    );
}

#[test]
fn a_link_row_states_the_target_it_writes() {
    let document = json!({
        "phase": "planned",
        "rows": records(json!({ "current": link(json!("Write"), "releases/1.2.3") })),
    });

    assert_eq!(
        projected(&document),
        "path,verdict,detail,shape,executable,target,owners,origin,phase\n\
         current,Write,,symlink,,releases/1.2.3,,,planned\n"
    );
}

#[test]
fn a_dropped_member_takes_no_csv_record() {
    let document = published_plan(PlannedRun {
        report: Report::default(),
        dropped: BTreeSet::from([Dropped {
            member: Utf8PathBuf::from("._pkg"),
            prefix: Utf8PathBuf::new(),
            strip: 1,
            origin: Origin::Archive {
                path: Utf8PathBuf::from("/assets/vendor.tar.gz"),
                via: None,
            },
        }]),
    });

    let header = "path,verdict,detail,shape,executable,target,owners,origin,phase\n";
    assert_eq!(projected(&document), header);

    let stopped = published_abort(AbortedRun::new(
        wrote(&[]),
        Report::default(),
        &Stopped::Recording(held("state.lock")),
    ));
    assert_eq!(projected(&stopped), header);
}

fn projected(document: &JsonValue) -> String {
    csv()
        .csv_projection()
        .render(document)
        .expect("a CSV projection")
}

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

fn held(path: &str) -> Error {
    Error::LockHeld {
        path: Utf8PathBuf::from(path),
    }
}

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

#[test]
fn a_containment_row_names_the_symlink_ancestor_the_refusal_carries() {
    let through_config = Refusal::Containment {
        through: Some(Utf8PathBuf::from("config")),
    };
    let from_library = Refused::one(
        Utf8PathBuf::from("config/app.toml"),
        through_config.clone(),
        Origin::Caller,
    )
    .to_string();
    let row = only(refused(&serialized(&through_config)));

    assert_eq!(
        row.note.as_deref(),
        Some("(containment) (below the symlink config)")
    );
    assert!(
        from_library.ends_with("(below the symlink config)"),
        "the library's own message says it too: {from_library}"
    );

    let row = only(refused(&serialized(Refusal::Containment { through: None })));

    assert_eq!(row.note.as_deref(), Some("(containment)"));
}

#[test]
fn the_rows_close_with_what_lifts_each_kind_of_refusal_among_them_once() {
    let refuse = |refusal: JsonValue| json!({ "facts": null, "verdict": { "Refuse": { "refusal": refusal } } });
    let document = planned(json!({
        "bin/tool": refuse(serialized(Refusal::Drift)),
        "etc/rc": refuse(serialized(Refusal::Drift)),
        "link": refuse(serialized(Refusal::ExternalTarget { target: "/opt".to_owned() })),
    }));

    assert_eq!(
        document.hints,
        [
            "pass --force to touch them anyway, where the projection can still tell what it \
             would replace",
            "pass --allow-external-targets to write them",
        ]
    );
}

#[test]
fn a_run_that_already_passed_force_is_not_told_to_pass_force() {
    let refuse = |refusal: JsonValue| json!({ "facts": null, "verdict": { "Refuse": { "refusal": refusal } } });
    let rows = json!({
        "bin/tool": refuse(serialized(Refusal::Drift)),
        "link": refuse(serialized(Refusal::ExternalTarget { target: "/opt".to_owned() })),
    });
    let document = |forced: bool| {
        lines(
            &json!({ "phase": "planned", "rows": records(rows.clone()) }),
            AmbiguousWidth::Narrow,
            forced,
        )
    };

    assert_eq!(
        document(true).hints,
        ["pass --allow-external-targets to write them"]
    );
    assert_eq!(document(false).hints.len(), 2, "and stays without the flag");
}

#[test]
fn rows_nothing_lifts_close_with_nothing() {
    let document = refused(&serialized(Refusal::Containment { through: None }));
    assert!(document.hints.is_empty());

    let document = planned(json!({ "one": file(json!("Write")) }));
    assert!(document.hints.is_empty());
}

#[test]
fn every_kind_the_library_lifts_finds_its_hint_through_the_serialized_name() {
    let declared = [
        RefusalKind::Containment,
        RefusalKind::TreeConflict,
        RefusalKind::RecordedLanding,
        RefusalKind::Foreign,
        RefusalKind::Drift,
        RefusalKind::DirectoryInTheWay,
        RefusalKind::OwnerConflict,
        RefusalKind::ExternalTarget,
        RefusalKind::InvalidTarget,
        RefusalKind::Block,
    ];
    for kind in declared {
        match kind {
            RefusalKind::Containment
            | RefusalKind::TreeConflict
            | RefusalKind::RecordedLanding
            | RefusalKind::Foreign
            | RefusalKind::Drift
            | RefusalKind::DirectoryInTheWay
            | RefusalKind::OwnerConflict
            | RefusalKind::ExternalTarget
            | RefusalKind::InvalidTarget
            | RefusalKind::Block => {}
        }
        let name = kind_name(kind).expect("a kind serializes as its name");

        assert!(
            REFUSAL_KINDS.contains(&kind),
            "REFUSAL_KINDS leaves out {kind:?}, whose hint the view would drop"
        );
        assert_eq!(hinting(&name, false), kind.override_hint(), "{kind:?}");
    }
    // The other direction: nothing sits in the array that the list above no
    // longer names, so the two cannot drift apart in either one.
    assert_eq!(REFUSAL_KINDS.len(), declared.len());
}

#[test]
fn an_unknown_refusal_kind_carries_no_hint() {
    assert_eq!(hinting("Pondered", false), None);
}
