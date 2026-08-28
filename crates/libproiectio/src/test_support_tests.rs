use std::collections::BTreeMap;
use std::fs;

use camino::Utf8PathBuf;

use super::*;
use crate::Entry;

fn sample_tree() -> Tree {
    Tree::new()
        .file("notes/a.txt", "alpha")
        .executable("bin/run", "#!/bin/sh\nexit 0\n")
        .symlink("latest", "notes/a.txt")
        .dir("empty")
}

#[test]
fn entries_builds_the_desired_tree_without_touching_disk() {
    let entries = sample_tree().entries();

    let expected: BTreeMap<Utf8PathBuf, Entry> = [
        (
            Utf8PathBuf::from("bin/run"),
            Entry::File {
                contents: b"#!/bin/sh\nexit 0\n".to_vec(),
                executable: true,
            },
        ),
        (
            Utf8PathBuf::from("latest"),
            Entry::Symlink {
                target: "notes/a.txt".to_owned(),
            },
        ),
        (
            Utf8PathBuf::from("notes/a.txt"),
            Entry::File {
                contents: b"alpha".to_vec(),
                executable: false,
            },
        ),
    ]
    .into();

    // The bare directory is implied by parent components in a desired tree,
    // so `entries` omits it.
    assert_eq!(entries, expected);
}

#[test]
fn materialize_round_trips_through_assert_tree() {
    let tree = sample_tree();
    let fixture = tree.materialize();

    assert!(fixture.root().is_absolute());
    assert!(fixture.path("notes/a.txt").is_absolute());
    assert_eq!(
        fs::read(fixture.path("notes/a.txt")).expect("read projected file"),
        b"alpha"
    );

    assert_tree(fixture.root(), &tree);
}

#[test]
fn tree_diff_names_each_divergence() {
    let fixture = sample_tree().materialize();

    let expected = Tree::new()
        .executable("notes/a.txt", "ALPHA") // contents + exec bit differ
        .symlink("latest", "notes/b.txt") // target differs
        .file("bin/run", "#!/bin/sh\nexit 0\n") // exec bit differs
        .file("missing.txt", "never written") // absent from disk
        .file("empty", "was a directory"); // kind differs

    let diff = tree_diff(fixture.root(), &expected);

    assert_eq!(
        diff,
        vec![
            "exec bit differs: bin/run (expected false, found true)".to_owned(),
            "kind differs: empty (expected file, found directory)".to_owned(),
            "link target differs: latest (expected \"notes/b.txt\", found \"notes/a.txt\")"
                .to_owned(),
            "missing: missing.txt (expected file)".to_owned(),
            "contents differ: notes/a.txt (expected \"ALPHA\", found \"alpha\")".to_owned(),
            "exec bit differs: notes/a.txt (expected true, found false)".to_owned(),
        ]
    );
}

#[test]
fn tree_diff_reports_undeclared_paths() {
    let tree = Tree::new().file("kept.txt", "ours");
    let fixture = tree.materialize();
    fs::write(fixture.path("foreign.txt"), b"not ours").expect("write foreign file");

    assert_eq!(
        tree_diff(fixture.root(), &tree),
        vec!["unexpected: foreign.txt (file)".to_owned()]
    );
}

#[test]
fn dropping_the_fixture_deletes_the_directory() {
    let fixture = Tree::new().file("a.txt", "alpha").materialize();
    let root = fixture.root().to_owned();
    assert!(root.exists());

    drop(fixture);

    assert!(!root.exists(), "RAII teardown removes {root}");
}
