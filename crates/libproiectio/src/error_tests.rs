use std::collections::BTreeSet;

use camino::Utf8PathBuf;

use super::*;
use crate::{BlockFault, Manifest, Origin, Refusal, RefusalKind, Refused};

fn every_variant() -> Vec<Error> {
    let refusal = |kind: RefusalKind| -> Error {
        let refusal = match kind {
            RefusalKind::Drift => Refusal::Drift,
            RefusalKind::Foreign => Refusal::Foreign,
            RefusalKind::Containment => Refusal::Containment,
            RefusalKind::TreeConflict => Refusal::TreeConflict {
                paths: BTreeSet::new(),
            },
            RefusalKind::OwnerConflict => Refusal::OwnerConflict {
                owners: BTreeSet::new(),
            },
            RefusalKind::ExternalTarget => Refusal::ExternalTarget {
                target: "/opt/rust".to_owned(),
            },
            RefusalKind::InvalidTarget => Refusal::InvalidTarget {
                target: String::new(),
            },
            RefusalKind::Block => Refusal::Block {
                fault: BlockFault::ContainerMissing,
            },
        };
        Refused::one(Utf8PathBuf::from("bin/tool"), refusal, Origin::Caller).into()
    };
    let mut every: Vec<Error> = [
        RefusalKind::Containment,
        RefusalKind::TreeConflict,
        RefusalKind::Foreign,
        RefusalKind::Drift,
        RefusalKind::OwnerConflict,
        RefusalKind::ExternalTarget,
        RefusalKind::InvalidTarget,
        RefusalKind::Block,
    ]
    .into_iter()
    .map(refusal)
    .collect();
    every.extend([
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
        Error::CurrentDirectory {
            source: std::io::Error::other("no such directory"),
        },
        Error::PathNotUtf8 {
            path: "/srv/si\u{fffd}te".to_owned(),
        },
        Error::StateDirIsTarget {
            path: Utf8PathBuf::from("/srv/site"),
        },
        Error::MappingFormat {
            path: Utf8PathBuf::from("deploy.toml"),
            source: toml::from_str::<toml::Table>("not toml").expect_err("parse failure"),
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
        Error::ArchiveFormatUnknown {
            path: Utf8PathBuf::from("/assets/vendor.rar"),
        },
        Error::ArchiveDecode {
            path: Utf8PathBuf::from("/assets/vendor.tar.gz"),
            format: crate::ArchiveFormat::TarGz,
            source: std::io::Error::other("invalid gzip header"),
        },
        Error::ArchiveMemberNameNotUtf8 {
            path: Utf8PathBuf::from("/assets/vendor.tar"),
            name: "lib/tool\u{fffd}".to_owned(),
        },
        Error::ArchiveMemberTargetNotUtf8 {
            path: Utf8PathBuf::from("/assets/vendor.tar"),
            member: Utf8PathBuf::from("current"),
            target: "releases/\u{fffd}".to_owned(),
        },
        Error::ArchiveMemberKind {
            path: Utf8PathBuf::from("/assets/vendor.tar"),
            member: Utf8PathBuf::from("lib/alias"),
        },
        Error::ArchiveMemberKindDisagrees {
            path: Utf8PathBuf::from("/assets/vendor.zip"),
            member: Utf8PathBuf::from("logs/"),
        },
        Error::ArchiveMemberDuplicate {
            path: Utf8PathBuf::from("/assets/vendor.zip"),
            member: Utf8PathBuf::from("lib/tool"),
        },
        Error::ArchiveMemberStripped {
            path: Utf8PathBuf::from("/assets/vendor.tar.gz"),
            member: Utf8PathBuf::from("README"),
            strip: 1,
        },
        Error::ArchiveMemberTooDeep {
            path: Utf8PathBuf::from("/assets/vendor.tar"),
            member: Utf8PathBuf::from("a/b/c"),
            limit: 64,
        },
        Error::ArchiveTooLarge {
            path: Utf8PathBuf::from("/assets/vendor.tar.gz"),
            limit: 67_108_864,
        },
        Error::ArchiveTooManyMembers {
            path: Utf8PathBuf::from("/assets/vendor.zip"),
            limit: 50_000,
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
        Error::FilesNodeKind {
            path: Utf8PathBuf::from("/srv/skeleton"),
        },
        Error::FilesDuplicate {
            first: Utf8PathBuf::from("/etc/motd"),
            second: Utf8PathBuf::from("/var/motd"),
        },
        Error::StripOnDirectory {
            path: Utf8PathBuf::from("/srv/skeleton"),
        },
    ]);
    every
}

// The CLI's 0/1/2 exit contract, as one match over `is_refusal`.
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

    let (refusals, failures) = (8, 30);
    assert_eq!(codes.len(), refusals + failures);
    assert!(codes[..refusals].iter().all(|&code| code == 2));
    assert!(codes[refusals..].iter().all(|&code| code == 1));
    assert_eq!(exit_code(Ok(())), 0);
}

// A source tree carrying something a desired tree cannot express fails the
// load rather than declining a destination path, so these are exit-1
// failures — and each names where in the source the trouble sits. The
// undecodable pieces are quoted so their edges show — a name is otherwise
// free to start or end in a space, or to render as nothing at all. Quoting
// does not recover what the lossy decode dropped: a replacement character
// stands for bytes with no UTF-8 spelling and for itself alike.
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

// An archive carrying something a desired tree cannot express fails the
// load the same way a source tree does — exit 1, and the message names both
// the archive and the member, since a member path alone says nothing about
// which archive to open.
#[test]
fn archive_messages_name_the_archive_and_the_member_and_exit_1() {
    let unknown = Error::ArchiveFormatUnknown {
        path: Utf8PathBuf::from("/assets/vendor.rar"),
    };
    assert!(!unknown.is_refusal());
    assert_eq!(
        unknown.to_string(),
        "archive /assets/vendor.rar: no decoder for this name; \
         expected one of .tar, .tar.gz, .tgz, .tar.zst, .zip"
    );

    let decode = Error::ArchiveDecode {
        path: Utf8PathBuf::from("/assets/vendor.tar.gz"),
        format: crate::ArchiveFormat::TarGz,
        source: std::io::Error::other("invalid gzip header"),
    };
    assert!(!decode.is_refusal());
    assert_eq!(
        decode.to_string(),
        "archive /assets/vendor.tar.gz does not decode as a gzip-compressed \
         tar archive: invalid gzip header"
    );

    let kind = Error::ArchiveMemberKind {
        path: Utf8PathBuf::from("/assets/vendor.tar"),
        member: Utf8PathBuf::from("lib/alias"),
    };
    assert!(!kind.is_refusal());
    assert_eq!(
        kind.to_string(),
        "archive /assets/vendor.tar: member lib/alias is not a file, \
         directory, or symlink"
    );

    let disagrees = Error::ArchiveMemberKindDisagrees {
        path: Utf8PathBuf::from("/assets/vendor.zip"),
        member: Utf8PathBuf::from("logs/"),
    };
    assert!(!disagrees.is_refusal());
    assert_eq!(
        disagrees.to_string(),
        "archive /assets/vendor.zip: member logs/ is one kind by name and another by mode"
    );

    let duplicate = Error::ArchiveMemberDuplicate {
        path: Utf8PathBuf::from("/assets/vendor.zip"),
        member: Utf8PathBuf::from("lib/tool"),
    };
    assert!(!duplicate.is_refusal());
    assert_eq!(
        duplicate.to_string(),
        "archive /assets/vendor.zip: more than one member projects to lib/tool"
    );

    let large = Error::ArchiveTooLarge {
        path: Utf8PathBuf::from("/assets/vendor.tar.gz"),
        limit: 67_108_864,
    };
    assert!(!large.is_refusal());
    assert_eq!(
        large.to_string(),
        "archive /assets/vendor.tar.gz expands past the 67108864 bytes an \
         archive may allocate"
    );
}

// The destination's depth error is the source tree's bound applied to the
// other tree, and it says so: same limit, different tree, and a path spelled
// relative to the destination rather than absolutely. Both are exit-1
// failures — nothing is being declined, the walk cannot be taken at all.
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

// The paths a caller hands the library are absolutized before anything
// opens them, and the three ways that can fail are exit-1 failures naming
// what could not be resolved.
#[test]
fn path_resolution_failures_name_the_path_and_exit_1() {
    let cwd = Error::CurrentDirectory {
        source: std::io::Error::other("no such directory"),
    };
    assert!(!cwd.is_refusal());
    assert_eq!(
        cwd.to_string(),
        "the current directory cannot be read: no such directory"
    );

    let not_utf8 = Error::PathNotUtf8 {
        path: "/srv/si\u{fffd}te".to_owned(),
    };
    assert!(!not_utf8.is_refusal());
    assert_eq!(
        not_utf8.to_string(),
        "path is not UTF-8: \"/srv/si\u{fffd}te\""
    );

    let state = Error::StateDirIsTarget {
        path: Utf8PathBuf::from("/srv/site"),
    };
    assert!(!state.is_refusal());
    assert_eq!(
        state.to_string(),
        "state directory /srv/site is the target directory: the projection's \
         own state files would classify as foreign"
    );
    assert_eq!(
        value(state),
        serde_json::json!({ "kind": "state_dir_is_target", "path": "/srv/site" })
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

// The single-writer lock's contention variant is exit-1 territory: not a
// refusal, and its message names the state-dir-relative lock path.
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

fn value(error: Error) -> serde_json::Value {
    serde_json::to_value(&error).expect("serializes")
}

#[test]
fn each_variant_family_serializes_as_a_map_tagged_by_its_kind() {
    assert_eq!(
        value(
            Refused::one(
                Utf8PathBuf::from("bin/tool"),
                Refusal::Drift,
                Origin::Caller
            )
            .into()
        ),
        serde_json::json!({
            "kind": "refused",
            "paths": { "bin/tool": { "refusal": "Drift", "origin": "Caller" } },
        })
    );

    assert_eq!(
        value(Error::Io {
            path: Utf8PathBuf::from("config/settings.toml"),
            source: std::io::Error::other("disk full"),
        }),
        serde_json::json!({
            "kind": "io",
            "path": "config/settings.toml",
            "source": "disk full",
        })
    );

    let mapping = value(Error::MappingFormat {
        path: Utf8PathBuf::from("deploy.toml"),
        source: toml::from_str::<toml::Table>("not toml").expect_err("parse failure"),
    });
    assert_eq!(mapping["kind"], "mapping_format");
    assert_eq!(mapping["path"], "deploy.toml");
    assert!(
        mapping["source"]
            .as_str()
            .expect("a string")
            .contains("key with no value")
    );

    assert_eq!(
        value(Error::ArchiveDecode {
            path: Utf8PathBuf::from("/assets/vendor.tar.gz"),
            format: crate::ArchiveFormat::TarGz,
            source: std::io::Error::other("invalid gzip header"),
        }),
        serde_json::json!({
            "kind": "archive_decode",
            "path": "/assets/vendor.tar.gz",
            "format": "TarGz",
            "source": "invalid gzip header",
        })
    );

    assert_eq!(
        value(Error::TreeTooDeep {
            path: Utf8PathBuf::from("/srv/skeleton/a/b/c"),
            limit: crate::MAX_WALK_DEPTH,
        }),
        serde_json::json!({
            "kind": "tree_too_deep",
            "path": "/srv/skeleton/a/b/c",
            "limit": 64,
        })
    );
}

#[test]
fn every_variant_serializes_under_a_kind_of_its_own() {
    let kinds: Vec<String> = every_variant()
        .into_iter()
        .map(|error| {
            value(error)["kind"]
                .as_str()
                .expect("a kind tag")
                .to_owned()
        })
        .collect();

    let (refusals, failures) = (8, 30);
    assert_eq!(kinds.len(), refusals + failures);
    assert!(kinds[..refusals].iter().all(|kind| kind == "refused"));
    assert_eq!(
        kinds.into_iter().collect::<BTreeSet<String>>().len(),
        1 + failures
    );
}
