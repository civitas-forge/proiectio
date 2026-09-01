use std::collections::{BTreeMap, BTreeSet};

use camino::Utf8PathBuf;

use super::*;
use crate::{BlockFault, IoRole, Limits, Manifest, Origin, Refusal, RefusalKind, Refused};

fn every_variant() -> Vec<Error> {
    let refusal = |kind: RefusalKind| -> Error {
        let refusal = match kind {
            RefusalKind::Drift => Refusal::Drift,
            RefusalKind::DirectoryInTheWay => Refusal::DirectoryInTheWay {
                holding: BTreeMap::from([(Utf8PathBuf::from("bin/tool/note.md"), BTreeSet::new())]),
                unreadable: BTreeSet::new(),
            },
            RefusalKind::Foreign => Refusal::Foreign,
            RefusalKind::Containment => Refusal::Containment { through: None },
            RefusalKind::TreeConflict => Refusal::TreeConflict {
                paths: BTreeSet::new(),
            },
            RefusalKind::RecordedLanding => Refusal::RecordedLanding {
                through: Utf8PathBuf::from("bin"),
                at: Utf8PathBuf::from("real/tool"),
                owners: BTreeSet::from(["other".to_owned()]),
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
        RefusalKind::RecordedLanding,
        RefusalKind::Foreign,
        RefusalKind::Drift,
        RefusalKind::DirectoryInTheWay,
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
            role: IoRole::Destination,
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
        Error::ManifestPathPruned {
            path: Utf8PathBuf::from("vendor/.git/config"),
        },
        Error::LockHeld {
            path: Utf8PathBuf::from("/srv/site/.proiectio/proiectio.lock"),
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
        Error::InvalidPrunedComponent {
            component: "not/one".to_owned(),
        },
        Error::MappingFormat {
            path: Utf8PathBuf::from("deploy.toml"),
            source: toml::from_str::<toml::Table>("not toml").expect_err("parse failure"),
        },
        Error::MappingIsDirectory {
            path: Utf8PathBuf::from("/srv/skeleton"),
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
        Error::ArchiveFullyStripped {
            path: Utf8PathBuf::from("/assets/vendor.tar.gz"),
            strip: 3,
            dropped: 4,
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
        Error::ArchiveFileTooLarge {
            path: Utf8PathBuf::from("/assets/vendor.zip"),
            size: 70_000_000,
            remaining: 67_108_864,
            limit: 67_108_864,
        },
        Error::SourceTooLarge {
            path: Utf8PathBuf::from("/assets/blob.bin"),
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
        Error::OwnerNotNamed {
            owner: "  ".to_owned(),
        },
    ]);
    every
}

// Every kind tag [`Error`] can serialize under, spelled out rather than
// counted off the list — a count taken from the list can only agree with
// itself. The refusals all serialize as `refused`, so this is one tag
// shorter than the list is long.
const EVERY_KIND: [&str; 37] = [
    "refused",
    "io",
    "manifest_format",
    "manifest_version",
    "manifest_path_pruned",
    "lock_held",
    "current_directory",
    "path_not_utf8",
    "state_dir_is_target",
    "invalid_pruned_component",
    "mapping_format",
    "mapping_is_directory",
    "mapping_version",
    "mapping_contents_xor_source",
    "mapping_duplicate",
    "archive_format_unknown",
    "archive_decode",
    "archive_member_name_not_utf8",
    "archive_member_target_not_utf8",
    "archive_member_kind",
    "archive_member_kind_disagrees",
    "archive_member_duplicate",
    "archive_fully_stripped",
    "archive_member_too_deep",
    "archive_too_large",
    "archive_file_too_large",
    "source_too_large",
    "archive_too_many_members",
    "tree_name_not_utf8",
    "tree_target_not_utf8",
    "tree_node_kind",
    "tree_too_deep",
    "destination_too_deep",
    "files_node_kind",
    "files_duplicate",
    "strip_on_directory",
    "owner_not_named",
];

// A variant added to `Error` stops this compiling until it is named here,
// beside `EVERY_KIND`.
fn is_named_above(error: &Error) -> bool {
    match error {
        Error::Refused(_)
        | Error::Io { .. }
        | Error::ManifestFormat { .. }
        | Error::ManifestVersion { .. }
        | Error::ManifestPathPruned { .. }
        | Error::LockHeld { .. }
        | Error::CurrentDirectory { .. }
        | Error::PathNotUtf8 { .. }
        | Error::StateDirIsTarget { .. }
        | Error::InvalidPrunedComponent { .. }
        | Error::MappingFormat { .. }
        | Error::MappingIsDirectory { .. }
        | Error::MappingVersion { .. }
        | Error::MappingContentsXorSource { .. }
        | Error::MappingDuplicate { .. }
        | Error::ArchiveFormatUnknown { .. }
        | Error::ArchiveDecode { .. }
        | Error::ArchiveMemberNameNotUtf8 { .. }
        | Error::ArchiveMemberTargetNotUtf8 { .. }
        | Error::ArchiveMemberKind { .. }
        | Error::ArchiveMemberKindDisagrees { .. }
        | Error::ArchiveMemberDuplicate { .. }
        | Error::ArchiveFullyStripped { .. }
        | Error::ArchiveMemberTooDeep { .. }
        | Error::ArchiveTooLarge { .. }
        | Error::ArchiveFileTooLarge { .. }
        | Error::SourceTooLarge { .. }
        | Error::ArchiveTooManyMembers { .. }
        | Error::TreeNameNotUtf8 { .. }
        | Error::TreeTargetNotUtf8 { .. }
        | Error::TreeNodeKind { .. }
        | Error::TreeTooDeep { .. }
        | Error::DestinationTooDeep { .. }
        | Error::FilesNodeKind { .. }
        | Error::FilesDuplicate { .. }
        | Error::StripOnDirectory { .. }
        | Error::OwnerNotNamed { .. } => true,
    }
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

    let (refusals, failures) = (10, EVERY_KIND.len() - 1);
    assert_eq!(codes.len(), refusals + failures);
    assert!(codes[..refusals].iter().all(|&code| code == 2));
    assert!(codes[refusals..].iter().all(|&code| code == 1));
    assert_eq!(exit_code(Ok(())), 0);
}

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
        "archive /assets/vendor.tar.gz expands past the 67108864 bytes one \
         load may hold in memory"
    );

    let on_disk = Error::ArchiveFileTooLarge {
        path: Utf8PathBuf::from("/assets/vendor.zip"),
        size: 70_000_000,
        remaining: 67_108_864,
        limit: 67_108_864,
    };
    assert!(!on_disk.is_refusal());
    assert_eq!(
        on_disk.to_string(),
        "archive /assets/vendor.zip is 70000000 bytes on disk, and a zip's index \
         is read whole before any member, so the file itself has to fit: 67108864 \
         bytes are left of the 67108864 bytes one load may hold in memory"
    );

    let source = Error::SourceTooLarge {
        path: Utf8PathBuf::from("/assets/blob.bin"),
        limit: Limits::DEFAULT_MAX_SOURCE_BYTES,
    };
    assert!(!source.is_refusal());
    assert_eq!(
        source.to_string(),
        "source /assets/blob.bin reads past the 524288000 bytes one load may \
         hold in memory"
    );

    let stripped = Error::ArchiveFullyStripped {
        path: Utf8PathBuf::from("/assets/vendor.tar.gz"),
        strip: 3,
        dropped: 4,
    };
    assert!(!stripped.is_refusal());
    assert_eq!(
        stripped.to_string(),
        "archive /assets/vendor.tar.gz: strip 3 left nothing to project \
         (4 members dropped)"
    );
}

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
        role: IoRole::Unstated,
        path: Utf8PathBuf::from("bin/tool"),
        source: std::io::Error::other("disk full"),
    };
    assert_eq!(error.to_string(), "bin/tool: disk full");
}

// Matched over `IoRole` itself, so a role added there stops this compiling
// until it has a word.
#[test]
fn every_role_opens_the_message_with_the_word_that_places_the_path() {
    for role in [
        IoRole::Destination,
        IoRole::StateDirectory,
        IoRole::Mapping,
        IoRole::SourceTree,
        IoRole::Archive,
        IoRole::Source,
        IoRole::NamedFile,
        IoRole::Unstated,
    ] {
        let placed = match role {
            IoRole::Destination => "destination /srv/site: not there",
            IoRole::StateDirectory => "state directory /srv/site: not there",
            IoRole::Mapping => "mapping /srv/site: not there",
            IoRole::SourceTree => "source tree /srv/site: not there",
            IoRole::Archive => "archive /srv/site: not there",
            IoRole::Source => "source /srv/site: not there",
            IoRole::NamedFile => "named file /srv/site: not there",
            // The run's own working paths, which its other lines place.
            IoRole::Unstated => "/srv/site: not there",
        };
        let error = Error::Io {
            role,
            path: Utf8PathBuf::from("/srv/site"),
            source: std::io::Error::other("not there"),
        };

        assert_eq!(error.to_string(), placed, "{role:?}");
    }
}

#[test]
fn a_directory_named_as_a_mapping_names_the_option_it_belongs_to() {
    let error = Error::MappingIsDirectory {
        path: Utf8PathBuf::from("/srv/skeleton"),
    };

    assert_eq!(
        error.to_string(),
        "mapping /srv/skeleton is a directory: a mapping is a TOML file; \
         pass a directory as --tree to project the tree it holds"
    );
}

#[test]
fn lock_held_exits_1_and_names_the_lock_path() {
    let lock = Utf8PathBuf::from("/srv/site/.proiectio/proiectio.lock");
    let error = Error::LockHeld { path: lock.clone() };
    assert!(!error.is_refusal());
    assert_eq!(exit_code(Err(error)), 1);
    let error = Error::LockHeld { path: lock };
    assert_eq!(
        error.to_string(),
        "state lock /srv/site/.proiectio/proiectio.lock is held by another writer"
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
            role: IoRole::Destination,
            path: Utf8PathBuf::from("config/settings.toml"),
            source: std::io::Error::other("disk full"),
        }),
        serde_json::json!({
            "kind": "io",
            "role": "destination",
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

    let refusals = 10;
    assert!(kinds[..refusals].iter().all(|kind| kind == "refused"));

    // A variant nobody listed is named here rather than passing unnoticed.
    let found: BTreeSet<&str> = kinds.iter().map(String::as_str).collect();
    let declared: BTreeSet<&str> = EVERY_KIND.into_iter().collect();
    assert_eq!(
        declared.difference(&found).collect::<Vec<_>>(),
        Vec::<&&str>::new(),
        "every_variant is missing a variant of Error"
    );
    assert_eq!(
        found.difference(&declared).collect::<Vec<_>>(),
        Vec::<&&str>::new(),
        "EVERY_KIND names a kind Error does not serialize under"
    );
    // One entry per variant, so nothing is listed twice either.
    assert_eq!(kinds.len(), refusals + EVERY_KIND.len() - 1);
}

// This match refuses to compile until every variant of `Error` is named beside it.
#[test]
fn every_variant_of_the_enum_is_one_the_tests_name() {
    assert!(every_variant().iter().all(is_named_above));
}
