use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::fs_utf8::Dir;

use super::*;
use crate::test_support::{Fixture, Tree, assert_tree, plant};
use crate::{BlockMarkers, EntryKind, ManifestEntry, Placement};

// Ambient authority is the test's to spend; the library never opens ambient paths.
fn dest(fixture: &Fixture) -> Dir {
    Dir::open_ambient_dir(fixture.root(), cap_std::ambient_authority())
        .expect("open fixture root as a Dir")
}

fn manifest_of(rows: &[(&str, EntryKind, String)]) -> Manifest {
    let mut manifest = Manifest::new();
    for (path, kind, hash) in rows {
        manifest.entries.insert(
            Utf8PathBuf::from(*path),
            ManifestEntry {
                kind: kind.clone(),
                hash: hash.clone(),
                executable: false,
                owners: BTreeSet::from(["test".to_owned()]),
            },
        );
    }
    manifest
}

fn observed(fixture: &Fixture, manifest: &Manifest) -> BTreeMap<Utf8PathBuf, Observation> {
    observed_wanting(fixture, manifest, &BlockMarkers::new())
}

fn observed_wanting(
    fixture: &Fixture,
    manifest: &Manifest,
    wanted: &BlockMarkers,
) -> BTreeMap<Utf8PathBuf, Observation> {
    observe(&dest(fixture), manifest, wanted)
        .expect("observe succeeds")
        .paths
}

fn observed_pruning(
    fixture: &Fixture,
    pruned_components: &[&str],
) -> BTreeMap<Utf8PathBuf, Observation> {
    let pruned_components = pruned_components
        .iter()
        .map(|component| (*component).to_owned())
        .collect();
    observe_scoped(
        &dest(fixture),
        &Manifest::new(),
        &BlockMarkers::new(),
        &pruned_components,
    )
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

    assert!(matches!(
        paths[Utf8Path::new("alias")],
        Observation::Symlink { .. }
    ));
    assert!(!paths.contains_key(Utf8Path::new("alias/inner.txt")));
    assert!(paths.contains_key(Utf8Path::new("real/inner.txt")));
}

#[test]
fn a_pruned_component_is_not_observed_at_any_depth() {
    let fixture = Tree::new()
        .file(".git/config", "root metadata")
        .file("vendor/project/.git/config", "nested metadata")
        .file("vendor/project/src/lib.rs", "pub fn live() {}")
        .file(".github/workflows/ci.yml", "jobs: {}")
        .materialize();

    let paths = observed_pruning(&fixture, &[".git"]);

    assert!(
        paths
            .keys()
            .all(|path| !path.components().any(|c| c.as_str() == ".git"))
    );
    assert!(paths.contains_key(Utf8Path::new("vendor/project/src/lib.rs")));
    assert!(paths.contains_key(Utf8Path::new(".github/workflows/ci.yml")));
}

#[test]
fn directories_containing_pruned_children_are_known_to_be_incomplete() {
    let fixture = Tree::new()
        .file(".git/config", "root metadata")
        .file("cache/.git/config", "nested metadata")
        .materialize();
    let pruned = BTreeSet::from([".git".to_owned()]);

    let observations = observe_scoped(
        &dest(&fixture),
        &Manifest::new(),
        &BlockMarkers::new(),
        &pruned,
    )
    .expect("observe succeeds");

    assert_eq!(
        observations.unobserved,
        BTreeSet::from([Utf8PathBuf::new(), Utf8PathBuf::from("cache")])
    );
    assert!(
        observations
            .paths
            .keys()
            .all(|path| !path.ends_with(".git"))
    );
}

#[test]
fn recorded_path_beneath_a_symlinked_ancestor_observes_absent() {
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
    assert_eq!(
        paths.get(Utf8Path::new("logs")),
        Some(&Observation::Symlink {
            hash: sha256_hex(b"real"),
            target: Some("real".to_owned()),
        })
    );
}

