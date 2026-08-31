use std::collections::BTreeMap;
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::fs_utf8::Dir as Utf8Dir;

use super::*;
use crate::test_support::{Fixture, MissingName, Tree, assert_tree, origins_of, state_at};
use crate::{
    Action, ApplyReport, Manifest, Origin, PlanOptions, Refusal, RefusalKind, apply, block_markers,
    decide, load_manifest, observe,
};

// Ambient authority is the test's to spend; the library opens only the tree it is handed.
fn dir_at(root: &Utf8Path) -> Utf8Dir {
    Utf8Dir::open_ambient_dir(root, cap_std::ambient_authority()).expect("open fixture root")
}

fn walked(source: &Fixture) -> Origin {
    Origin::Tree {
        path: source.root().to_owned(),
    }
}

fn from_tree(source: &Fixture, entries: BTreeMap<Utf8PathBuf, Entry>) -> Desired {
    Desired::from_source(entries, walked(source))
}

fn refused(source: &Fixture, names: &[&str]) -> BTreeMap<Utf8PathBuf, Origin> {
    names
        .iter()
        .map(|name| (Utf8PathBuf::from(name), walked(source)))
        .collect()
}

fn project(dest: &Fixture, state: &Fixture, desired: &Desired) -> ApplyReport {
    let dest_dir = dir_at(dest.root());
    let state_dir = state_at(state.root());
    let manifest = load_manifest(&state_dir).expect("load manifest");
    let observations =
        observe(&dest_dir, &manifest, &block_markers(desired)).expect("observe destination");
    let plan = decide(
        "tree",
        desired,
        &manifest,
        &observations,
        None,
        PlanOptions::default(),
    )
    .expect("decide");
    apply(&dest_dir, &state_dir, &manifest, &plan).expect("apply the plan")
}

fn actions_for(desired: &Desired) -> BTreeMap<Utf8PathBuf, Action> {
    let dest = Tree::new().materialize();
    let dest_dir = dir_at(dest.root());
    let manifest = Manifest::new();
    let observations =
        observe(&dest_dir, &manifest, &block_markers(desired)).expect("observe destination");
    decide(
        "tree",
        desired,
        &manifest,
        &observations,
        None,
        PlanOptions::default(),
    )
    .expect("decide")
    .actions
}

#[test]
fn a_tree_of_files_exec_bits_and_an_in_tree_link_round_trips() {
    let declared = Tree::new()
        .file("config/settings.toml", "listen = \":8080\"\n")
        .executable("bin/tool", "#!/bin/sh\necho tool\n")
        .file("releases/1.2.3/marker", "release\n")
        .symlink("current", "releases/1.2.3");
    let source = declared.materialize();

    let desired = load_tree(source.root(), crate::Limits::default()).unwrap();
    assert_eq!(desired, from_tree(&source, declared.entries()));

    let (dest, state) = (Tree::new().materialize(), Tree::new().materialize());
    project(&dest, &state, &desired);

    assert_tree(dest.root(), &declared);
    assert_eq!(
        fs::read(dest.path("current/marker")).expect("read through the projected link"),
        b"release\n",
    );
}

#[test]
fn a_link_out_of_the_tree_is_carried_verbatim_and_graded_external() {
    // What reaches the tree is the pointer, never the pointed-at bytes.
    let outside = Tree::new()
        .file("secret.txt", "not the projection's")
        .materialize();
    let target = outside.path("secret.txt");
    let source = Tree::new()
        .symlink("escape", target.as_str())
        .symlink("climb", "../../elsewhere")
        .materialize();

    let desired = load_tree(source.root(), crate::Limits::default()).unwrap();
    assert_eq!(
        desired,
        from_tree(
            &source,
            BTreeMap::from([
                (
                    Utf8PathBuf::from("escape"),
                    Entry::Symlink {
                        target: target.to_string(),
                    },
                ),
                (
                    Utf8PathBuf::from("climb"),
                    Entry::Symlink {
                        target: "../../elsewhere".to_owned(),
                    },
                ),
            ]),
        ),
    );

    let actions = actions_for(&desired);
    assert!(matches!(
        actions.get(Utf8Path::new("escape")),
        Some(Action::Refuse {
            refusal: Refusal::ExternalTarget { target: refused },
        }) if *refused == target.as_str()
    ));
    assert!(matches!(
        actions.get(Utf8Path::new("climb")),
        Some(Action::Refuse {
            refusal: Refusal::ExternalTarget { target },
        }) if target == "../../elsewhere"
    ));
}

