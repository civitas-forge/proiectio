use std::collections::BTreeSet;

use camino::{Utf8Path, Utf8PathBuf};

use super::*;

const DEST: &str = "/projects/dest";

fn dest() -> &'static Utf8Path {
    Utf8Path::new(DEST)
}

#[test]
fn accepted_paths_join_lexically_normalized() {
    let cases: &[(&str, &str)] = &[
        // Plain paths pass through.
        ("x", "x"),
        ("a/b", "a/b"),
        ("a/b/c", "a/b/c"),
        // `..` that stays inside dest is normalized away.
        ("a/../b", "b"),
        ("a/b/../c", "a/c"),
        ("a/b/../../c", "c"),
        ("a/b/c/../../d/../e", "a/e"),
        // Dots inside names are names, not traversal.
        ("..a/b..", "..a/b.."),
        ("...", "..."),
        ("a.b/.hidden", "a.b/.hidden"),
        // The state-dir default name is an ordinary component here; keeping
        // desired paths out of the state dir is plan's job, not this one's.
        (".proiectio/manifest.json", ".proiectio/manifest.json"),
    ];

    for (rel, joined) in cases {
        let got = contained_join(dest(), Utf8Path::new(rel))
            .unwrap_or_else(|error| panic!("{rel}: expected acceptance, got {error}"));
        assert_eq!(got, Utf8PathBuf::from(format!("{DEST}/{joined}")), "{rel}");
    }
}

#[test]
fn escaping_paths_are_refused_with_the_path_verbatim() {
    let cases: &[&str] = &[
        // Absolute.
        "/",
        "/etc/passwd",
        "/a/../b",
        // Windows-style absolute, on every platform.
        "C:/evil",
        "C:\\evil",
        "c:",
        "\\\\server\\share",
        // Backslash anywhere: never a separator we honor, never a filename.
        "..\\..\\escape",
        "a\\b",
        "a/b\\c",
        "a\\../b",
        // `.` components.
        ".",
        "./x",
        "x/.",
        "a/./b",
        // Empty components: doubled, leading, and trailing slashes.
        "",
        "a//b",
        "a/",
        "a/b/",
        // `..` climbing out, at every depth.
        "..",
        "../a",
        "../../a",
        "a/../..",
        "a/../../b",
        "a/b/../../../c",
        "a/b/c/../../../../x",
        "../a/b/c",
        // Normalizing to nothing: the destination itself, not a path in it.
        "a/..",
        "a/b/../..",
    ];

    for rel in cases {
        let error = contained_join(dest(), Utf8Path::new(rel))
            .expect_err(&format!("{rel:?}: expected refusal"));
        assert!(error.is_refusal(), "{rel:?}");
        match error {
            Error::Containment { paths } => {
                assert_eq!(paths, BTreeSet::from([Utf8PathBuf::from(*rel)]), "{rel:?}");
            }
            other => panic!("{rel:?}: expected Containment, got {other}"),
        }
    }
}
