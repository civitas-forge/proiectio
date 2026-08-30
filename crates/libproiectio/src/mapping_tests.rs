use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};

use super::*;
use crate::test_support::{MissingName, origins_of};
use crate::{Dropped, RefusalKind};

// A fixed absolute location for table tests: entries that only carry
// inline `contents` never read the filesystem, so the file need not exist.
const MAPPING: &str = "/maps/deploy.toml";

fn parse_at(text: &str) -> Result<Desired> {
    parse(Utf8Path::new(MAPPING), text)
}

fn file(contents: &str, executable: bool) -> Entry {
    Entry::File {
        contents: contents.as_bytes().to_vec(),
        executable,
    }
}

fn link(target: &str) -> Entry {
    Entry::Symlink {
        target: target.to_owned(),
    }
}

fn from_mapping(path: &Utf8Path, entries: &[(&str, Entry)]) -> Desired {
    Desired::from_source(
        tree(entries),
        Origin::Mapping {
            path: path.to_owned(),
        },
    )
}

fn mapped(entries: &[(&str, Entry)]) -> Desired {
    from_mapping(Utf8Path::new(MAPPING), entries)
}

fn sourced(entries: &[(&str, Entry, Origin)]) -> Desired {
    let mut desired = Desired::new();
    for (key, entry, origin) in entries {
        desired.insert(Utf8PathBuf::from(*key), entry.clone(), origin.clone());
    }
    desired
}

fn tree(entries: &[(&str, Entry)]) -> BTreeMap<Utf8PathBuf, Entry> {
    entries
        .iter()
        .map(|(path, entry)| (Utf8PathBuf::from(*path), entry.clone()))
        .collect()
}

#[test]
fn a_minimal_mapping_parses_to_its_tree() {
    let text = r#"
        version = 1

        [files."config/settings.toml"]
        contents = "listen\n"

        [links."current"]
        target = "releases/1.2.3"
    "#;

    assert_eq!(
        parse_at(text).unwrap(),
        mapped(&[
            ("config/settings.toml", file("listen\n", false)),
            ("current", link("releases/1.2.3")),
        ])
    );
}

#[test]
fn a_mapping_without_tables_projects_nothing() {
    assert_eq!(parse_at("version = 1").unwrap(), Desired::new());
}

#[test]
fn keys_are_lexically_normalized() {
    let text = r#"
        version = 1
        [files."a/../b"]
        contents = "x"
    "#;

    assert_eq!(parse_at(text).unwrap(), mapped(&[("b", file("x", false))]));
}

#[test]
fn keys_stay_slash_separated_on_every_host() {
    // Path equality compares components, which on Windows would hide a key
    // rebuilt with `\` — and such a key would fail containment at plan
    // time. The byte-level assertion pins the separator on every host.
    let text = r#"
        version = 1
        [files."a/b/../c/d"]
        contents = "x"
    "#;

    let keys: Vec<String> = parse_at(text)
        .unwrap()
        .iter()
        .map(|(key, _)| key.as_str().to_owned())
        .collect();
    assert_eq!(keys, ["a/c/d"]);
}

#[test]
fn inline_contents_default_to_non_executable_and_the_override_wins() {
    let text = r#"
        version = 1
        [files."plain"]
        contents = "a"
        [files."tool"]
        contents = "b"
        executable = true
    "#;

    assert_eq!(
        parse_at(text).unwrap(),
        mapped(&[("plain", file("a", false)), ("tool", file("b", true))])
    );
}

#[test]
fn link_targets_are_carried_verbatim_absolute_included() {
    // Grading a target in-dest or external needs the destination, so it is
    // plan's judgment; the mapping source carries the string untouched.
    let text = r#"
        version = 1
        [links."toolchain"]
        target = "/opt/toolchains/rust-1.80"
        [links."escape"]
        target = "../outside"
    "#;

    assert_eq!(
        parse_at(text).unwrap(),
        mapped(&[
            ("toolchain", link("/opt/toolchains/rust-1.80")),
            ("escape", link("../outside")),
        ])
    );
}

#[test]
fn a_missing_version_is_a_format_error() {
    let text = r#"
        [files."x"]
        contents = "a"
    "#;

    assert!(matches!(
        parse_at(text).unwrap_err(),
        Error::MappingFormat { path, .. } if path == MAPPING
    ));
}

