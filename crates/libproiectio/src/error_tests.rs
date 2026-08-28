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
        Error::TreeNameNotUtf8 {
            path: Utf8PathBuf::from("/srv/skeleton/bin"),
            name: "tool\u{fffd}".to_owned(),
        },
        Error::TreeTargetNotUtf8 {
            path: Utf8PathBuf::from("/srv/skeleton/current"),
            target: "releases/\u{fffd}".to_owned(),
        },
        Error::TreeNodeKind {
            path: Utf8PathBuf::from("/srv/skeleton/run.sock"),
        },
        Error::TreeTooDeep {
            path: Utf8PathBuf::from("/srv/skeleton/a/b/c"),
            limit: 64,
        },
        Error::DestinationTooDeep {
            path: Utf8PathBuf::from("vendor/a/b/c"),
            limit: 64,
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
        vec![
            2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1
        ]
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

/// A source tree carrying something a desired tree cannot express fails the
/// load rather than declining a destination path, so these are exit-1
/// failures — and each names where in the source the trouble sits. The
/// undecodable pieces are quoted so their edges show — a name is otherwise
/// free to start or end in a space, or to render as nothing at all. Quoting
/// does not recover what the lossy decode dropped: a replacement character
/// stands for bytes with no UTF-8 spelling and for itself alike.
#[test]
fn tree_source_messages_name_the_node_and_exit_1() {
    let name = Error::TreeNameNotUtf8 {
        path: Utf8PathBuf::from("/srv/skeleton/bin"),
        name: "tool\u{fffd}".to_owned(),
    };
    assert!(!name.is_refusal());
    assert_eq!(
        name.to_string(),
        "tree entry name under /srv/skeleton/bin is not UTF-8: \"tool\u{fffd}\""
    );

    let target = Error::TreeTargetNotUtf8 {
        path: Utf8PathBuf::from("/srv/skeleton/current"),
        target: "releases/\u{fffd}".to_owned(),
    };
    assert!(!target.is_refusal());
    assert_eq!(
        target.to_string(),
        "tree symlink /srv/skeleton/current has a target that is not UTF-8: \
         \"releases/\u{fffd}\""
    );

    let kind = Error::TreeNodeKind {
        path: Utf8PathBuf::from("/srv/skeleton/run.sock"),
    };
    assert!(!kind.is_refusal());
    assert_eq!(
        kind.to_string(),
        "tree node /srv/skeleton/run.sock is not a file, directory, or symlink"
    );

    let deep = Error::TreeTooDeep {
        path: Utf8PathBuf::from("/srv/skeleton/a/b/c"),
        limit: 64,
    };
    assert!(!deep.is_refusal());
    assert_eq!(
        deep.to_string(),
        "tree directory /srv/skeleton/a/b/c nests deeper than the 64 levels \
         a source tree may carry"
    );
}

/// The destination's depth error is the source tree's bound applied to the
/// other tree, and it says so: same limit, different tree, and a path spelled
/// relative to the destination rather than absolutely. Both are exit-1
/// failures — nothing is being declined, the walk cannot be taken at all.
#[test]
fn a_destination_too_deep_names_the_directory_and_exits_1() {
    let deep = Error::DestinationTooDeep {
        path: Utf8PathBuf::from("vendor/a/b/c"),
        limit: crate::MAX_WALK_DEPTH,
    };
    assert!(!deep.is_refusal());
    assert_eq!(
        deep.to_string(),
        "destination directory vendor/a/b/c nests deeper than the 64 levels \
         a destination may carry"
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
