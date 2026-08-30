//! What a test of the whole shell needs before it can run one: an isolated
//! harness, and a destination the library itself materialized.

use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};
use libproiectio::{Desired, Entry, PlanOptions, Projection};
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