#[test]
fn a_future_version_is_reported_as_unsupported_before_strict_decoding() {
    // The unknown `[widgets]` table would fail strict decoding; the lenient
    // version pass reports the real problem first.
    let text = r#"
        version = 2
        [widgets."x"]
        frob = true
    "#;

    assert!(matches!(
        parse_at(text).unwrap_err(),
        Error::MappingVersion {
            path,
            found: 2,
            supported: MAPPING_VERSION,
        } if path == MAPPING
    ));
}

#[test]
fn unknown_keys_are_format_errors() {
    let cases: &[&str] = &[
        // A stray top-level key.
        "version = 1\nowner = \"site\"\n",
        // A stray key in a files entry.
        "version = 1\n[files.\"x\"]\ncontents = \"a\"\nmode = \"0755\"\n",
        // `executable` belongs to files entries, not links.
        "version = 1\n[links.\"x\"]\ntarget = \"y\"\nexecutable = true\n",
        // An archive entry is still decoded strictly.
        "version = 1\n[archives.\"v/\"]\nsource = \"./v.tar\"\nkeep = true\n",
        // An archive entry without a source is a shape error.
        "version = 1\n[archives.\"v/\"]\nstrip = 1\n",
    ];

    for text in cases {
        assert!(
            matches!(parse_at(text).unwrap_err(), Error::MappingFormat { .. }),
            "expected a format error for {text:?}"
        );
    }
}

#[test]
fn contents_and_source_together_name_the_key() {
    let text = r#"
        version = 1
        [files."bin/tool"]
        contents = "a"
        source = "./tool.sh"
    "#;

    assert!(matches!(
        parse_at(text).unwrap_err(),
        Error::MappingContentsXorSource { path, key }
            if path == MAPPING && key == "bin/tool"
    ));
}

#[test]
fn neither_contents_nor_source_names_the_key() {
    let text = r#"
        version = 1
        [files."bin/tool"]
        executable = true
    "#;

    assert!(matches!(
        parse_at(text).unwrap_err(),
        Error::MappingContentsXorSource { path, key }
            if path == MAPPING && key == "bin/tool"
    ));
}

#[test]
fn escaping_keys_are_refused_together_each_named_verbatim() {
    let text = r#"
        version = 1
        [files."/etc/passwd"]
        contents = "a"
        [files."../sibling"]
        contents = "b"
        [files."a//b"]
        contents = "c"
        [files."kept"]
        contents = "d"
        [links."dir\\sub"]
        target = "x"
        [archives."../../outside/"]
        source = "./v.tar"
    "#;

    let want: BTreeMap<Utf8PathBuf, Origin> = [
        "/etc/passwd",
        "../sibling",
        "a//b",
        "dir\\sub",
        "../../outside/",
    ]
    .into_iter()
    .map(|key| {
        (
            Utf8PathBuf::from(key),
            Origin::Mapping {
                path: MAPPING.into(),
            },
        )
    })
    .collect();
    let error = parse_at(text).unwrap_err();
    assert!(matches!(
        &error,
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment && origins_of(refused) == want
    ));
    let named = want
        .keys()
        .map(|key| format!("{key} (from mapping {MAPPING})"))
        .collect::<Vec<_>>()
        .join(", ");
    assert_eq!(
        error.to_string(),
        format!("refusing paths that violate containment: {named}"),
        "the refusal names the file to edit: {error}"
    );
}

#[test]
fn two_entries_projecting_one_normalized_key_are_refused() {
    let text = r#"
        version = 1
        [files."a/../b"]
        contents = "x"
        [links."b"]
        target = "y"
    "#;

    assert!(matches!(
        parse_at(text).unwrap_err(),
        Error::MappingDuplicate { path, key } if path == MAPPING && key == "b"
    ));
}

// A well-formed gzipped tar of `(name, contents, executable)` members.
// Mapping tests need only legitimate archives — the hostile corpus lives
// beside the expansion it exercises — so the `tar` writer builds them.
fn targz(members: &[(&str, &str, bool)]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    for (name, body, executable) in members {
        let mut header = tar::Header::new_ustar();
        header.set_size(body.len() as u64);
        header.set_mode(if *executable { 0o755 } else { 0o644 });
        header.set_entry_type(tar::EntryType::Regular);
        builder
            .append_data(&mut header, name, body.as_bytes())
            .expect("append a tar member");
    }
    builder
        .into_inner()
        .expect("finish the tar")
        .finish()
        .expect("finish the gzip stream")
}

