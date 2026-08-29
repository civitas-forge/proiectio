use std::collections::BTreeSet;

use camino::{Utf8Path, Utf8PathBuf};

use super::*;
use crate::RefusalKind;

// A plan holding one of every action, with the facts each row draws on:
// an entry for the writes, an expected signature for the skips and the
// remove that has one, nothing for the rest.
fn one_of_each() -> Plan {
    Plan {
        owner: "site".to_owned(),
        origins: BTreeMap::from([(
            Utf8PathBuf::from("bin/tool"),
            Origin::Mapping {
                path: Utf8PathBuf::from("/etc/deploy.toml"),
            },
        )]),
        external_targets: ExternalTargetPolicy::Allow,
        actions: BTreeMap::from([
            (
                Utf8PathBuf::from("bin/tool"),
                Action::Overwrite {
                    entry: Entry::File {
                        contents: b"#!/bin/sh\n".to_vec(),
                        executable: true,
                    },
                    expected: NodeSignature {
                        kind: EntryKind::File,
                        hash: "aa11".to_owned(),
                        executable: true,
                    },
                    reason: OverwriteReason::ForcedDrift,
                },
            ),
            (
                Utf8PathBuf::from("config/settings.toml"),
                Action::Skip {
                    expected: NodeSignature {
                        kind: EntryKind::File,
                        hash: "dd44".to_owned(),
                        executable: false,
                    },
                },
            ),
            (
                Utf8PathBuf::from("current"),
                Action::Write {
                    entry: Entry::Symlink {
                        target: "releases/1.2.3".to_owned(),
                    },
                },
            ),
            (
                Utf8PathBuf::from("gone.txt"),
                Action::Remove { expected: None },
            ),
            (
                Utf8PathBuf::from("orphan.txt"),
                Action::Remove {
                    expected: Some(NodeSignature {
                        kind: EntryKind::File,
                        hash: "bb22".to_owned(),
                        executable: false,
                    }),
                },
            ),
            (Utf8PathBuf::from("shared/.zshrc"), Action::Release),
            (
                Utf8PathBuf::from("toolchain"),
                Action::Refuse {
                    refusal: Refusal::ExternalTarget {
                        target: "/opt/rust".to_owned(),
                    },
                },
            ),
        ]),
    }
}

fn verdict(report: &Report<PlannedAction>, path: &str) -> PlannedAction {
    report.rows[Utf8Path::new(path)].verdict.clone()
}

fn facts(report: &Report<PlannedAction>, path: &str) -> Option<PathFacts> {
    report.rows[Utf8Path::new(path)].facts.clone()
}

#[test]
fn a_report_carries_one_verdict_per_planned_path() {
    let report = one_of_each().report();

    assert_eq!(
        report
            .iter()
            .map(|(path, row)| (path.as_str(), row.verdict.clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                "bin/tool",
                PlannedAction::Overwrite {
                    reason: OverwriteReason::ForcedDrift
                }
            ),
            ("config/settings.toml", PlannedAction::Skip),
            ("current", PlannedAction::Write),
            ("gone.txt", PlannedAction::Remove),
            ("orphan.txt", PlannedAction::Remove),
            ("shared/.zshrc", PlannedAction::Release),
            (
                "toolchain",
                PlannedAction::Refuse {
                    refusal: Refusal::ExternalTarget {
                        target: "/opt/rust".to_owned()
                    }
                }
            ),
        ]
    );
}

// A plan does not carry the manifest, so no row names an owner.
#[test]
fn write_and_overwrite_rows_take_their_facts_from_the_entry() {
    let report = one_of_each().report();

    assert_eq!(
        facts(&report, "bin/tool"),
        Some(PathFacts {
            shape: PathShape::File { executable: true },
            owners: BTreeSet::new(),
            origin: Some(Origin::Mapping {
                path: Utf8PathBuf::from("/etc/deploy.toml")
            }),
        })
    );
}

#[test]
fn a_planned_symlink_carries_its_target() {
    let report = one_of_each().report();

    assert_eq!(
        facts(&report, "current"),
        Some(PathFacts {
            shape: PathShape::Symlink {
                target: Some("releases/1.2.3".to_owned())
            },
            owners: BTreeSet::new(),
            origin: Some(Origin::Caller),
        })
    );
}

