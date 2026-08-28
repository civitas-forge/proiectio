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
#[should_panic(expected = "free of `.`/`..`/empty segments")]
fn tree_rejects_dot_dot_components() {
    let _ = Tree::new().file("../escape.txt", "outside");
}

#[test]
#[should_panic(expected = "nodes may only sit under directories")]
fn tree_rejects_nodes_nested_under_a_symlink() {
    // A file under a declared symlink would write through the link —
    // potentially outside the fixture — so the builder refuses the shape.
    let _ = Tree::new()
        .symlink("escape", "../outside")
        .file("escape/x", "boom");
}

#[test]
#[should_panic(expected = "nodes may only sit under directories")]
fn tree_rejects_declaring_an_ancestor_file_after_its_descendant() {
    let _ = Tree::new().file("a/b.txt", "inner").file("a", "now a file");
}

#[test]
#[should_panic(expected = "free of `.`/`..`/empty segments")]
fn tree_rejects_dot_and_empty_segments() {
    // `Utf8Path::components()` would normalize these away; the raw-segment
    // check refuses them instead of silently rewriting the path.
    let _ = Tree::new().file("a/./b", "hidden dot");
}

#[test]
#[should_panic(expected = "refusing to write through it")]
fn write_under_refuses_an_on_disk_symlink_ancestor() {
    let fixture = Tree::new()
        .dir("real")
        .symlink("link", "real")
        .materialize();

    // `link/x` resolves through the symlink; the overlay must refuse, not
    // follow.
    Tree::new()
        .file("link/x", "boom")
        .write_under(fixture.root());
}

#[test]
fn write_under_replaces_a_symlink_leaf_instead_of_writing_through_it() {
    let fixture = Tree::new()
        .file("target.txt", "original")
        .symlink("alias", "target.txt")
        .materialize();

    Tree::new().file("alias", "new").write_under(fixture.root());

    // The link target is untouched; the link itself became a regular file.
    assert_eq!(
        fs::read(fixture.path("target.txt")).expect("read link target"),
        b"original"
    );
    assert_tree(
        fixture.root(),
        &Tree::new()
            .file("target.txt", "original")
            .file("alias", "new"),
    );
}

#[test]
fn write_under_clears_a_stale_exec_bit() {
    let fixture = Tree::new()
        .executable("bin/run", "#!/bin/sh\n")
        .materialize();

    let plain = Tree::new().file("bin/run", "just data");
    plain.write_under(fixture.root());

    assert_tree(fixture.root(), &plain);
}

#[test]
fn dropping_the_fixture_deletes_the_directory() {
    let fixture = Tree::new().file("a.txt", "alpha").materialize();
    let root = fixture.root().to_owned();
    assert!(root.exists());

    drop(fixture);

    assert!(!root.exists(), "RAII teardown removes {root}");
}
