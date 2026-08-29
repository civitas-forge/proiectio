use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};

use super::*;
use crate::test_support::Tree;

fn from_files(entries: BTreeMap<Utf8PathBuf, Entry>) -> Desired {
    Desired::from_source(entries, Origin::Files)
}

fn file(contents: &str, executable: bool) -> Entry {
    Entry::File {
        contents: contents.as_bytes().to_vec(),
        executable,
    }
}

#[test]
fn each_named_file_projects_under_its_basename_with_its_own_exec_bit() {
    let source = Tree::new()
        .file("etc/motd", "welcome\n")
        .executable("bin/tool", "#!/bin/sh\necho tool\n")
        .materialize();

    let desired = load_files(&[source.path("etc/motd"), source.path("bin/tool")]).unwrap();

    assert_eq!(
        desired,
        from_files(BTreeMap::from([
            (Utf8PathBuf::from("motd"), file("welcome\n", false)),
            (
                Utf8PathBuf::from("tool"),
                file("#!/bin/sh\necho tool\n", true)
            ),
        ]))
    );
}

#[test]
fn every_entry_is_sourced_from_the_named_files() {
    let source = Tree::new()
        .file("etc/motd", "welcome\n")
        .symlink("current", "releases/1.2.3")
        .materialize();

    let desired = load_files(&[source.path("etc/motd"), source.path("current")]).unwrap();

    assert_eq!(
        desired.sources().collect::<Vec<_>>(),
        vec![
            (&Utf8PathBuf::from("current"), &Origin::Files),
            (&Utf8PathBuf::from("motd"), &Origin::Files),
        ]
    );
}

#[test]
fn a_named_symlink_projects_as_a_link_carrying_its_target_verbatim() {
    let source = Tree::new()
        .file("releases/1.2.3/motd", "welcome\n")
        .symlink("current", "releases/1.2.3/motd")
        .materialize();

    let desired = load_files(&[source.path("current")]).unwrap();

    assert_eq!(
        desired,
        from_files(BTreeMap::from([(
            Utf8PathBuf::from("current"),
            Entry::Symlink {
                target: "releases/1.2.3/motd".to_owned(),
            },
        )]))
    );
}

#[test]
fn a_named_directory_fails_the_load() {
    let source = Tree::new().file("skeleton/motd", "welcome\n").materialize();
    let directory = source.path("skeleton");

    assert!(matches!(
        load_files(std::slice::from_ref(&directory)).unwrap_err(),
        Error::TreeNodeKind { path } if path == directory
    ));
}

#[test]
fn two_paths_sharing_a_file_name_fail_naming_both() {
    let source = Tree::new()
        .file("etc/motd", "welcome\n")
        .file("var/motd", "goodbye\n")
        .materialize();
    let (first, second) = (source.path("etc/motd"), source.path("var/motd"));

    let error = load_files(&[first.clone(), second.clone()]).unwrap_err();

    assert!(!error.is_refusal());
    assert!(matches!(
        &error,
        Error::FilesDuplicate { name, paths }
            if name == "motd" && *paths == BTreeSet::from([first.clone(), second.clone()])
    ));
    assert_eq!(
        error.to_string(),
        format!("more than one named path projects as motd: {first}, {second}")
    );
}

#[test]
fn two_spellings_of_one_path_are_one_entry() {
    let source = Tree::new().file("etc/motd", "welcome\n").materialize();
    let spelled = Utf8PathBuf::from(format!("{}/etc/./motd", source.root()));

    let desired = load_files(&[source.path("etc/motd"), spelled]).unwrap();

    assert_eq!(
        desired,
        from_files(BTreeMap::from([(
            Utf8PathBuf::from("motd"),
            file("welcome\n", false)
        )]))
    );
}

#[test]
fn naming_no_paths_gives_an_empty_tree() {
    let desired = load_files(&[]).unwrap();

    assert!(desired.is_empty());
    assert_eq!(desired, Desired::new());
}

#[test]
fn a_named_node_the_projection_never_writes_is_named_and_never_opened() {
    let source = Tree::new().file("keep.txt", "a").materialize();
    let socket = source.path("run.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind a unix socket");

    assert!(matches!(
        load_files(std::slice::from_ref(&socket)).unwrap_err(),
        Error::TreeNodeKind { path } if path == socket
    ));
}

#[test]
fn a_path_that_is_not_there_fails_at_the_path_it_names() {
    let source = Tree::new().file("keep.txt", "a").materialize();
    let missing = source.path("motd");

    assert!(matches!(
        load_files(std::slice::from_ref(&missing)).unwrap_err(),
        Error::Io { path, source } if path == missing
            && source.kind() == std::io::ErrorKind::NotFound
    ));
}

#[test]
fn the_root_carries_no_name_to_project_under() {
    assert!(matches!(
        load_files(&[Utf8PathBuf::from("/")]).unwrap_err(),
        Error::TreeNodeKind { path } if path == Utf8Path::new("/")
    ));
}