// A zip carrying one member under the name given, verbatim — the writer
// does not sanitize what it is handed, which is what lets a mapping test
// spell a member that climbs out of its prefix.
fn zip_named(name: &str, body: &str) -> Vec<u8> {
    use std::io::Write;

    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file(name, zip::write::SimpleFileOptions::default())
        .expect("start a zip member");
    writer
        .write_all(body.as_bytes())
        .expect("write a zip member");
    writer.finish().expect("finish the zip").into_inner()
}

#[test]
fn archive_members_expand_under_their_prefix_as_ordinary_entries() {
    let text = r#"
        version = 1
        [archives."vendor/"]
        source = "./assets/vendor.tar.gz"
        strip = 1
        [archives."plugins/"]
        source = "./assets/plugins.zip"
    "#;
    let vendor = targz(&[
        ("vendor-1.0/lib/tool.so", "so\n", false),
        ("vendor-1.0/bin/run", "#!/bin/sh\n", true),
    ]);
    let fixture = crate::test_support::Tree::new()
        .file("deploy.toml", text)
        .file("assets/vendor.tar.gz", vendor)
        .file("assets/plugins.zip", zip_named("hello.lua", "print()\n"))
        .materialize();

    let plugins = Origin::Archive {
        path: fixture.path("assets/plugins.zip"),
        via: Some(fixture.path("deploy.toml")),
    };
    let vendor = Origin::Archive {
        path: fixture.path("assets/vendor.tar.gz"),
        via: Some(fixture.path("deploy.toml")),
    };
    assert_eq!(
        load_mapping(&fixture.path("deploy.toml")).unwrap(),
        sourced(&[
            ("plugins/hello.lua", file("print()\n", false), plugins),
            ("vendor/bin/run", file("#!/bin/sh\n", true), vendor.clone()),
            ("vendor/lib/tool.so", file("so\n", false), vendor),
        ])
    );
}

// The archive stock macOS `tar` writes: an AppleDouble `._vendor-1.0` beside
// the wrapper, which `strip = 1` leaves with no path. The mapping loads, and
// the dropped member is named against the archive that carried it — not
// against the prefix, which it never reaches.
#[test]
fn an_archive_member_strip_erases_is_dropped_and_named_by_its_archive() {
    let text = r#"
        version = 1
        [archives."vendor/"]
        source = "./assets/vendor.tar.gz"
        strip = 1
    "#;
    let vendor = targz(&[
        ("._vendor-1.0", "Mac OS X\n", false),
        ("vendor-1.0/lib/tool.so", "so\n", false),
    ]);
    let fixture = crate::test_support::Tree::new()
        .file("deploy.toml", text)
        .file("assets/vendor.tar.gz", vendor)
        .materialize();

    let origin = Origin::Archive {
        path: fixture.path("assets/vendor.tar.gz"),
        via: Some(fixture.path("deploy.toml")),
    };
    let loaded = load_mapping(&fixture.path("deploy.toml")).unwrap();
    assert_eq!(
        loaded.iter().collect::<Vec<_>>(),
        sourced(&[("vendor/lib/tool.so", file("so\n", false), origin.clone())])
            .iter()
            .collect::<Vec<_>>()
    );
    assert_eq!(
        loaded.dropped(),
        &BTreeSet::from([Dropped {
            member: Utf8PathBuf::from("._vendor-1.0"),
            prefix: Utf8PathBuf::from("vendor"),
            strip: 1,
            origin,
        }])
    );
}

// A member name is unique only inside its own archive. Two archives in one
// mapping both dropping `._pkg` are two drops, each named by the archive
// that carried it — neither displaces the other.
#[test]
fn two_archives_dropping_the_same_member_name_are_both_recorded() {
    let text = r#"
        version = 1
        [archives."vendor/"]
        source = "./assets/vendor.tar.gz"
        strip = 1
        [archives."plugins/"]
        source = "./assets/plugins.tar.gz"
        strip = 1
    "#;
    let fixture = crate::test_support::Tree::new()
        .file("deploy.toml", text)
        .file(
            "assets/vendor.tar.gz",
            targz(&[
                ("._pkg", "Mac OS X\n", false),
                ("pkg/lib/tool.so", "so\n", false),
            ]),
        )
        .file(
            "assets/plugins.tar.gz",
            targz(&[
                ("._pkg", "Mac OS X\n", false),
                ("pkg/hello.lua", "print()\n", false),
            ]),
        )
        .materialize();

    let carried_by = |archive: &str, prefix: &str| Dropped {
        member: Utf8PathBuf::from("._pkg"),
        prefix: Utf8PathBuf::from(prefix),
        strip: 1,
        origin: Origin::Archive {
            path: fixture.path(archive),
            via: Some(fixture.path("deploy.toml")),
        },
    };
    let loaded = load_mapping(&fixture.path("deploy.toml")).unwrap();
    assert_eq!(
        loaded.dropped(),
        &BTreeSet::from([
            carried_by("assets/plugins.tar.gz", "plugins"),
            carried_by("assets/vendor.tar.gz", "vendor"),
        ])
    );
}

