use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::fs_utf8::Dir;

use super::*;
use crate::test_support::{Fixture, Tree, assert_tree};
use crate::{EntryKind, ManifestEntry};

/// Opens the destination handle observe takes, rooted at the fixture.
/// Ambient authority is the test's to spend; the library itself never
/// opens ambient paths.
fn dest(fixture: &Fixture) -> Dir {
    Dir::open_ambient_dir(fixture.root(), cap_std::ambient_authority())
        .expect("open fixture root as a Dir")
}

/// A manifest recording the given `(path, kind, hash)` rows under one owner.
fn manifest_of(rows: &[(&str, EntryKind, String)]) -> Manifest {
    let mut manifest = Manifest::new();
    for (path, kind, hash) in rows {
        manifest.entries.insert(
            Utf8PathBuf::from(*path),
            ManifestEntry {
                kind: *kind,
                hash: hash.clone(),
                executable: false,
                owners: BTreeSet::from(["test".to_owned()]),
            },
        );
    }
    manifest
}

fn observed(fixture: &Fixture, manifest: &Manifest) -> BTreeMap<Utf8PathBuf, Observation> {
    observe(&dest(fixture), manifest)
        .expect("observe succeeds")
        .paths
}

#[test]
fn sha256_hex_matches_the_published_test_vectors() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn observes_the_union_of_disk_and_manifest() {
    let tree = Tree::new()
        .file("notes/a.txt", "alpha")
        .executable("bin/run", "#!/bin/sh\nexit 0\n")
        .symlink("latest", "notes/a.txt")
        .dir("empty");
    let fixture = tree.materialize();
    // `notes/a.txt` is recorded and on disk; `gone.txt` is recorded and not.
    let manifest = manifest_of(&[
        ("notes/a.txt", EntryKind::File, sha256_hex(b"alpha")),
        ("gone.txt", EntryKind::File, sha256_hex(b"bye")),
    ]);

    let paths = observed(&fixture, &manifest);

    let expected: BTreeMap<Utf8PathBuf, Observation> = [
        ("bin".into(), Observation::Directory),
        (
            "bin/run".into(),
            Observation::File {
                hash: sha256_hex(b"#!/bin/sh\nexit 0\n"),
                executable: true,
            },
        ),
        ("empty".into(), Observation::Directory),
        ("gone.txt".into(), Observation::Absent),
        (
            "latest".into(),
            Observation::Symlink {
                hash: sha256_hex(b"notes/a.txt"),
                target: Some("notes/a.txt".to_owned()),
            },
        ),
        ("notes".into(), Observation::Directory),
        (
            "notes/a.txt".into(),
            Observation::File {
                hash: sha256_hex(b"alpha"),
                executable: false,
            },
        ),
    ]
    .into();
    assert_eq!(paths, expected);
}

#[test]
fn drifted_file_hashes_as_the_disk_bytes_not_the_recorded_ones() {
    let fixture = Tree::new()
        .file("conf.toml", "edited = true\n")
        .materialize();
    let recorded = sha256_hex(b"edited = false\n");
    let manifest = manifest_of(&[("conf.toml", EntryKind::File, recorded.clone())]);

    let paths = observed(&fixture, &manifest);

    let Observation::File { hash, .. } = &paths[Utf8Path::new("conf.toml")] else {
        panic!("conf.toml observes as a file");
    };
    assert_eq!(*hash, sha256_hex(b"edited = true\n"));
    assert_ne!(*hash, recorded);
}

#[test]
fn a_file_spanning_many_hasher_chunks_hashes_as_its_whole_contents() {
    // 1 MiB of patterned bytes: far past any single read of the streaming
    // hash, so the chunk-boundary accumulation is what this exercises.
    let contents: Vec<u8> = (0..1_048_576u32).map(|i| (i % 251) as u8).collect();
    let fixture = Tree::new().materialize();
    fs::write(fixture.path("big.bin").as_std_path(), &contents).expect("write big file");

    let paths = observed(&fixture, &Manifest::new());

    assert_eq!(
        paths.get(Utf8Path::new("big.bin")),
        Some(&Observation::File {
            hash: sha256_hex(&contents),
            executable: false,
        })
    );
}

