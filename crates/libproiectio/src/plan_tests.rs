use camino::Utf8PathBuf;

use super::*;

#[test]
fn a_plan_serializes_with_paths_as_keys() {
    let plan = Plan {
        owner: "site".to_owned(),
        actions: BTreeMap::from([
            (
                Utf8PathBuf::from("bin/tool"),
                Action::Overwrite {
                    entry: Entry::File {
                        contents: b"#!/bin/sh\n".to_vec(),
                        executable: true,
                    },
                    expected_hash: "aa11".to_owned(),
                },
            ),
            (
                Utf8PathBuf::from("config/settings.toml"),
                Action::Skip {
                    expected_hash: "dd44".to_owned(),
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
    assert_eq!(
        json["actions"]["config/settings.toml"]["Skip"]["expected_hash"],
        "dd44"
    );
    assert_eq!(json["actions"]["shared/.zshrc"], "Release");
    assert_eq!(
        json["actions"]["bin/tool"]["Overwrite"]["expected_hash"],
        "aa11"
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