// One archive named twice is two expansions. Their drops share a member
// name, an archive, and a mapping, so what tells them apart is the entry
// that asked for each: its prefix and its strip count.
#[test]
fn one_archive_expanded_under_two_prefixes_drops_a_member_once_per_entry() {
    let text = r#"
        version = 1
        [archives."vendor/"]
        source = "./assets/vendor.tar.gz"
        strip = 1
        [archives."backup/"]
        source = "./assets/vendor.tar.gz"
        strip = 2
    "#;
    let fixture = crate::test_support::Tree::new()
        .file("deploy.toml", text)
        .file(
            "assets/vendor.tar.gz",
            targz(&[
                ("._pkg", "Mac OS X\n", false),
                ("pkg/lib/tool.so", "so\n", false),
            ]),
        )
        .materialize();

    let asked_by = |prefix: &str, strip: u32| Dropped {
        member: Utf8PathBuf::from("._pkg"),
        prefix: Utf8PathBuf::from(prefix),
        strip,
        origin: Origin::Archive {
            path: fixture.path("assets/vendor.tar.gz"),
            via: Some(fixture.path("deploy.toml")),
        },
    };
    let loaded = load_mapping(&fixture.path("deploy.toml")).unwrap();
    assert_eq!(
        loaded.dropped(),
        &BTreeSet::from([asked_by("backup", 2), asked_by("vendor", 1)])
    );
    // `strip = 2` erases the wrapper *and* the directory under it, so the
    // backup entry keeps `tool.so` at its own root while `vendor/` keeps the
    // path below the wrapper.
    assert_eq!(
        loaded
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>(),
        vec!["backup/tool.so", "vendor/lib/tool.so"]
    );
}

// A member is judged before the prefix is joined, so a prefix confines
// rather than absorbs: joined first, `../escape` under `vendor/` would have
// normalized to `escape` — a projected path outside the prefix the mapping
// wrote, refused by nothing.
#[test]
fn an_archive_member_climbing_out_of_its_prefix_is_refused_by_name() {
    let text = r#"
        version = 1
        [archives."vendor/"]
        source = "./assets/vendor.zip"
    "#;
    let fixture = crate::test_support::Tree::new()
        .file("deploy.toml", text)
        .file("assets/vendor.zip", zip_named("../escape", "out\n"))
        .materialize();

    assert!(matches!(
        load_mapping(&fixture.path("deploy.toml")).unwrap_err(),
        Error::Refused(refused)
            if origins_of(&refused) == BTreeMap::from([(
                Utf8PathBuf::from("../escape"),
                Origin::Archive {
                    path: fixture.path("assets/vendor.zip"),
                    via: Some(fixture.path("deploy.toml")),
                },
            )])
    ));
}

// The definition of done: one mapping may name several archives, so a
// member path says neither which archive to open nor which file to edit.
// The refusal names both.
#[test]
fn a_refused_member_names_its_archive_and_the_mapping_that_named_it() {
    let text = r#"
        version = 1
        [archives."first/"]
        source = "assets/first.zip"
        [archives."second/"]
        source = "assets/second.zip"
    "#;
    let fixture = crate::test_support::Tree::new()
        .file("deploy.toml", text)
        .file("assets/first.zip", zip_named("ok.txt", "fine\n"))
        .file("assets/second.zip", zip_named("../escape", "out\n"))
        .materialize();

    let error = load_mapping(&fixture.path("deploy.toml")).unwrap_err();

    match &error {
        Error::Refused(refused) => {
            assert_eq!(
                origins_of(refused),
                BTreeMap::from([(
                    Utf8PathBuf::from("../escape"),
                    Origin::Archive {
                        path: fixture.path("assets/second.zip"),
                        via: Some(fixture.path("deploy.toml")),
                    },
                )])
            );
        }
        other => panic!("expected Containment, got {other:?}"),
    }
    assert_eq!(
        error.to_string(),
        format!(
            "refusing paths that violate containment: \
             ../escape (from archive {}, named by mapping {})",
            fixture.path("assets/second.zip"),
            fixture.path("deploy.toml"),
        )
    );
}

