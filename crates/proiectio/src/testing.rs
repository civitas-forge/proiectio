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

/// One file under `x/`, named the way `tar czf dot.tgz -C skel .` names its
/// members: with a leading `./`.
pub(crate) fn dot_tarball(dir: &Utf8Path) -> Utf8PathBuf {
    use std::io::Write;

    let path = dir.join("dot.tgz");
    let mut bytes = Vec::new();
    ustar_member(&mut bytes, "./", DIRECTORY, 0o755, b"");
    ustar_member(&mut bytes, "./x/", DIRECTORY, 0o755, b"");
    ustar_member(&mut bytes, "./x/a.txt", REGULAR, 0o644, b"a\n");
    bytes.extend_from_slice(&[0u8; 1024]);
    assert_eq!(
        member_names(&bytes),
        ["./", "./x/", "./x/a.txt"],
        "the fixture no longer spells its members with a leading ./"
    );
    let mut encoder = flate2::write::GzEncoder::new(
        std::fs::File::create(&path).expect("an archive"),
        flate2::Compression::default(),
    );
    encoder.write_all(&bytes).expect("a written archive");
    encoder.finish().expect("a flushed archive");
    path
}

/// The same tree as [`tarball`], as stock macOS `tar` writes it: an
/// AppleDouble `._skeleton-1.2` sibling at depth 1, which `--strip 1` leaves
/// with no path at all.
pub(crate) fn appledouble_tarball(dir: &Utf8Path) -> Utf8PathBuf {
    use std::io::Write;

    let path = dir.join("appledouble.tgz");
    let mut bytes = Vec::new();
    ustar_member(
        &mut bytes,
        "._skeleton-1.2",
        REGULAR,
        0o644,
        b"Mac OS X\0\0\0",
    );
    ustar_member(&mut bytes, "skeleton-1.2/", DIRECTORY, 0o755, b"");
    ustar_member(&mut bytes, "skeleton-1.2/top", REGULAR, 0o644, b"top\n");
    bytes.extend_from_slice(&[0u8; 1024]);
    let mut encoder = flate2::write::GzEncoder::new(
        std::fs::File::create(&path).expect("an archive"),
        flate2::Compression::default(),
    );
    encoder.write_all(&bytes).expect("a written archive");
    encoder.finish().expect("a flushed archive");
    path
}

const REGULAR: u8 = b'0';
const DIRECTORY: u8 = b'5';

/// Appends one member under a hand-written ustar header, which spells the
/// name byte for byte where `tar::Builder` would normalize it.
fn ustar_member(out: &mut Vec<u8>, name: &str, kind: u8, mode: u32, body: &[u8]) {
    assert!(name.len() <= 100, "{name} needs a ustar name prefix");
    let mut header = [0u8; 512];
    header[..name.len()].copy_from_slice(name.as_bytes());
    octal(&mut header[100..108], u64::from(mode), 7);
    octal(&mut header[108..116], 0, 7);
    octal(&mut header[116..124], 0, 7);
    octal(&mut header[124..136], body.len() as u64, 11);
    octal(&mut header[136..148], 0, 11);
    header[156] = kind;
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    header[148..156].copy_from_slice(b"        ");
    let sum: u32 = header.iter().map(|&byte| u32::from(byte)).sum();
    octal(&mut header[148..156], u64::from(sum), 6);
    header[154] = 0;
    header[155] = b' ';
    out.extend_from_slice(&header);
    out.extend_from_slice(body);
    out.extend_from_slice(&vec![0u8; (512 - body.len() % 512) % 512]);
}

/// Writes `value` as a NUL-terminated octal string of `digits` digits.
fn octal(field: &mut [u8], value: u64, digits: usize) {
    let text = format!("{value:0digits$o}");
    field[..digits].copy_from_slice(text.as_bytes());
    field[digits] = 0;
}

/// The member names an archive carries, as its headers spell them.
fn member_names(tar: &[u8]) -> Vec<String> {
    let mut archive = tar::Archive::new(tar);
    archive
        .entries()
        .expect("the archive's members")
        .map(|entry| {
            String::from_utf8(entry.expect("a member").path_bytes().to_vec())
                .expect("a utf-8 member name")
        })
        .collect()
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
