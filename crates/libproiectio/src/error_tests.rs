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
        Error::OwnerConflict {
            conflicts: BTreeMap::from([(
                Utf8PathBuf::from("shared/.zshrc"),
                ["dotfiles".to_owned()].into_iter().collect(),
            )]),
        },
        Error::ExternalTarget {
            links: BTreeMap::from([(Utf8PathBuf::from("toolchain"), "/opt/rust".to_owned())]),
        },
        Error::InvalidTarget {
            links: BTreeMap::from([(Utf8PathBuf::from("rc"), String::new())]),
        },
        Error::TreeConflict {
            paths: paths(&["a", "a/b"]),
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
        Error::LockHeld {
            path: Utf8PathBuf::from(crate::LOCK_FILE_NAME),
        },
        Error::MappingFormat {
            path: Utf8PathBuf::from("deploy.toml"),
            source: toml::from_str::<crate::Manifest>("not toml").expect_err("parse failure"),
        },
        Error::MappingVersion {
            path: Utf8PathBuf::from("deploy.toml"),
            found: 9,
            supported: crate::MAPPING_VERSION,
        },
        Error::MappingContentsXorSource {
            path: Utf8PathBuf::from("deploy.toml"),
            key: Utf8PathBuf::from("bin/tool"),
        },
        Error::MappingDuplicate {
            path: Utf8PathBuf::from("deploy.toml"),
            key: Utf8PathBuf::from("bin/tool"),
        },
        Error::MappingArchiveUnimplemented {
            path: Utf8PathBuf::from("deploy.toml"),
            keys: paths(&["vendor/"]),
        },
        Error::ApplyBlockUnimplemented {
            paths: paths(&["shared/.zshrc"]),
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

    assert_eq!(
        codes,
        vec![2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1]
    );
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

    // Quoted, because these are the targets a bare rendering would hide.
    let invalid = Error::InvalidTarget {
        links: BTreeMap::from([
            (Utf8PathBuf::from("rc"), String::new()),
            (Utf8PathBuf::from("zed"), "a\0b".to_owned()),
        ]),
    };
    assert_eq!(
        invalid.to_string(),
        "refusing symlinks whose targets are not paths: rc -> \"\", zed -> \"a\\0b\""
    );

    let conflict = Error::OwnerConflict {
        conflicts: BTreeMap::from([(
            Utf8PathBuf::from("shared/.zshrc"),
            ["dotfiles".to_owned(), "site".to_owned()]
                .into_iter()
                .collect(),
        )]),
    };
    assert_eq!(
        conflict.to_string(),
        "refusing paths whose desired entries conflict with another owner's: \
         shared/.zshrc (held by dotfiles+site)"
    );

    let tree_conflict = Error::TreeConflict {
        paths: paths(&["a", "a/b"]),
    };
    assert_eq!(
        tree_conflict.to_string(),
        "refusing desired paths that claim overlapping locations: a, a/b"
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

/// The single-writer lock's contention variant is exit-1 territory: not a
/// refusal, and its message names the state-dir-relative lock path.
#[test]
fn lock_held_exits_1_and_names_the_lock_path() {
    let error = Error::LockHeld {
        path: Utf8PathBuf::from(crate::LOCK_FILE_NAME),
    };
    assert!(!error.is_refusal());
    assert_eq!(exit_code(Err(error)), 1);
    let error = Error::LockHeld {
        path: Utf8PathBuf::from(crate::LOCK_FILE_NAME),
    };
    assert_eq!(
        error.to_string(),
        "state lock proiectio.lock is held by another writer"
    );
}
