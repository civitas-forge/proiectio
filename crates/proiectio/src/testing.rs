//! What a test of the whole shell needs before it can run one: an isolated
//! harness, and a destination the library itself materialized.

use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};
use libproiectio::{Desired, Entry, Manifest, PlanOptions, Projection};
use standout::{ColorMode, DEFAULT_MISSING_STYLE_INDICATOR as MISSING, Theme};
use standout_test::TestHarness;
use tempfile::TempDir;

pub(crate) const OWNER: &str = "default";

/// Isolates the config scopes, which resolve under the platform config
/// directory, inside a directory the test owns.
pub(crate) fn harness(dir: &TempDir) -> TestHarness {
    let home = dir.path().to_str().expect("a usable path");
    TestHarness::new()
        .no_color()
        .cwd(dir.path())
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
}

pub(crate) fn utf8(dir: &TempDir) -> Utf8PathBuf {
    Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("a UTF-8 temporary directory")
}

/// The tour's own material: a mapping beside its assets, and an empty
/// destination under the same working directory.
pub(crate) fn tour() -> (TempDir, Utf8PathBuf, Utf8PathBuf) {
    let dir = TempDir::new().expect("a temporary directory");
    let root = utf8(&dir);
    let dest = root.join("dest");
    std::fs::create_dir(&dest).expect("a destination");
    let deploy = mapping(&root);
    (dir, dest, deploy)
}

/// What a projection recorded, read back through the library.
pub(crate) fn manifest_of(dest: &Utf8Path) -> Manifest {
    Projection::new(dest, None)
        .expect("a projection")
        .manifest()
        .expect("a manifest")
}

pub(crate) fn modified(path: &Utf8Path) -> std::time::SystemTime {
    std::fs::metadata(path)
        .expect("a projected path")
        .modified()
        .expect("a modification time")
}

/// Projects three files, then edits one and removes another, so a status of
/// `dest` reads one drifted, one clean and one missing path.
pub(crate) fn classified(dest: &Utf8Path) {
    project(
        dest,
        &Desired::from_caller(BTreeMap::from([
            (Utf8PathBuf::from("bin/tool"), file(b"#!/bin/sh\n")),
            (
                Utf8PathBuf::from("config/settings.toml"),
                file(b"listen = \":8080\"\n"),
            ),
            (Utf8PathBuf::from("current"), file(b"releases/1.2.3\n")),
        ])),
    );
    std::fs::write(dest.join("bin/tool"), b"#!/bin/sh\necho edited\n").expect("an edited file");
    std::fs::remove_file(dest.join("current")).expect("a removed file");
}

/// The mapping `docs/cli-tour.lex` section 1 writes: an inline file, an
/// executable file from a source beside the mapping, and a link.
pub(crate) fn mapping(dir: &Utf8Path) -> Utf8PathBuf {
    let assets = dir.join("assets");
    std::fs::create_dir_all(&assets).expect("an asset directory");
    let tool = assets.join("tool.sh");
    std::fs::write(&tool, b"#!/bin/sh\necho hi\n").expect("a source file");
    executable(&tool);

    let path = dir.join("deploy.toml");
    std::fs::write(
        &path,
        b"version = 1\n\
          \n\
          [files.\"config/settings.toml\"]\n\
          contents = \"listen = \\\":8080\\\"\\n\"\n\
          \n\
          [files.\"bin/tool\"]\n\
          source = \"./assets/tool.sh\"\n\
          executable = true\n\
          \n\
          [links.\"current\"]\n\
          target = \"releases/1.2.3\"\n",
    )
    .expect("a mapping file");
    path
}

/// A directory holding `top` and `nested/leaf.txt`, for the tree mode.
pub(crate) fn skeleton(dir: &Utf8Path) -> Utf8PathBuf {
    let root = dir.join("skeleton");
    std::fs::create_dir_all(root.join("nested")).expect("a tree");
    std::fs::write(root.join("top"), b"top\n").expect("a file");
    std::fs::write(root.join("nested/leaf.txt"), b"leaf\n").expect("a file");
    root
}

