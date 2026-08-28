use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};

use super::*;

/// A fixed absolute location for table tests: entries that only carry
/// inline `contents` never read the filesystem, so the file need not exist.
const MAPPING: &str = "/maps/deploy.toml";

fn parse_at(text: &str) -> Result<BTreeMap<Utf8PathBuf, Entry>> {
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
        tree(&[
            ("config/settings.toml", file("listen\n", false)),
            ("current", link("releases/1.2.3")),
        ])
    );
}

#[test]
fn a_mapping_without_tables_projects_nothing() {
    assert_eq!(parse_at("version = 1").unwrap(), BTreeMap::new());
}

#[test]
fn keys_are_lexically_normalized() {
    let text = r#"
        version = 1
        [files."a/../b"]
        contents = "x"
    "#;

    assert_eq!(parse_at(text).unwrap(), tree(&[("b", file("x", false))]));
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
        .keys()
        .map(|key| key.as_str().to_owned())
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
        tree(&[("plain", file("a", false)), ("tool", file("b", true))])
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
        tree(&[
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

    let want: BTreeSet<Utf8PathBuf> = [
        "/etc/passwd",
        "../sibling",
        "a//b",
        "dir\\sub",
        "../../outside/",
    ]
    .into_iter()
    .map(Utf8PathBuf::from)
    .collect();
    assert!(matches!(
        parse_at(text).unwrap_err(),
        Error::Containment { paths } if paths == want
    ));
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

#[test]
fn archive_entries_parse_structurally_but_are_not_yet_implemented() {
    let text = r#"
        version = 1
        [archives."vendor/"]
        source = "./assets/vendor.tar.gz"
        strip = 1
        [archives."plugins/"]
        source = "./assets/plugins.zip"
    "#;

    let want: BTreeSet<Utf8PathBuf> = ["vendor/", "plugins/"]
        .into_iter()
        .map(Utf8PathBuf::from)
        .collect();
    assert!(matches!(
        parse_at(text).unwrap_err(),
        Error::MappingArchiveUnimplemented { path, keys }
            if path == MAPPING && keys == want
    ));
}

/// The full example of `docs/cli-tour.lex` section 5, verbatim.
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
fn the_cli_tour_example_fails_only_on_its_archive_entry() {
    // Everything before the archives check passes; the entry itself is the
    // not-yet-implemented boundary (and no source file is read on the way).
    assert!(matches!(
        parse_at(CLI_TOUR_EXAMPLE).unwrap_err(),
        Error::MappingArchiveUnimplemented { keys, .. }
            if keys == BTreeSet::from([Utf8PathBuf::from("vendor/")])
    ));
}

#[test]
#[cfg(unix)]
fn the_cli_tour_example_minus_archives_parses_to_its_tree() {
    let (text, _) = CLI_TOUR_EXAMPLE
        .split_once("# extracted under")
        .expect("the example carries its archive comment");
    let tool = "#!/bin/sh\necho tool\n";
    let fixture = crate::test_support::Tree::new()
        .file("deploy.toml", text)
        .file("assets/tool.sh", tool)
        .materialize();

    assert_eq!(
        load_mapping(&fixture.path("deploy.toml")).unwrap(),
        tree(&[
            ("config/settings.toml", file("listen = \":8080\"\n", false)),
            ("bin/tool", file(tool, true)),
            ("current", link("releases/1.2.3")),
            ("toolchain", link("/opt/toolchains/rust-1.80")),
        ])
    );
}

#[test]
#[cfg(unix)]
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
        tree(&[
            ("beside", file("beside", false)),
            ("above", file("above", false)),
        ])
    );
}

#[test]
#[cfg(unix)]
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
        tree(&[("x", file("anywhere the invoker can read", false))])
    );
}

#[test]
#[cfg(unix)]
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
        tree(&[
            ("copied", file("#!/bin/sh\n", true)),
            ("cleared", file("#!/bin/sh\n", false)),
            ("raised", file("data", true)),
        ])
    );
}

#[test]
#[cfg(unix)]
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
#[cfg(unix)]
fn a_missing_mapping_file_is_an_io_error() {
    let fixture = crate::test_support::Tree::new().materialize();

    assert!(matches!(
        load_mapping(&fixture.path("gone.toml")).unwrap_err(),
        Error::Io { path, .. } if path == fixture.path("gone.toml")
    ));
}

#[test]
#[should_panic(expected = "mapping path must be absolute")]
fn a_relative_mapping_path_is_rejected() {
    let _ = load_mapping(Utf8Path::new("deploy.toml"));
}