#[test]
fn foreign_file_is_surfaced() {
    let fixture = Tree::new().file("stray.txt", "not ours").materialize();

    let paths = observed(&fixture, &Manifest::new());

    assert_eq!(
        paths.get(Utf8Path::new("stray.txt")),
        Some(&Observation::File {
            hash: sha256_hex(b"not ours"),
            executable: false,
        })
    );
}

#[test]
fn dangling_link_observes_with_its_target_verbatim() {
    let fixture = Tree::new()
        .symlink("dangling", "missing/target")
        .materialize();

    let paths = observed(&fixture, &Manifest::new());

    let expected: BTreeMap<Utf8PathBuf, Observation> = [(
        "dangling".into(),
        Observation::Symlink {
            hash: sha256_hex(b"missing/target"),
            target: Some("missing/target".to_owned()),
        },
    )]
    .into();
    assert_eq!(paths, expected);
}

#[test]
fn symlinked_directory_is_never_entered() {
    let fixture = Tree::new()
        .file("real/inner.txt", "inner")
        .symlink("alias", "real")
        .materialize();

    let paths = observed(&fixture, &Manifest::new());

    // `alias` itself is observed; nothing beneath it is.
    assert!(matches!(
        paths[Utf8Path::new("alias")],
        Observation::Symlink { .. }
    ));
    assert!(!paths.contains_key(Utf8Path::new("alias/inner.txt")));
    assert!(paths.contains_key(Utf8Path::new("real/inner.txt")));
}

#[test]
fn recorded_path_beneath_a_symlinked_ancestor_observes_absent() {
    // Following `logs` would find `real/x.txt` — exactly what the walk must
    // never do: a recorded path is only real if every ancestor is a real
    // directory.
    let fixture = Tree::new()
        .file("real/x.txt", "x")
        .symlink("logs", "real")
        .materialize();
    let manifest = manifest_of(&[("logs/x.txt", EntryKind::File, sha256_hex(b"x"))]);

    let paths = observed(&fixture, &manifest);

    assert_eq!(
        paths.get(Utf8Path::new("logs/x.txt")),
        Some(&Observation::Absent)
    );
    assert!(matches!(
        paths[Utf8Path::new("logs")],
        Observation::Symlink { .. }
    ));
}

#[test]
fn external_target_is_returned_verbatim_and_not_followed() {
    let outside = Tree::new().file("secret.txt", "outside").materialize();
    let fixture = Tree::new()
        .symlink("out", outside.root().as_str())
        .materialize();

    let paths = observed(&fixture, &Manifest::new());

    let expected: BTreeMap<Utf8PathBuf, Observation> = [(
        "out".into(),
        Observation::Symlink {
            hash: sha256_hex(outside.root().as_str().as_bytes()),
            target: Some(outside.root().as_str().to_owned()),
        },
    )]
    .into();
    // The link is a pointer, observed as such — `secret.txt` appears
    // nowhere, under `out/` or otherwise.
    assert_eq!(paths, expected);
}

#[test]
fn non_utf8_entry_name_is_skipped_not_an_error() {
    let fixture = Tree::new().file("named.txt", "fine").materialize();
    let bad_name = fixture
        .root()
        .as_std_path()
        .join(OsStr::from_bytes(b"bad-\xff-name"));
    if fs::write(&bad_name, b"unnameable").is_err() {
        // The filesystem refuses non-UTF-8 names outright (APFS on macOS
        // enforces UTF-8), so the entry this test skips cannot exist here.
        // CI runs on Linux, where it can.
        return;
    }

    let paths = observed(&fixture, &Manifest::new());

    let expected: BTreeMap<Utf8PathBuf, Observation> = [(
        "named.txt".into(),
        Observation::File {
            hash: sha256_hex(b"fine"),
            executable: false,
        },
    )]
    .into();
    assert_eq!(paths, expected);
}