#[test]
fn a_directory_link_out_of_the_tree_is_carried_not_walked_into() {
    // Descending the link would copy a subtree the caller never named.
    let outside = Tree::new()
        .file("secrets/key.txt", "private")
        .file("secrets/deeper/more.txt", "private")
        .materialize();
    let source = Tree::new()
        .file("keep.txt", "a")
        .symlink("peek", outside.path("secrets").as_str())
        .materialize();

    let keys: Vec<String> = load_tree(source.root(), crate::Limits::default())
        .unwrap()
        .iter()
        .map(|(key, _)| key.as_str().to_owned())
        .collect();
    assert_eq!(keys, ["keep.txt", "peek"]);
}

#[test]
fn an_in_tree_link_is_graded_in_dest() {
    let source = Tree::new()
        .file("releases/1.2.3/marker", "release\n")
        .symlink("current", "releases/1.2.3")
        .materialize();

    let actions = actions_for(&load_tree(source.root(), crate::Limits::default()).unwrap());
    assert!(matches!(
        actions.get(Utf8Path::new("current")),
        Some(Action::Write { .. })
    ));
}

// Gzip's magic bytes and deflate header, then bytes no text encoding would
// survive — enough to prove the copy is byte-for-byte and not a decode.
const ARCHIVE: &[u8] = b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x03\xff\x00\xfe\x01";

#[test]
fn an_archive_inside_the_tree_is_a_file_copied_byte_for_byte() {
    // Extraction happens only where it is asked for (`docs/cli-tour.lex`
    // section 5); an archive met while walking is content like any other.
    let declared = Tree::new().file("vendor/bundle.tar.gz", ARCHIVE);
    let source = declared.materialize();

    let desired = load_tree(source.root(), crate::Limits::default()).unwrap();
    assert_eq!(
        desired,
        from_tree(
            &source,
            BTreeMap::from([(
                Utf8PathBuf::from("vendor/bundle.tar.gz"),
                Entry::File {
                    contents: ARCHIVE.to_vec(),
                    executable: false,
                },
            )]),
        ),
    );

    let (dest, state) = (Tree::new().materialize(), Tree::new().materialize());
    project(&dest, &state, &desired);

    assert_tree(dest.root(), &declared);
    assert_eq!(
        fs::read(dest.path("vendor/bundle.tar.gz")).expect("read the projected archive"),
        ARCHIVE,
    );
}

#[test]
fn empty_directories_carry_no_entry() {
    let source = Tree::new()
        .dir("empty")
        .dir("also/empty")
        .file("deep/nested/file.txt", "x")
        .materialize();

    let keys: Vec<String> = load_tree(source.root(), crate::Limits::default())
        .unwrap()
        .iter()
        .map(|(key, _)| key.as_str().to_owned())
        .collect();
    assert_eq!(keys, ["deep/nested/file.txt"]);
}

#[test]
fn a_source_holding_nothing_projects_nothing() {
    let source = Tree::new().materialize();

    assert_eq!(
        load_tree(source.root(), crate::Limits::default()).unwrap(),
        Desired::new()
    );
}

#[test]
fn keys_stay_slash_separated_on_every_host() {
    let source = Tree::new().file("a/b/c.txt", "x").materialize();

    let keys: Vec<String> = load_tree(source.root(), crate::Limits::default())
        .unwrap()
        .iter()
        .map(|(key, _)| key.as_str().to_owned())
        .collect();
    assert_eq!(keys, ["a/b/c.txt"]);
}

#[test]
fn names_the_containment_gateway_refuses_are_reported_together_verbatim() {
    // Ordinary Unix names, none of them a path the projection may create:
    // a backslash, a colon, a trailing dot, and a Windows device name.
    let source = Tree::new()
        .file("NUL", "device")
        .file("dir\\sub", "backslash")
        .file("weird:name", "stream")
        .file("sub/trailing.", "dot")
        .file("sub/COM1", "port")
        .file("kept", "fine")
        .materialize();

    let want = refused(
        &source,
        &["NUL", "dir\\sub", "weird:name", "sub/trailing.", "sub/COM1"],
    );
    assert!(matches!(
        load_tree(source.root(), crate::Limits::default()).unwrap_err(),
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment && origins_of(&refused) == want
    ));
}

