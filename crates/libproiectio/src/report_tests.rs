use camino::Utf8PathBuf;

use super::*;
use crate::PathState;

fn facts(shape: PathShape, origin: Option<Origin>) -> PathFacts {
    PathFacts {
        shape: Some(shape),
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
                    Some(facts(
                        PathShape::File { executable: true },
                        Some(Origin::Mapping {
                            path: Utf8PathBuf::from("/etc/deploy.toml"),
                        }),
                    )),
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

// The one row a single-row report serializes, so a case reading fields can
// name them without indexing past a sequence.
fn serialized_row(report: &Report<PathState>) -> serde_json::Value {
    let json = serde_json::to_value(report).expect("serialize");
    let mut rows = json["rows"].as_array().expect("a rows array").clone();
    assert_eq!(rows.len(), 1, "one row");
    rows.remove(0)
}

#[test]
fn a_report_serializes_one_record_per_row_and_no_bytes() {
    let json = serde_json::to_value(two_rows()).expect("serialize");

    assert_eq!(
        json,
        serde_json::json!({
            "rows": [
                {
                    "path": "bin/tool",
                    "verdict": "Drifted",
                    "facts": {
                        "shape": { "File": { "executable": true } },
                        "owners": ["site"],
                        "origin": { "Mapping": { "path": "/etc/deploy.toml" } },
                    },
                },
                {
                    "path": "theirs.txt",
                    "verdict": "Foreign",
                    "facts": null,
                },
            ]
        })
    );
}

// A path is a value in every format the CLI writes, so two paths one XML
// element name cannot tell apart still serialize as two rows.
#[test]
fn paths_that_share_an_xml_element_name_serialize_as_separate_rows() {
    let report = Report {
        rows: BTreeMap::from([
            (Utf8PathBuf::from("a/b"), row(None, PathState::Foreign)),
            (Utf8PathBuf::from("a_b"), row(None, PathState::Foreign)),
        ]),
    };

    let json = serde_json::to_value(&report).expect("serialize");

    let paths: Vec<&str> = json["rows"]
        .as_array()
        .expect("a rows array")
        .iter()
        .map(|row| row["path"].as_str().expect("a path"))
        .collect();
    assert_eq!(paths, vec!["a/b", "a_b"]);
}

#[test]
fn a_symlink_row_carries_its_target_verbatim() {
    let report = Report {
        rows: BTreeMap::from([(
            Utf8PathBuf::from("current"),
            row(
                Some(facts(
                    PathShape::Symlink {
                        target: Some("releases/1.2.3".to_owned()),
                    },
                    Some(Origin::Caller),
                )),
                PathState::Clean,
            ),
        )]),
    };

    let json = serialized_row(&report);

    assert_eq!(json["path"], "current");
    assert_eq!(
        json["facts"]["shape"]["Symlink"]["target"],
        "releases/1.2.3"
    );
    assert_eq!(json["facts"]["origin"], "Caller");
}

// A status row knows the manifest's hash of a link target rather than the
// string, and no source named the path.
#[test]
fn a_row_can_name_neither_target_nor_origin() {
    let report = Report {
        rows: BTreeMap::from([(
            Utf8PathBuf::from("current"),
            row(
                Some(facts(PathShape::Symlink { target: None }, None)),
                PathState::Clean,
            ),
        )]),
    };

    let json = serialized_row(&report);

    assert!(json["facts"]["shape"]["Symlink"]["target"].is_null());
    assert!(json["facts"]["origin"].is_null());
}