#[test]
fn skip_and_remove_rows_take_their_facts_from_the_expected_signature() {
    let report = one_of_each().report();

    assert_eq!(
        facts(&report, "config/settings.toml"),
        Some(PathFacts {
            shape: PathShape::File { executable: false },
            owners: BTreeSet::new(),
            origin: Some(Origin::Caller),
        })
    );
    assert_eq!(
        facts(&report, "orphan.txt").map(|facts| facts.shape),
        Some(PathShape::File { executable: false })
    );
}

// Nothing on disk is expected at these paths, so there is nothing to state.
#[test]
fn rows_without_an_entry_or_a_signature_carry_no_facts() {
    let report = one_of_each().report();

    assert_eq!(facts(&report, "gone.txt"), None);
    assert_eq!(facts(&report, "shared/.zshrc"), None);
    assert_eq!(facts(&report, "toolchain"), None);
}

#[test]
fn a_summary_counts_the_rows_of_each_action() {
    let report = one_of_each().report();

    assert_eq!(
        report.summary(),
        BTreeMap::from([
            (PlannedAction::Write, 1),
            (
                PlannedAction::Overwrite {
                    reason: OverwriteReason::ForcedDrift
                },
                1
            ),
            (PlannedAction::Skip, 1),
            (PlannedAction::Remove, 2),
            (PlannedAction::Release, 1),
            (
                PlannedAction::Refuse {
                    refusal: Refusal::ExternalTarget {
                        target: "/opt/rust".to_owned()
                    }
                },
                1
            ),
        ])
    );
}

#[test]
fn a_report_serializes_with_paths_as_keys_and_no_bytes() {
    let json = serde_json::to_value(one_of_each().report()).expect("serialize");

    assert_eq!(
        json["rows"]["bin/tool"],
        serde_json::json!({
            "facts": {
                "shape": { "File": { "executable": true } },
                "owners": [],
                "origin": { "Mapping": { "path": "/etc/deploy.toml" } },
            },
            "verdict": { "Overwrite": { "reason": "ForcedDrift" } },
        })
    );
    assert_eq!(json["rows"]["shared/.zshrc"]["verdict"], "Release");
    assert_eq!(
        json["rows"]["toolchain"]["verdict"]["Refuse"]["refusal"]["ExternalTarget"]["target"],
        "/opt/rust"
    );
}

#[test]
fn a_plan_that_refuses_nothing_is_not_refused() {
    let mut plan = one_of_each();
    plan.actions.remove(Utf8Path::new("toolchain"));

    assert_eq!(plan.refused(), None);
    assert_eq!(
        verdict(&plan.report(), "bin/tool"),
        PlannedAction::Overwrite {
            reason: OverwriteReason::ForcedDrift
        }
    );
}

// Several kinds refused, one error: the least kind by declaration, carrying
// every path refused for it.
#[test]
fn refused_reduces_the_plans_refusals_to_one_kind() {
    let mut plan = one_of_each();
    plan.actions.insert(
        Utf8PathBuf::from("../escape"),
        Action::Refuse {
            refusal: Refusal::Containment,
        },
    );
    plan.actions.insert(
        Utf8PathBuf::from("theirs.txt"),
        Action::Refuse {
            refusal: Refusal::Foreign,
        },
    );

    let refused = plan.refused().expect("the plan refuses three paths");

    assert_eq!(refused.kind(), RefusalKind::Containment);
    assert_eq!(
        refused.keys().collect::<Vec<_>>(),
        vec![Utf8Path::new("../escape")]
    );
}

#[test]
fn drift_policy_defaults_to_refuse() {
    assert_eq!(DriftPolicy::default(), DriftPolicy::Refuse);
}

#[test]
fn external_target_policy_defaults_to_refuse() {
    assert_eq!(
        ExternalTargetPolicy::default(),
        ExternalTargetPolicy::Refuse
    );
}

#[test]
fn plan_options_default_to_the_strict_projection() {
    // Both policies lift a rule, so the default has to refuse both: a
    // caller that names neither gets the strict projection.
    assert_eq!(
        PlanOptions::default(),
        PlanOptions {
            drift: DriftPolicy::Refuse,
            external_targets: ExternalTargetPolicy::Refuse,
        }
    );
}