#[test]
fn a_directory_the_gateway_refuses_is_named_instead_of_its_descendants() {
    let source = Tree::new()
        .file("COM1/a.txt", "a")
        .file("COM1/deeper/b.txt", "b")
        .file("kept", "fine")
        .materialize();

    let want = refused(&source, &["COM1"]);
    assert!(matches!(
        load_tree(source.root(), crate::Limits::default()).unwrap_err(),
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment && origins_of(&refused) == want
    ));
}

#[test]
fn a_refused_directory_holding_nothing_is_still_refused() {
    let source = Tree::new().dir("weird:name").materialize();

    let want = refused(&source, &["weird:name"]);
    assert!(matches!(
        load_tree(source.root(), crate::Limits::default()).unwrap_err(),
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment && origins_of(&refused) == want
    ));
}

// Nests `depth` directories under `root` and returns the deepest one;
// the chain stays inside the host's path limit at these depths.
fn nest(root: &Utf8Path, depth: usize) -> Utf8PathBuf {
    let deep = root.join(vec!["d"; depth].join("/"));
    fs::create_dir_all(&deep).expect("nest directories");
    deep
}

#[test]
fn a_tree_at_the_depth_limit_loads_and_one_past_it_is_named() {
    // The walk spends a stack frame per level; past the limit it must error, not overflow.
    let source = Tree::new().materialize();
    let deepest = nest(source.root(), MAX_WALK_DEPTH);
    fs::write(deepest.join("marker"), "deep").expect("write the deepest file");

    let desired = load_tree(source.root(), crate::Limits::default()).unwrap();
    assert_eq!(desired.len(), 1);

    let past = nest(source.root(), MAX_WALK_DEPTH + 1);
    assert!(matches!(
        load_tree(source.root(), crate::Limits::default()).unwrap_err(),
        Error::TreeTooDeep { path, limit } if path == past && limit == MAX_WALK_DEPTH
    ));
}

#[test]
fn a_node_kind_the_projection_never_writes_is_named_and_never_opened() {
    let source = Tree::new().file("keep.txt", "a").materialize();
    let socket = source.path("sock");
    // A socket rather than a FIFO: it needs no privileges and no `mknod`,
    // and the walk judges every kind it never writes the same way.
    let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind a unix socket");

    assert!(matches!(
        load_tree(source.root(), crate::Limits::default()).unwrap_err(),
        Error::TreeNodeKind { path } if path == socket
    ));
}

#[test]
fn a_symlink_target_that_is_not_utf8_names_the_link() {
    use std::os::unix::ffi::OsStrExt;

    let source = Tree::new().file("keep.txt", "a").materialize();
    let link = source.path("bad-target");
    std::os::unix::fs::symlink(
        std::ffi::OsStr::from_bytes(b"target\xff"),
        std::path::Path::new(link.as_str()),
    )
    .expect("create a link with a non-UTF-8 target");

    assert!(matches!(
        load_tree(source.root(), crate::Limits::default()).unwrap_err(),
        Error::TreeTargetNotUtf8 { path, target }
            if path == link && target.starts_with("target")
    ));
}

// Linux only: APFS rejects a filename that is not valid UTF-8, so the
// fixture cannot be built on macOS.
#[test]
#[cfg(target_os = "linux")]
fn a_name_that_is_not_utf8_names_the_directory_holding_it() {
    use std::os::unix::ffi::OsStrExt;

    let source = Tree::new().file("sub/keep.txt", "a").materialize();
    let dir = source.path("sub");
    let bad = std::path::Path::new(dir.as_str()).join(std::ffi::OsStr::from_bytes(b"bad\xff"));
    fs::write(&bad, "x").expect("write a file with a non-UTF-8 name");

    assert!(matches!(
        load_tree(source.root(), crate::Limits::default()).unwrap_err(),
        Error::TreeNameNotUtf8 { path, name }
            if path == dir && name.starts_with("bad")
    ));
}

#[test]
fn executable_bits_come_from_the_source_metadata() {
    let source = Tree::new()
        .executable("bin/run", "#!/bin/sh\n")
        .file("data.txt", "data")
        .materialize();

    assert_eq!(
        load_tree(source.root(), crate::Limits::default()).unwrap(),
        from_tree(
            &source,
            BTreeMap::from([
                (
                    Utf8PathBuf::from("bin/run"),
                    Entry::File {
                        contents: b"#!/bin/sh\n".to_vec(),
                        executable: true,
                    },
                ),
                (
                    Utf8PathBuf::from("data.txt"),
                    Entry::File {
                        contents: b"data".to_vec(),
                        executable: false,
                    },
                ),
            ]),
        ),
    );
}