#[test]
fn every_hop_on_the_way_to_a_recorded_path_is_observed() {
    let fixture = Tree::new()
        .file("real/deep/x.txt", "x")
        .symlink("a/b", "../real/deep")
        .materialize();
    let manifest = manifest_of(&[("a/b/x.txt", EntryKind::File, sha256_hex(b"x"))]);

    let paths = observed(&fixture, &manifest);

    assert_eq!(paths[Utf8Path::new("a")], Observation::Directory);
    assert_eq!(
        paths[Utf8Path::new("a/b")],
        Observation::Symlink {
            hash: sha256_hex(b"../real/deep"),
            target: Some("../real/deep".to_owned()),
        }
    );
    assert_eq!(
        paths.get(Utf8Path::new("a/b/x.txt")),
        Some(&Observation::Absent)
    );
    assert_eq!(paths[Utf8Path::new("real/deep")], Observation::Directory);
    assert!(matches!(
        paths[Utf8Path::new("real/deep/x.txt")],
        Observation::File { .. }
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
    assert_eq!(paths, expected);
}

#[test]
fn non_utf8_entry_name_is_skipped_and_names_its_directory_unreadable() {
    let fixture = Tree::new()
        .file("named.txt", "fine")
        .file("scaffolding/kept.txt", "kept")
        .materialize();
    let bad_name = fixture
        .root()
        .as_std_path()
        .join("scaffolding")
        .join(OsStr::from_bytes(b"bad-\xff-name"));
    if !plant(&bad_name) {
        return;
    }

    let observations =
        observe(&dest(&fixture), &Manifest::new(), &BlockMarkers::new()).expect("observe succeeds");

    let expected: BTreeMap<Utf8PathBuf, Observation> = [
        (
            "named.txt".into(),
            Observation::File {
                hash: sha256_hex(b"fine"),
                executable: false,
            },
        ),
        ("scaffolding".into(), Observation::Directory),
        (
            "scaffolding/kept.txt".into(),
            Observation::File {
                hash: sha256_hex(b"kept"),
                executable: false,
            },
        ),
    ]
    .into();
    assert_eq!(observations.paths, expected);
    assert_eq!(
        observations.unreadable,
        BTreeSet::from([Utf8PathBuf::from("scaffolding")])
    );
}

// Nests `depth` directories and returns the deepest one's relative path;
// the chain stays inside the host's path limit at these depths.
fn nest(root: &Utf8Path, depth: usize) -> Utf8PathBuf {
    let rel = Utf8PathBuf::from(vec!["d"; depth].join("/"));
    fs::create_dir_all(root.join(&rel)).expect("nest directories");
    rel
}

#[test]
fn a_destination_at_the_depth_limit_observes_and_one_past_it_is_named() {
    // The walk spends a stack frame per level; past the limit it must error, not overflow.
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
        observe(&dest(&fixture), &Manifest::new(), &BlockMarkers::new()).unwrap_err(),
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

    assert_tree(fixture.root(), &tree);
}

// --- blocks: observing the region, not the container ---

// A manifest recording a region at `path` under `marker` and `placement`,
// whose body hashes to `body`.
fn block_manifest(path: &str, marker: &str, placement: Placement, body: &str) -> Manifest {
    manifest_of(&[(
        path,
        EntryKind::Block {
            marker: marker.to_owned(),
            placement,
        },
        sha256_hex(body.as_bytes()),
    )])
}

#[test]
fn a_recorded_block_observes_its_region_and_not_the_container() {
    let fixture = Tree::new()
        .file("rc", "author\n# proiectio\nmanaged\n")
        .materialize();
    let manifest = block_manifest("rc", "# proiectio", Placement::Append, "managed\n");

    let observations = observed(&fixture, &manifest);

    assert_eq!(
        observations[Utf8Path::new("rc")],
        Observation::Block {
            hash: Some(sha256_hex(b"managed\n")),
            // The author's side ends at the marker's line start.
            newline_terminated: true,
            occurrences: 1,
            desired: None,
        }
    );
}

#[test]
fn an_edit_outside_the_region_leaves_its_hash_alone() {
    let before = Tree::new()
        .file("rc", "author\n# proiectio\nmanaged\n")
        .materialize();
    let after = Tree::new()
        .file("rc", "author\nand a new line\n# proiectio\nmanaged\n")
        .materialize();
    let manifest = block_manifest("rc", "# proiectio", Placement::Append, "managed\n");

    assert_eq!(
        observed(&before, &manifest)[Utf8Path::new("rc")],
        observed(&after, &manifest)[Utf8Path::new("rc")]
    );
}

#[test]
fn a_container_with_no_marker_line_observes_no_region() {
    let fixture = Tree::new().file("rc", "author only").materialize();
    let manifest = block_manifest("rc", "# proiectio", Placement::Append, "managed\n");

    let observations = observed(&fixture, &manifest);

    assert_eq!(
        observations[Utf8Path::new("rc")],
        Observation::Block {
            hash: None,
            // No region, so the author's side is the whole file — and this
            // one has no final newline to append after.
            newline_terminated: false,
            occurrences: 0,
            desired: None,
        }
    );
}

#[test]
fn newline_termination_is_about_the_author_side_alone() {
    let terminated = Tree::new()
        .file("rc", "managed\n# proiectio\nauthor\n")
        .materialize();
    let bare = Tree::new()
        .file("rc", "managed\n# proiectio\nauthor")
        .materialize();
    let manifest = block_manifest("rc", "# proiectio", Placement::Prepend, "managed\n");

    for (fixture, want) in [(&terminated, true), (&bare, false)] {
        assert_eq!(
            observed(fixture, &manifest)[Utf8Path::new("rc")],
            Observation::Block {
                hash: Some(sha256_hex(b"managed\n")),
                newline_terminated: want,
                occurrences: 1,
                desired: None,
            }
        );
    }
}

#[test]
fn an_unrecorded_container_observes_as_an_ordinary_file() {
    let contents = "author\n# proiectio\nmanaged\n";
    let fixture = Tree::new().file("rc", contents).materialize();

    let observations = observed(&fixture, &Manifest::new());

    assert_eq!(
        observations[Utf8Path::new("rc")],
        Observation::File {
            hash: sha256_hex(contents.as_bytes()),
            executable: false,
        }
    );
}

#[test]
fn a_container_swapped_for_another_kind_observes_as_that_kind() {
    let manifest = block_manifest("rc", "# proiectio", Placement::Append, "managed\n");
    let as_link = Tree::new()
        .file("real", "x")
        .symlink("rc", "real")
        .materialize();
    let as_dir = Tree::new().dir("rc").materialize();

    assert_eq!(
        observed(&as_link, &manifest)[Utf8Path::new("rc")],
        Observation::Symlink {
            hash: sha256_hex(b"real"),
            target: Some("real".to_owned()),
        }
    );
    assert_eq!(
        observed(&as_dir, &manifest)[Utf8Path::new("rc")],
        Observation::Directory
    );
}

#[test]
fn a_region_reached_through_a_recorded_link_is_stated_under_its_own_key() {
    let contents = "author\n# proiectio\nmanaged\n";
    let fixture = Tree::new()
        .file("real/rc", contents)
        .symlink("logs", "real")
        .materialize();
    let mut manifest = block_manifest("logs/rc", "# proiectio", Placement::Append, "managed\n");
    manifest.entries.insert(
        Utf8PathBuf::from("logs"),
        ManifestEntry {
            kind: EntryKind::Symlink,
            hash: sha256_hex(b"real"),
            executable: false,
            owners: BTreeSet::from(["test".to_owned()]),
        },
    );

    let observations = observed(&fixture, &manifest);

    assert_eq!(
        observations[Utf8Path::new("logs/rc")],
        Observation::Block {
            hash: Some(sha256_hex(b"managed\n")),
            newline_terminated: true,
            occurrences: 1,
            desired: None,
        }
    );
    assert_eq!(
        observations[Utf8Path::new("real/rc")],
        Observation::File {
            hash: sha256_hex(contents.as_bytes()),
            executable: false,
        }
    );
}

#[test]
fn a_region_beneath_a_hand_made_link_is_left_where_the_walk_read_it() {
    let contents = "author\n# proiectio\nmanaged\n";
    let fixture = Tree::new()
        .file("real/rc", contents)
        .symlink("logs", "real")
        .materialize();
    let manifest = block_manifest("logs/rc", "# proiectio", Placement::Append, "managed\n");

    let observations = observed(&fixture, &manifest);

    assert_eq!(
        observations[Utf8Path::new("real/rc")],
        Observation::File {
            hash: sha256_hex(contents.as_bytes()),
            executable: false,
        }
    );
}

// The recorded link every relocating test walks out through.
fn recorded_link(manifest: &mut Manifest, at: &str, target: &str) {
    manifest.entries.insert(
        Utf8PathBuf::from(at),
        ManifestEntry {
            kind: EntryKind::Symlink,
            hash: sha256_hex(target.as_bytes()),
            executable: false,
            owners: BTreeSet::from(["test".to_owned()]),
        },
    );
}

#[test]
fn a_relocated_region_reads_the_desired_text_under_its_own_key() {
    let contents = "author\n# renamed\n# proiectio\nmanaged\n";
    let fixture = Tree::new()
        .file("real/rc", contents)
        .symlink("logs", "real")
        .materialize();
    let mut manifest = block_manifest("logs/rc", "# proiectio", Placement::Append, "managed\n");
    recorded_link(&mut manifest, "logs", "real");
    let wanted = BlockMarkers::from([(
        Utf8PathBuf::from("logs/rc"),
        ("# renamed".to_owned(), Placement::Append),
    )]);

    let observations = observed_wanting(&fixture, &manifest, &wanted);

    assert_eq!(
        observations[Utf8Path::new("logs/rc")],
        Observation::Block {
            hash: Some(sha256_hex(b"managed\n")),
            newline_terminated: true,
            occurrences: 1,
            desired: Some(DesiredRegion {
                occurrences: 1,
                hash: Some(sha256_hex(b"# proiectio\nmanaged\n")),
                author_newline_terminated: true,
            }),
        }
    );
}

#[test]
fn two_records_reaching_one_container_each_state_their_own_region() {
    let contents = "beta\n# beta\nauthor\n# alpha\nalpha\n";
    let fixture = Tree::new()
        .file("real/rc", contents)
        .symlink("a", "real")
        .symlink("b", "real")
        .materialize();
    let mut manifest = manifest_of(&[
        (
            "a/rc",
            EntryKind::Block {
                marker: "# alpha".to_owned(),
                placement: Placement::Append,
            },
            sha256_hex(b"alpha\n"),
        ),
        (
            "b/rc",
            EntryKind::Block {
                marker: "# beta".to_owned(),
                placement: Placement::Prepend,
            },
            sha256_hex(b"beta\n"),
        ),
    ]);
    recorded_link(&mut manifest, "a", "real");
    recorded_link(&mut manifest, "b", "real");

    let observations = observed(&fixture, &manifest);

    assert_eq!(
        observations[Utf8Path::new("a/rc")],
        Observation::Block {
            hash: Some(sha256_hex(b"alpha\n")),
            newline_terminated: true,
            occurrences: 1,
            desired: None,
        }
    );
    assert_eq!(
        observations[Utf8Path::new("b/rc")],
        Observation::Block {
            hash: Some(sha256_hex(b"beta\n")),
            newline_terminated: true,
            occurrences: 1,
            desired: None,
        }
    );
}
