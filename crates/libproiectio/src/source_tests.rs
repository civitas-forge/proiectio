use std::io::Write;

use super::*;
use crate::Origin;
use crate::test_support::Tree;

fn tar_gz(members: &[(&str, &str)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, body) in members {
        let mut header = tar::Header::new_ustar();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o644);
        header.set_size(body.len() as u64);
        builder
            .append_data(&mut header, name, body.as_bytes())
            .expect("append a member");
    }
    let tar = builder.into_inner().expect("finish the tar stream");
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&tar).expect("gzip the archive");
    encoder.finish().expect("finish the gzip stream")
}

#[test]
fn a_directory_walks_as_a_tree() {
    let source = Tree::new()
        .file("config/settings.toml", "listen = \":8080\"\n")
        .executable("bin/tool", "#!/bin/sh\n")
        .materialize();

    let loaded = load_source(source.root(), None).expect("load the directory");

    assert_eq!(
        loaded,
        Desired::from_source(
            Tree::new()
                .file("config/settings.toml", "listen = \":8080\"\n")
                .executable("bin/tool", "#!/bin/sh\n")
                .entries(),
            Origin::Tree {
                path: source.root().to_owned(),
            },
        )
    );
}

#[test]
fn an_archive_expands_with_strip_applied() {
    let bytes = tar_gz(&[
        ("skeleton-1.2/bin/tool", "#!/bin/sh\n"),
        ("skeleton-1.2/config/settings.toml", "listen = \":8080\"\n"),
    ]);
    let fixture = Tree::new().file("skeleton-1.2.tar.gz", bytes).materialize();
    let path = fixture.path("skeleton-1.2.tar.gz");

    let loaded = load_source(&path, Some(1)).expect("expand the archive");

    assert_eq!(
        loaded,
        Desired::from_source(
            Tree::new()
                .file("bin/tool", "#!/bin/sh\n")
                .file("config/settings.toml", "listen = \":8080\"\n")
                .entries(),
            Origin::Archive {
                path: path.clone(),
                via: None,
            },
        )
    );
}

#[test]
fn strip_on_a_directory_is_an_error() {
    let source = Tree::new().file("bin/tool", "#!/bin/sh\n").materialize();

    let error = load_source(source.root(), Some(1)).unwrap_err();

    assert!(!error.is_refusal());
    assert!(matches!(
        &error,
        Error::StripOnDirectory { path } if path == source.root()
    ));
    assert_eq!(
        error.to_string(),
        format!(
            "source {} is a directory: strip drops components of archive members",
            source.root()
        )
    );
}

#[test]
fn a_name_no_decoder_claims_is_an_error() {
    let fixture = Tree::new()
        .file("notes.txt", "not an archive\n")
        .materialize();
    let path = fixture.path("notes.txt");

    let error = load_source(&path, None).unwrap_err();

    assert!(matches!(
        &error,
        Error::ArchiveFormatUnknown { path: named } if *named == path
    ));
}

#[test]
fn a_missing_path_fails_on_the_open() {
    let fixture = Tree::new().materialize();
    let path = fixture.path("absent.tar.gz");

    let error = load_source(&path, None).unwrap_err();

    assert!(matches!(
        &error,
        Error::Io { path: named, source }
            if *named == path && source.kind() == std::io::ErrorKind::NotFound
    ));
}