#[test]
fn a_missing_source_is_an_io_error_naming_it() {
    let fixture = Tree::new().materialize();
    let gone = fixture.path("gone");

    assert!(matches!(
        load_tree(&gone, crate::Limits::default()).unwrap_err(),
        Error::Io {
            role: IoRole::SourceTree,
            path,
            ..
        } if path == gone
    ));
}

#[test]
fn a_source_that_is_a_file_is_an_io_error_naming_it() {
    let fixture = Tree::new().file("notes.txt", "x").materialize();
    let file = fixture.path("notes.txt");

    assert!(matches!(
        load_tree(&file, crate::Limits::default()).unwrap_err(),
        Error::Io {
            role: IoRole::SourceTree,
            path,
            ..
        } if path == file
    ));
}

#[test]
fn a_relative_source_path_resolves_against_the_current_directory() {
    let absent = MissingName::with_suffix("");

    assert!(matches!(
        load_tree(absent.relative(), crate::Limits::default()).unwrap_err(),
        Error::Io {
            role: IoRole::SourceTree,
            path,
            ..
        } if path == absent.absolute()
    ));
}

#[test]
fn a_tree_whose_files_outweigh_the_bound_fails_the_load() {
    let source = Tree::new()
        .file("a.bin", "0".repeat(600))
        .file("b.bin", "0".repeat(600))
        .materialize();

    let limits = Limits {
        max_source_bytes: 1000,
    };
    assert!(matches!(
        load_tree(source.root(), limits).unwrap_err(),
        Error::SourceTooLarge { limit, .. } if limit == 1000
    ));
    // Neither file exceeds the bound alone; the walk spends one budget across
    // both, and the default is wide enough for both together.
    load_tree(source.root(), Limits::default()).expect("load under the default bound");
}

#[test]
fn a_tree_of_empty_files_spends_the_bound_on_the_names_it_holds() {
    let mut source = Tree::new();
    for index in 0..100 {
        source = source.file(format!("empty-{index:03}"), "");
    }
    let source = source.materialize();

    // Each key is "empty-NNN": nine bytes, so a hundred of them is 900.
    let limits = Limits::default().with_max_source_bytes(800);
    assert!(matches!(
        load_tree(source.root(), limits).unwrap_err(),
        Error::SourceTooLarge { limit, .. } if limit == 800
    ));
    load_tree(source.root(), Limits::default().with_max_source_bytes(900))
        .expect("a bound covering every key exactly");
}

// Whichever charge runs the budget out, the refusal names the node it was
// on — not the directory holding it.
#[test]
fn the_refusal_names_the_node_the_budget_ran_out_on() {
    let source = Tree::new()
        .file("nested/big.bin", "0".repeat(600))
        .materialize();
    let refused_at = |bound: u64| match load_tree(
        source.root(),
        Limits::default().with_max_source_bytes(bound),
    )
    .unwrap_err()
    {
        Error::SourceTooLarge { path, .. } => path,
        other => panic!("expected the bound to refuse, got {other}"),
    };

    // 100 bytes runs out inside the file's own 600, mid-read.
    assert_eq!(refused_at(100), source.root().join("nested/big.bin"));
    // 610 covers the bytes and leaves 10, which the 14-byte key does not
    // fit — a different charge, and the same node named.
    assert_eq!(refused_at(610), source.root().join("nested/big.bin"));
}

#[test]
fn refused_names_spend_the_bound_they_are_held_against() {
    let mut source = Tree::new();
    for index in 0..100 {
        // A backslash is refused by containment, and the file is empty, so
        // the name is the only thing the walk holds.
        source = source.file(format!("bad\\{index:03}"), "");
    }
    let source = source.materialize();

    assert!(matches!(
        load_tree(source.root(), Limits::default().with_max_source_bytes(200)).unwrap_err(),
        Error::SourceTooLarge { limit, .. } if limit == 200
    ));
    assert!(matches!(
        load_tree(source.root(), Limits::default()).unwrap_err(),
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment
    ));
}

#[test]
fn a_symlink_target_spends_the_bound() {
    let source = Tree::new().symlink("current", "releases/v1").materialize();

    // "current" is 7 bytes of key and "releases/v1" is 11 of target.
    let limits = Limits::default().with_max_source_bytes(17);
    assert!(matches!(
        load_tree(source.root(), limits).unwrap_err(),
        Error::SourceTooLarge { limit, .. } if limit == 17
    ));
    load_tree(source.root(), Limits::default().with_max_source_bytes(18))
        .expect("a bound covering the key and the target");
}