/// Nests `depth` directories under `root` and returns the deepest one's
/// path relative to `root`. `create_dir_all` spells the whole chain in one
/// path, which stays well inside the host's path limit at these depths — the
/// walk itself is bound by no such limit, which is the point of
/// `MAX_WALK_DEPTH`.
fn nest(root: &Utf8Path, depth: usize) -> Utf8PathBuf {
    let rel = Utf8PathBuf::from(vec!["d"; depth].join("/"));
    fs::create_dir_all(root.join(&rel)).expect("nest directories");
    rel
}

#[test]
fn a_destination_at_the_depth_limit_observes_and_one_past_it_is_named() {
    // The walk spends a stack frame per level and the destination picks the
    // depth — foreign content and mount loops included — so a destination
    // nested past the limit has to come back as an error a caller can
    // report, not as a stack the walk runs off the end of.
    let fixture = Tree::new().materialize();
    let deepest = nest(fixture.root(), MAX_WALK_DEPTH);
    fs::write(fixture.root().join(&deepest).join("marker"), "deep").expect("write a deep file");

    let paths = observed(&fixture, &Manifest::new());
    assert_eq!(
        paths.get(&deepest.join("marker")),
        Some(&Observation::File {
            hash: sha256_hex(b"deep"),
            executable: false,
        })
    );

    let past = nest(fixture.root(), MAX_WALK_DEPTH + 1);
    assert!(matches!(
        observe(&dest(&fixture), &Manifest::new()).unwrap_err(),
        Error::DestinationTooDeep { path, limit } if path == past && limit == MAX_WALK_DEPTH
    ));
}

#[test]
fn non_utf8_link_target_observes_as_hash_only() {
    let fixture = Tree::new().materialize();
    let target_bytes: &[u8] = b"tar-\xff-get";
    std::os::unix::fs::symlink(
        OsStr::from_bytes(target_bytes),
        fixture.path("weird").as_std_path(),
    )
    .expect("create symlink with non-UTF-8 target");

    let paths = observed(&fixture, &Manifest::new());

    // The hash covers the raw bytes, which no recorded UTF-8 target string
    // can hash to — so a recorded link edited this way compares as drifted.
    let expected: BTreeMap<Utf8PathBuf, Observation> = [(
        "weird".into(),
        Observation::Symlink {
            hash: sha256_hex(target_bytes),
            target: None,
        },
    )]
    .into();
    assert_eq!(paths, expected);
}

#[test]
fn fifo_observes_as_other_and_is_never_opened() {
    let fixture = Tree::new().materialize();
    let status = std::process::Command::new("mkfifo")
        .arg(fixture.path("pipe").as_str())
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo failed");

    // Opening a writerless FIFO for read blocks forever, so this returning
    // at all is the "never opened" assertion.
    let paths = observed(&fixture, &Manifest::new());

    let expected: BTreeMap<Utf8PathBuf, Observation> = [("pipe".into(), Observation::Other)].into();
    assert_eq!(paths, expected);
}

#[test]
fn observe_writes_nothing() {
    let tree = Tree::new()
        .file("notes/a.txt", "alpha")
        .executable("bin/run", "#!/bin/sh\nexit 0\n")
        .symlink("latest", "notes/a.txt")
        .dir("empty");
    let fixture = tree.materialize();

    observed(
        &fixture,
        &manifest_of(&[("gone.txt", EntryKind::File, sha256_hex(b"bye"))]),
    );

    // The zero-writes discipline: after a full observation the tree is
    // byte-for-byte what was declared — nothing created, changed, or removed.
    assert_tree(fixture.root(), &tree);
}