// An expanded member is an ordinary projected path, so one colliding with
// another entry's key is the same double claim two `[files]` keys would be.
#[test]
fn an_archive_member_colliding_with_another_entry_is_a_duplicate() {
    let text = r#"
        version = 1
        [files."vendor/lib/tool.so"]
        contents = "mine\n"
        [archives."vendor/"]
        source = "./assets/vendor.tar.gz"
    "#;
    let fixture = crate::test_support::Tree::new()
        .file("deploy.toml", text)
        .file(
            "assets/vendor.tar.gz",
            targz(&[("lib/tool.so", "theirs\n", false)]),
        )
        .materialize();

    assert!(matches!(
        load_mapping(&fixture.path("deploy.toml")).unwrap_err(),
        Error::MappingDuplicate { key, .. } if key == "vendor/lib/tool.so"
    ));
}

// Two archive tables naming one prefix would merge into one location with
// no rule for which member wins where they overlap.
#[test]
fn two_archive_tables_naming_one_prefix_are_a_duplicate() {
    let text = r#"
        version = 1
        [archives."vendor/"]
        source = "./a.tar"
        [archives."vendor"]
        source = "./b.tar"
    "#;

    assert!(matches!(
        parse_at(text).unwrap_err(),
        Error::MappingDuplicate { path, key } if path == MAPPING && key == "vendor"
    ));
}

// Every `[archives]` table in one mapping spends one byte budget. The
// bound is on what one untrusted input may make the process allocate, and
// a mapping is one input: per-table budgets would let a mapping buy a
// multiple of the bound by naming one small bomb from several tables, with
// every expanded tree live at once because they all merge into one.
#[test]
fn archive_tables_in_one_mapping_share_one_byte_budget() {
    // Two halves of the budget, each fine alone and not together.
    let half = usize::try_from(crate::archive::MAX_EXPANDED_BYTES / 2 + (1 << 20)).unwrap();
    let body = "0".repeat(half);
    let text = r#"
        version = 1
        [archives."first/"]
        source = "./half.tar.gz"
        [archives."second/"]
        source = "./half.tar.gz"
    "#;
    let fixture = crate::test_support::Tree::new()
        .file("deploy.toml", text)
        .file("half.tar.gz", targz(&[("big", &body, false)]))
        .materialize();

    assert!(matches!(
        load_mapping(&fixture.path("deploy.toml")).unwrap_err(),
        Error::ArchiveTooLarge { .. }
    ));
}

// The full example of `docs/cli-tour.lex` section 5, verbatim.
const CLI_TOUR_EXAMPLE: &str = r#"version = 1

[files."config/settings.toml"]
contents = """
listen = ":8080"
"""

[files."bin/tool"]
source = "./assets/tool.sh"
executable = true

# standard symlink semantics: target is written verbatim and
# resolves relative to the link's parent, inside dest
[links."current"]
target = "releases/1.2.3"

# absolute target: refused unless the invoker passes
# --allow-external-targets
[links."toolchain"]
target = "/opt/toolchains/rust-1.80"

# extracted under the key prefix at plan time; each member
# becomes an ordinary manifest entry
[archives."vendor/"]
source = "./assets/vendor.tar.gz"
strip = 1
"#;

#[test]
fn the_cli_tour_example_parses_to_its_tree_archive_included() {
    let tool = "#!/bin/sh\necho tool\n";
    let vendor = targz(&[
        ("vendor-1.0/lib/libv.so", "elf\n", false),
        ("vendor-1.0/bin/vendor", "#!/bin/sh\n", true),
    ]);
    let fixture = crate::test_support::Tree::new()
        .file("deploy.toml", CLI_TOUR_EXAMPLE)
        .file("assets/tool.sh", tool)
        .file("assets/vendor.tar.gz", vendor)
        .materialize();

    // The archive's members are ordinary entries beside the mapping's own,
    // keyed under the table's prefix with the wrapper directory stripped.
    let mapping = Origin::Mapping {
        path: fixture.path("deploy.toml"),
    };
    let vendor_origin = Origin::Archive {
        path: fixture.path("assets/vendor.tar.gz"),
        via: Some(fixture.path("deploy.toml")),
    };
    assert_eq!(
        load_mapping(&fixture.path("deploy.toml")).unwrap(),
        sourced(&[
            (
                "config/settings.toml",
                file("listen = \":8080\"\n", false),
                mapping.clone(),
            ),
            ("bin/tool", file(tool, true), mapping.clone()),
            ("current", link("releases/1.2.3"), mapping.clone()),
            ("toolchain", link("/opt/toolchains/rust-1.80"), mapping),
            (
                "vendor/bin/vendor",
                file("#!/bin/sh\n", true),
                vendor_origin.clone(),
            ),
            ("vendor/lib/libv.so", file("elf\n", false), vendor_origin),
        ])
    );
}