/// The same tree as [`skeleton`], gzipped under one leading component.
pub(crate) fn tarball(dir: &Utf8Path) -> Utf8PathBuf {
    let path = dir.join("skeleton-1.2.tar.gz");
    let file = std::fs::File::create(&path).expect("an archive");
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        file,
        flate2::Compression::default(),
    ));
    for (name, contents) in [
        ("skeleton-1.2/top", &b"top\n"[..]),
        ("skeleton-1.2/nested/leaf.txt", &b"leaf\n"[..]),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, name, contents)
            .expect("an archived member");
    }
    builder
        .into_inner()
        .expect("a finished archive")
        .finish()
        .expect("a flushed archive");
    path
}

fn executable(path: &Utf8Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut mode = std::fs::metadata(path)
        .expect("a source file")
        .permissions();
    mode.set_mode(0o755);
    std::fs::set_permissions(path, mode).expect("an executable source file");
}

fn file(contents: &[u8]) -> Entry {
    Entry::File {
        contents: contents.to_vec(),
        executable: false,
    }
}

fn project(dest: &Utf8Path, desired: &Desired) {
    let projection = Projection::new(dest, None).expect("a projection");
    let mut run = projection.begin().expect("a run");
    run.plan(OWNER, desired, PlanOptions::default())
        .expect("a plan");
    run.apply().expect("an applied plan");
}

/// Asserts that terminal-debug output opens only styles the stylesheet
/// declares, closes every one it opens, and gives each one a colour under
/// both terminal themes.
pub(crate) fn assert_tags_declared(case: &str, rendered: &str) {
    let declared = declared_styles();
    let emitted = tags(rendered);
    assert!(!emitted.is_empty(), "the {case} case emitted no style tag");
    let mut unclosed: Vec<String> = Vec::new();
    let mut opened: BTreeSet<String> = BTreeSet::new();
    for tag in emitted {
        if let Some(closing) = tag.strip_prefix('/') {
            assert_eq!(
                unclosed.pop().as_deref(),
                Some(closing),
                "[/{closing}] closes nothing in the {case} case:\n{rendered}"
            );
        } else {
            assert!(
                declared.contains(&tag),
                "the stylesheet declares no [{tag}], emitted in the {case} case"
            );
            opened.insert(tag.clone());
            unclosed.push(tag);
        }
    }
    assert!(
        unclosed.is_empty(),
        "unclosed {unclosed:?} in the {case} case:\n{rendered}"
    );

    let theme = Theme::from_css(STYLESHEET).expect("a stylesheet Standout parses");
    for (mode, chosen) in [
        (ColorMode::Light, styles_under(LIGHT)),
        (ColorMode::Dark, styles_under(DARK)),
    ] {
        let resolved = theme.resolve_styles(Some(mode));
        for style in &opened {
            assert!(
                !resolved.apply_plain(style, "").starts_with(MISSING),
                "[{style}], emitted in the {case} case, resolves to nothing in {mode:?} mode"
            );
            assert!(
                chosen.contains(style),
                "the stylesheet chooses no {mode:?} colour for [{style}], emitted in the {case} case"
            );
        }
    }
}

/// Asserts that terminal output names no style the app's own theme failed to
/// resolve.
pub(crate) fn assert_styles_resolved(case: &str, rendered: &str) {
    assert!(
        !rendered.contains("?]"),
        "a style the app's theme could not resolve reached the {case} case:\n{rendered}"
    );
}

const STYLESHEET: &str = include_str!("styles/proiectio.css");

const LIGHT: &str = "@media (prefers-color-scheme: light)";
const DARK: &str = "@media (prefers-color-scheme: dark)";

fn declared_styles() -> BTreeSet<String> {
    class_names(STYLESHEET.split(LIGHT).next().expect("the base rules"))
}

fn styles_under(query: &str) -> BTreeSet<String> {
    let after = STYLESHEET.split_once(query).expect("a colour-mode block").1;
    class_names(after.split(DARK).next().expect("one block"))
}

fn class_names(css: &str) -> BTreeSet<String> {
    css.lines()
        .filter_map(|line| line.trim().strip_prefix('.'))
        .filter_map(|rule| rule.split([' ', '{', ',']).next())
        .map(str::to_owned)
        .collect()
}

fn tags(rendered: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = rendered;
    while let Some(start) = rest.find('[') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find(']') else { break };
        let tag = &rest[..end];
        if !tag.is_empty()
            && tag
                .trim_start_matches('/')
                .chars()
                .all(|character| character.is_ascii_lowercase() || character == '_')
        {
            found.push(tag.to_owned());
        }
        rest = &rest[end + 1..];
    }
    found
}
