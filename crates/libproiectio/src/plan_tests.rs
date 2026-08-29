use camino::Utf8PathBuf;

use super::*;

#[test]
fn a_plan_serializes_with_paths_as_keys() {
    let plan = Plan {
        owner: "site".to_owned(),
        origins: BTreeMap::new(),
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
    };

    let json = serde_json::to_value(&plan).expect("serialize");

    assert_eq!(json["owner"], "site");
    // The permission the plan was decided under rides along: apply reads it
    // to know whether a re-graded target has a verdict to hold to.
    assert_eq!(json["external_targets"], "Allow");
    assert_eq!(
        json["actions"]["config/settings.toml"]["Skip"]["expected"]["hash"],
        "dd44"
    );
    assert_eq!(json["actions"]["shared/.zshrc"], "Release");
    assert_eq!(
        json["actions"]["bin/tool"]["Overwrite"]["expected"]["hash"],
        "aa11"
    );
    assert_eq!(
        json["actions"]["bin/tool"]["Overwrite"]["reason"],
        "ForcedDrift"
    );
    assert_eq!(
        json["actions"]["toolchain"]["Refuse"]["refusal"]["ExternalTarget"]["target"],
        "/opt/rust"
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