#[test]
fn relative_sources_resolve_against_the_mapping_files_directory() {
    // The mapping sits in a subdirectory; its sources resolve beside it —
    // including one climbing out of that subdirectory, because reads may
    // come from anywhere the invoker can read.
    let text = r#"
        version = 1
        [files."beside"]
        source = "./assets/beside.txt"
        [files."above"]
        source = "../shared/above.txt"
    "#;
    let fixture = crate::test_support::Tree::new()
        .file("nested/deploy.toml", text)
        .file("nested/assets/beside.txt", "beside")
        .file("shared/above.txt", "above")
        .materialize();

    assert_eq!(
        load_mapping(&fixture.path("nested/deploy.toml")).unwrap(),
        from_mapping(
            &fixture.path("nested/deploy.toml"),
            &[
                ("beside", file("beside", false)),
                ("above", file("above", false)),
            ],
        )
    );
}

#[test]
fn an_absolute_source_is_read_as_given() {
    let fixture = crate::test_support::Tree::new()
        .file("elsewhere/content.txt", "anywhere the invoker can read")
        .materialize();
    let source = fixture.path("elsewhere/content.txt");
    let text = format!("version = 1\n[files.\"x\"]\nsource = \"{source}\"\n");
    let mapping = crate::test_support::Tree::new()
        .file("deploy.toml", text)
        .materialize();

    assert_eq!(
        load_mapping(&mapping.path("deploy.toml")).unwrap(),
        from_mapping(
            &mapping.path("deploy.toml"),
            &[("x", file("anywhere the invoker can read", false))],
        )
    );
}

#[test]
fn source_metadata_is_copied_and_the_override_wins() {
    let text = r#"
        version = 1
        [files."copied"]
        source = "./run.sh"
        [files."cleared"]
        source = "./run.sh"
        executable = false
        [files."raised"]
        source = "./data.txt"
        executable = true
    "#;
    let fixture = crate::test_support::Tree::new()
        .file("deploy.toml", text)
        .executable("run.sh", "#!/bin/sh\n")
        .file("data.txt", "data")
        .materialize();

    assert_eq!(
        load_mapping(&fixture.path("deploy.toml")).unwrap(),
        from_mapping(
            &fixture.path("deploy.toml"),
            &[
                ("copied", file("#!/bin/sh\n", true)),
                ("cleared", file("#!/bin/sh\n", false)),
                ("raised", file("data", true)),
            ],
        )
    );
}

#[test]
fn a_missing_source_is_an_io_error_naming_the_resolved_path() {
    let text = r#"
        version = 1
        [files."x"]
        source = "./assets/gone.txt"
    "#;
    let fixture = crate::test_support::Tree::new()
        .file("deploy.toml", text)
        .materialize();

    assert!(matches!(
        load_mapping(&fixture.path("deploy.toml")).unwrap_err(),
        Error::Io { path, .. } if path == fixture.path("assets/gone.txt")
    ));
}

#[test]
fn a_missing_mapping_file_is_an_io_error() {
    let fixture = crate::test_support::Tree::new().materialize();

    assert!(matches!(
        load_mapping(&fixture.path("gone.toml")).unwrap_err(),
        Error::Io { path, .. } if path == fixture.path("gone.toml")
    ));
}

// A relative path resolves against the current directory rather than
// failing, so the error names where the load actually looked.
#[test]
fn a_relative_mapping_path_resolves_against_the_current_directory() {
    let absent = MissingName::with_suffix(".toml");

    assert!(matches!(
        load_mapping(absent.relative()).unwrap_err(),
        Error::Io { path, .. } if path == absent.absolute()
    ));
}
