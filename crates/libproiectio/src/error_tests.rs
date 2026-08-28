use std::collections::{BTreeMap, BTreeSet};

use camino::Utf8PathBuf;

use super::*;
use crate::Manifest;

fn paths(names: &[&str]) -> BTreeSet<Utf8PathBuf> {
    names.iter().map(Utf8PathBuf::from).collect()
}

fn every_variant() -> Vec<Error> {
    vec![
        Error::Drift {
            paths: paths(&["bin/tool"]),
        },
        Error::Foreign {
            paths: paths(&["notes.txt"]),
        },
        Error::Containment {
            paths: paths(&["../escape", "/etc/passwd"]),
        },
        Error::ExternalTarget {
            links: BTreeMap::from([(Utf8PathBuf::from("toolchain"), "/opt/rust".to_owned())]),
        },
        Error::Io {
            path: Utf8PathBuf::from("config/settings.toml"),
            source: std::io::Error::other("disk full"),
        },
        Error::ManifestFormat {
            path: Utf8PathBuf::from(".proiectio/manifest.json"),
            source: serde_json::from_str::<Manifest>("not json").expect_err("parse failure"),
        },
        Error::ManifestVersion {
            path: Utf8PathBuf::from(".proiectio/manifest.json"),
            found: 9,
            supported: crate::MANIFEST_VERSION,
        },
    ]
}

/// The CLI's 0/1/2 exit contract, as one match over `is_refusal`.
fn exit_code(result: Result<()>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(error) if error.is_refusal() => 2,
        Err(_) => 1,
    }
}

#[test]
fn refusals_exit_2_and_failures_exit_1() {
    let codes: Vec<i32> = every_variant()
        .into_iter()
        .map(|error| exit_code(Err(error)))
        .collect();

    assert_eq!(codes, vec![2, 2, 2, 2, 1, 1, 1]);
    assert_eq!(exit_code(Ok(())), 0);
}

#[test]
fn refusal_messages_name_the_offending_paths() {
    let drift = Error::Drift {
        paths: paths(&["bin/tool", "config/settings.toml"]),
    };
    assert_eq!(
        drift.to_string(),
        "refusing to touch drifted paths (edited on disk): bin/tool, config/settings.toml"
    );

    let external = Error::ExternalTarget {
        links: BTreeMap::from([(Utf8PathBuf::from("toolchain"), "/opt/rust".to_owned())]),
    };
    assert_eq!(
        external.to_string(),
        "refusing symlinks with targets outside the destination: toolchain -> /opt/rust"
    );
}

#[test]
fn io_messages_keep_the_os_error_visible() {
    let error = Error::Io {
        path: Utf8PathBuf::from("bin/tool"),
        source: std::io::Error::other("disk full"),
    };
    assert_eq!(error.to_string(), "bin/tool: disk full");
}
