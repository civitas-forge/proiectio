use std::collections::{BTreeMap, BTreeSet};

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
        // A NUL terminates a pathname rather than appearing in one, so a
        // path carrying one names nothing that could be written.
        "a\u{0}b",
        "a/b\u{0}",
        "\u{0}",
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
            Error::Containment { paths, .. } => {
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
        // Colon shapes that name something under the destination rather
        // than a location outside it: a device name is an ordinary name in
        // a pointer, and an NTFS stream addresses a sibling's stream.
        ("", "NUL", "NUL"),
        ("", "victim:stream", "victim:stream"),
        ("nested", "ab:c/d", "nested/ab:c/d"),
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
        // A leading drive designator names a place on that drive, with a
        // slash or without; graded on every host, like the backslash below.
        ("", "C:/escape"),
        ("", "C:escape"),
        ("", "c:"),
        ("nested", "a:b"),
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

// --- chain resolution: grading through the destination's own links ---

/// A destination described by its symlinks alone: each path mapped to the
/// target it points at, or to `None` for a link whose on-disk target is not
/// UTF-8. Every other path is a hop the chain stops at, which is all
/// resolution asks about — no filesystem, so the whole rule stays a table.
type Links<'a> = &'a [(&'a str, Option<&'a str>)];

/// The chain resolution over the destination `links` describes.
fn resolve(parent: &str, target: &str, links: Links<'_>) -> Option<Utf8PathBuf> {
    let links: BTreeMap<Utf8PathBuf, Option<String>> = links
        .iter()
        .map(|(path, target)| {
            (
                Utf8PathBuf::from(*path),
                target.map(|target| target.to_owned()),
            )
        })
        .collect();
    let landing = contained_target_chain(Utf8Path::new(parent), target, |path| {
        Ok::<Hop, std::convert::Infallible>(match links.get(path) {
            Some(Some(target)) => Hop::Link(target.clone()),
            Some(None) => Hop::Unresolvable,
            None => Hop::Terminal,
        })
    });
    match landing {
        Ok(landing) => landing,
        Err(never) => match never {},
    }
}

#[test]
fn a_chain_of_in_dest_links_resolves_to_where_the_pointer_lands() {
    // (the link's parent, the target as written, the destination's links,
    // where the pointer lands).
    let cases: &[(&str, &str, Links, &str)] = &[
        // The ordinary chain: `shared -> real` under `rc -> shared/rc`.
        // Nothing here needs the external-target permission.
        ("", "shared/rc", &[("shared", Some("real"))], "real/rc"),
        // Several hops, and a hop resolved from the link's own parent.
        (
            "",
            "a/leaf",
            &[("a", Some("b")), ("b", Some("c"))],
            "c/leaf",
        ),
        (
            "",
            "deep/pivot/leaf",
            &[("deep/pivot", Some("side"))],
            "deep/side/leaf",
        ),
        (
            "nested",
            "../shared/rc",
            &[("shared", Some("real"))],
            "real/rc",
        ),
        // `..` pops what the chain walked, not what the target spelled:
        // after `deep/pivot` resolves to `deep/side`, `..` is `deep`.
        ("", "deep/pivot/..", &[("deep/pivot", Some("side"))], "deep"),
        // A hop pointing at nothing keeps the chain in-dest: a pointer to
        // nothing is still a pointer inside the destination.
        ("", "shared/rc", &[("shared", Some("gone"))], "gone/rc"),
        ("", "not-there/yet", &[], "not-there/yet"),
        // A destination with no links at all grades exactly as the lexical
        // resolution does.
        ("nested", "sub/../sibling", &[], "nested/sibling"),
    ];

    for (parent, target, links, landing) in cases {
        assert_eq!(
            resolve(parent, target, links),
            Some(Utf8PathBuf::from(*landing)),
            "{parent:?} -> {target:?} through {links:?}"
        );
    }
}

#[test]
fn a_chain_reaching_outside_the_destination_grades_external() {
    // (the link's parent, the target as written, the destination's links).
    let cases: &[(&str, &str, Links)] = &[
        // The pivot case: dest holds `pivot -> /etc`, so `pivot/passwd`
        // dereferences to /etc/passwd.
        ("", "pivot/passwd", &[("pivot", Some("/etc"))]),
        // The same hop, escaped through a `..` a lexical resolution would
        // have popped before ever looking at `pivot`.
        ("", "pivot/../passwd", &[("pivot", Some("/etc"))]),
        // A hop climbing out, from its own parent rather than the link's.
        ("", "pivot/x", &[("pivot", Some("../outside"))]),
        (
            "nested",
            "pivot",
            &[("nested/pivot", Some("../../outside"))],
        ),
        // A hop far along the chain.
        ("", "a/leaf", &[("a", Some("b")), ("b", Some("/etc"))]),
        // The two spellings graded external on every host apply to a
        // followed link's target exactly as to the target as written.
        ("", "pivot/x", &[("pivot", Some("C:/escape"))]),
        ("", "pivot/x", &[("pivot", Some("..\\..\\escape"))]),
        // A hop nothing can resolve: the on-disk target is not UTF-8, so
        // no verdict about where the chain lands is available.
        ("", "pivot/passwd", &[("pivot", None)]),
    ];

    for (parent, target, links) in cases {
        assert_eq!(
            resolve(parent, target, links),
            None,
            "{parent:?} -> {target:?} through {links:?}"
        );
    }
}

#[test]
fn a_cycle_of_links_grades_external_rather_than_looping() {
    // The guard is a visited set of the links followed, the shape apply's
    // no-follow walk carries. It terminates the resolution instead of
    // chasing the cycle, and the verdict is external: a chain that never
    // lands cannot be said to land inside the destination.
    assert_eq!(resolve("", "self", &[("self", Some("self"))]), None);
    assert_eq!(
        resolve("", "l1", &[("l1", Some("l2")), ("l2", Some("l1"))]),
        None
    );
    assert_eq!(
        resolve("", "l1/leaf", &[("l1", Some("l2")), ("l2", Some("l1"))]),
        None
    );
    // Blunter than a kernel's ELOOP counter, deliberately: a target that
    // traverses one link twice without cycling grades external too.
    assert_eq!(resolve("", "s/x/../../s/y", &[("s", Some("real"))]), None);
}

#[test]
fn the_lexical_grading_is_the_chain_over_a_destination_holding_no_links() {
    for (parent, target) in [
        ("", "shared/rc"),
        ("nested", "../shared/rc"),
        ("", "/etc/passwd"),
        ("", "../outside"),
        ("", "C:/escape"),
    ] {
        assert_eq!(
            contained_target(Utf8Path::new(parent), target),
            resolve(parent, target, &[]),
            "{parent:?} -> {target:?}"
        );
    }
}

#[test]
fn only_the_two_strings_that_are_not_pathnames_fail_the_pathname_check() {
    // Grading asks where a target lands; this asks first whether there is
    // a path to land anywhere. Only these two are not pathnames on any
    // host — a target the filesystem rejects for its length is nothing
    // lexical rules can see.
    assert!(!is_pathname(""));
    assert!(!is_pathname("\0"));
    assert!(!is_pathname("shared/\0rc"));
    for target in ["shared/rc", "../outside", "/etc/passwd", "C:/escape", "."] {
        assert!(is_pathname(target), "{target:?}");
    }
}
