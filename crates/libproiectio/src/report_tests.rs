use camino::Utf8PathBuf;

use super::*;
use crate::PathState;

fn facts(kind: EntryKind, origin: Origin) -> PathFacts {
    PathFacts {
        kind,
        executable: false,
        target: None,
        owners: BTreeSet::from(["site".to_owned()]),
        origin,
    }
}

fn row(facts: Option<PathFacts>, verdict: PathState) -> Row<PathState> {
    Row { facts, verdict }
}

// The two rows of a status listing: a path the manifest records, and one
// only the disk knows about.
fn two_rows() -> Report<PathState> {
    Report {
        rows: BTreeMap::from([
            (
                Utf8PathBuf::from("bin/tool"),
                row(
                    Some(PathFacts {
                        executable: true,
                        ..facts(
                            EntryKind::File,
                            Origin::Mapping {
                                path: Utf8PathBuf::from("/etc/deploy.toml"),
                            },
                        )
                    }),
                    PathState::Drifted,
                ),
            ),
            (
                Utf8PathBuf::from("theirs.txt"),
                row(None, PathState::Foreign),
            ),
        ]),
    }
}

#[test]
fn iterating_walks_every_row_in_path_order() {
    let report = two_rows();

    assert!(!report.is_empty());
    assert_eq!(
        report
            .iter()
            .map(|(path, row)| (path.as_str(), row.verdict))
            .collect::<Vec<_>>(),
        vec![
            ("bin/tool", PathState::Drifted),
            ("theirs.txt", PathState::Foreign)
        ]
    );
}

#[test]
fn a_summary_counts_the_rows_of_each_verdict() {
    let report = Report {
        rows: BTreeMap::from([
            (Utf8PathBuf::from("a.txt"), row(None, PathState::Clean)),
            (Utf8PathBuf::from("b.txt"), row(None, PathState::Clean)),
            (Utf8PathBuf::from("c.txt"), row(None, PathState::Drifted)),
        ]),
    };

    assert_eq!(
        report.summary(),
        BTreeMap::from([(PathState::Clean, 2), (PathState::Drifted, 1)])
    );
}

// A verdict no row carries is absent rather than zero, and the verdicts
// present come out in the order the enum declares them.
#[test]
fn a_summary_orders_verdicts_by_declaration() {
    let report = Report {
        rows: BTreeMap::from([
            (Utf8PathBuf::from("a.txt"), row(None, PathState::Foreign)),
            (Utf8PathBuf::from("b.txt"), row(None, PathState::Clean)),
            (Utf8PathBuf::from("c.txt"), row(None, PathState::Missing)),
        ]),
    };

    assert_eq!(
        report.summary().into_keys().collect::<Vec<_>>(),
        vec![PathState::Clean, PathState::Missing, PathState::Foreign]
    );
}

#[test]
fn an_empty_report_summarizes_to_nothing() {
    let report = Report::<PathState> {
        rows: BTreeMap::new(),
    };

    assert!(report.is_empty());
    assert_eq!(report.iter().count(), 0);
    assert!(report.summary().is_empty());
}

#[test]
fn a_report_serializes_with_paths_as_keys_and_no_bytes() {
    let json = serde_json::to_value(two_rows()).expect("serialize");

    assert_eq!(
        json,
        serde_json::json!({
            "rows": {
                "bin/tool": {
                    "facts": {
                        "kind": "File",
                        "executable": true,
                        "target": null,
                        "owners": ["site"],
                        "origin": { "Mapping": { "path": "/etc/deploy.toml" } },
                    },
                    "verdict": "Drifted",
                },
                "theirs.txt": {
                    "facts": null,
                    "verdict": "Foreign",
                },
            }
        })
    );
}

#[test]
fn a_symlink_row_carries_its_target_verbatim() {
    let report = Report {
        rows: BTreeMap::from([(
            Utf8PathBuf::from("current"),
            row(
                Some(PathFacts {
                    target: Some("releases/1.2.3".to_owned()),
                    ..facts(EntryKind::Symlink, Origin::Caller)
                }),
                PathState::Clean,
            ),
        )]),
    };

    let json = serde_json::to_value(&report).expect("serialize");

    assert_eq!(json["rows"]["current"]["facts"]["target"], "releases/1.2.3");
    assert_eq!(json["rows"]["current"]["facts"]["origin"], "Caller");
}
