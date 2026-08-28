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
        // Dots inside or leading a name are names, not traversal.
        ("..a/b", "..a/b"),
        ("a.b/.hidden", "a.b/.hidden"),
        // Device-name lookalikes that Windows does not reserve.
        ("common/config", "common/config"),
        ("nulled", "nulled"),
        ("com0", "com0"),
        ("lpt10", "lpt10"),
        ("aux2.c", "aux2.c"),
        // The state-dir default name is an ordinary component here; keeping
        // desired paths out of the state dir is plan's job, not this one's.
        (".proiectio/manifest.json", ".proiectio/manifest.json"),
    ];

    for (rel, joined) in cases {
        let got = contained_join(dest(), Utf8Path::new(rel))
            .unwrap_or_else(|error| panic!("{rel}: expected acceptance, got {error}"));
        // Built by pushing components, exactly as the join does, so the
        // expectation holds under Windows separators too.
        let mut want = Utf8PathBuf::from(DEST);
        want.extend(joined.split('/'));
        assert_eq!(got, want, "{rel}");
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
        "C:evil",
        "\\\\server\\share",
        // Drive prefix in a later component: on Windows `Path::push`
        // replaces the accumulated path when handed one of these.
        "a/C:evil",
        "a/../C:/evil",
        "x/c:/y",
        "a/C:",
        // Any colon: an NTFS alternate data stream addresses another
        // file's stream, not a file of this name.
        "victim:stream",
        "ab:c/d",
        // Trailing dot or space: Windows strips them before resolving,
        // so `".. "` kept as a name would climb out there.
        ".. /escape",
        "a/.. ",
        "b..",
        "...",
        "a./b",
        "a /b",
        "name.",
        "name ",
        // Reserved device names, with or without an extension.
        "NUL",
        "nul.txt",
        "a/CON/b",
        "com1",
        "LPT9.log",
        "prn",
        "AUX.c",
        "CONIN$",
        "conout$.txt",
        "COM¹",
        "lpt²",
        "com³.dat",
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

#[test]
fn in_dest_targets_resolve_from_the_links_parent() {
    // (the link's parent, the target as written, where it lands).
    let cases: &[(&str, &str, &str)] = &[
        ("", "shared/rc", "shared/rc"),
        ("nested", "../shared/rc", "shared/rc"),
        ("nested/deep", "../../shared/rc", "shared/rc"),
        ("nested", "sibling", "nested/sibling"),
        ("nested", "sub/../sibling", "nested/sibling"),
        // Spellings a filesystem resolves away — the pointer is content,
        // not a path the projection creates.
        ("", "./shared/rc", "shared/rc"),
        ("", "shared//rc", "shared/rc"),
        ("", "shared/", "shared"),
        ("nested", ".", "nested"),
        ("", "NUL", "NUL"),
        ("", "a:b", "a:b"),
        // Nothing needs to exist at the far end: a dangling pointer is a
        // legal link.
        ("", "not-there/yet", "not-there/yet"),
        // The destination itself.
        ("", ".", ""),
        ("nested", "..", ""),
    ];

    for (parent, target, landing) in cases {
        assert_eq!(
            contained_target(Utf8Path::new(parent), target),
            Some(Utf8PathBuf::from(*landing)),
            "{parent:?} -> {target:?}"
        );
    }
}

#[test]
fn escaping_targets_grade_external() {
    let cases: &[(&str, &str)] = &[
        ("", "/etc/passwd"),
        ("", "/"),
        ("nested", "/etc/passwd"),
        ("", ".."),
        ("", "../outside"),
        ("nested", "../../outside"),
        ("nested/deep", "../../../outside"),
        ("", "a/../../outside"),
        // Backslashes are a separator on one host and a name on another;
        // a projection grades them the same way everywhere.
        ("", "..\\..\\escape"),
        ("", "a\\b"),
    ];

    for (parent, target) in cases {
        assert_eq!(
            contained_target(Utf8Path::new(parent), target),
            None,
            "{parent:?} -> {target:?}"
        );
    }
}
