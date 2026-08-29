//! Table tests for the region mechanics — pure over bytes, no filesystem
//! (`docs/implementation.lex` section 2).

use super::*;
use Placement::{Append, Prepend};

const MARKER: &str = "# proiectio";

/// The body a located region names, for readability in the tables below.
fn located(container: &str, marker: &str, placement: Placement) -> Option<String> {
    let region = locate(container.as_bytes(), marker, placement)?;
    Some(String::from_utf8(container.as_bytes()[region.body].to_vec()).expect("UTF-8 body"))
}

/// The author's side left after the region is stripped.
fn stripped(container: &str, marker: &str, placement: Placement) -> String {
    let region = locate(container.as_bytes(), marker, placement);
    String::from_utf8(strip(container.as_bytes(), region.as_ref())).expect("UTF-8 author")
}

#[test]
fn a_marker_occurrence_is_a_whole_line() {
    let cases: &[(&str, Placement, Option<&str>)] = &[
        // Terminated by a newline, by CRLF, and by the end of the file.
        ("keep\n# proiectio\nbody\n", Append, Some("body\n")),
        ("keep\n# proiectio\r\nbody\n", Append, Some("body\n")),
        ("keep\n# proiectio", Append, Some("")),
        // Anchored at a line start: an indented or quoted line carrying the
        // marker text is not an occurrence, so a container may discuss it.
        ("keep\n  # proiectio\nbody\n", Append, None),
        ("keep\n\"# proiectio\"\nbody\n", Append, None),
        // Matched byte-exact: a longer line that starts with the marker is
        // not the marker.
        ("keep\n# proiectio extra\nbody\n", Append, None),
        // The first line of the file is a line start.
        ("# proiectio\nbody\n", Append, Some("body\n")),
        // Trailing whitespace makes a different line.
        ("keep\n# proiectio \nbody\n", Append, None),
    ];
    for (container, placement, want) in cases {
        assert_eq!(
            located(container, MARKER, *placement).as_deref(),
            *want,
            "{container:?}"
        );
    }
}

#[test]
fn append_takes_the_last_occurrence_and_prepend_the_first() {
    // The body may carry no marker line, so every occurrence but the
    // projection's own is one the author wrote.
    let container = "# proiectio\nauthor\n# proiectio\nours\n";
    assert_eq!(
        located(container, MARKER, Append).as_deref(),
        Some("ours\n")
    );
    assert_eq!(located(container, MARKER, Prepend).as_deref(), Some(""));
    assert_eq!(
        stripped(container, MARKER, Prepend),
        "author\n# proiectio\nours\n"
    );
}

#[test]
fn stripping_leaves_the_author_byte_for_byte() {
    let cases: &[(&str, Placement, &str)] = &[
        ("author\n# proiectio\nbody\n", Append, "author\n"),
        ("body\n# proiectio\nauthor\n", Prepend, "author\n"),
        // A CRLF marker terminator belongs to the region, not the author.
        ("body\n# proiectio\r\nauthor\n", Prepend, "author\n"),
        // Stripping may empty the container; the container still stands.
        ("# proiectio\nbody\n", Append, ""),
        ("# proiectio\n", Prepend, ""),
        // No occurrence: nothing is stripped.
        ("author only\n", Append, "author only\n"),
    ];
    for (container, placement, want) in cases {
        assert_eq!(
            stripped(container, MARKER, *placement),
            *want,
            "{container:?}"
        );
    }
}

#[test]
fn splicing_puts_the_region_at_the_chosen_end() {
    assert_eq!(
        splice(b"author\n", MARKER, Append, b"body\n"),
        b"author\n# proiectio\nbody\n".to_vec()
    );
    assert_eq!(
        splice(b"author\n", MARKER, Prepend, b"body\n"),
        b"body\n# proiectio\nauthor\n".to_vec()
    );
    // An empty author on either side needs no newline of its own.
    assert_eq!(
        splice(b"", MARKER, Append, b"body\n"),
        b"# proiectio\nbody\n".to_vec()
    );
}

#[test]
fn a_spliced_region_reads_back_and_strips_back() {
    for placement in [Append, Prepend] {
        let author = "author line\nsecond\n";
        let spliced = splice(author.as_bytes(), MARKER, placement, b"body\n");
        let spliced = String::from_utf8(spliced).expect("UTF-8");
        assert_eq!(
            located(&spliced, MARKER, placement).as_deref(),
            Some("body\n"),
            "{placement:?}"
        );
        assert_eq!(
            stripped(&spliced, MARKER, placement),
            author,
            "{placement:?}"
        );
    }
}

#[test]
fn the_marker_rules_refuse_at_plan_time() {
    let cases: &[(&str, Placement, &[u8], Option<BlockFault>)] = &[
        ("", Append, b"body\n", Some(BlockFault::MarkerEmpty)),
        (
            "# one\n# two",
            Append,
            b"body\n",
            Some(BlockFault::MarkerNotOneLine),
        ),
        (
            "# one\r",
            Append,
            b"body\n",
            Some(BlockFault::MarkerNotOneLine),
        ),
        // A formatter stripping trailing whitespace would rewrite this marker
        // into one no later read finds.
        (
            "# proiectio ",
            Append,
            b"body\n",
            Some(BlockFault::MarkerEdgeWhitespace),
        ),
        (
            "\t# proiectio",
            Append,
            b"body\n",
            Some(BlockFault::MarkerEdgeWhitespace),
        ),
        // A body that carries the marker writes a container nothing can read
        // back.
        (
            MARKER,
            Append,
            b"a\n# proiectio\nb\n",
            Some(BlockFault::BodyCarriesMarker),
        ),
        // ... as a whole line only: an indented mention is fine.
        (MARKER, Append, b"a\n  # proiectio\nb\n", None),
        // Prepend needs the marker line to start a line, so the body ends
        // with one.
        (
            MARKER,
            Prepend,
            b"body",
            Some(BlockFault::BodyNotNewlineTerminated),
        ),
        (MARKER, Prepend, b"body\n", None),
        (MARKER, Prepend, b"", None),
        // Append puts the body last, so its terminator is the caller's.
        (MARKER, Append, b"body", None),
    ];
    for (marker, placement, body, want) in cases {
        assert_eq!(
            entry_fault(marker, *placement, body),
            *want,
            "{marker:?} {placement:?} {body:?}"
        );
    }
}

#[test]
fn occurrences_are_counted_by_the_same_whole_line_rule() {
    let cases: &[(&str, usize)] = &[
        ("author only\n", 0),
        ("author\n# proiectio\nbody\n", 1),
        // The marker terminated by the end of the file counts.
        ("author\n# proiectio", 1),
        // Two bare lines: which one bounds the region is no longer knowable.
        ("# proiectio\nauthor\n# proiectio\nours\n", 2),
        // Indented, quoted and longer lines are not occurrences, so a
        // container discussing its marker keeps its region identifiable.
        (
            "  # proiectio\n\"# proiectio\"\n# proiectio extra\n# proiectio\nours\n",
            1,
        ),
    ];
    for (container, want) in cases {
        assert_eq!(
            occurrence_count(container.as_bytes(), MARKER),
            *want,
            "{container:?}"
        );
    }
    // An empty marker would otherwise count every line start.
    assert_eq!(occurrence_count(b"a\nb\n", ""), 0);
}

#[test]
fn newline_termination_is_emptiness_or_a_trailing_newline() {
    assert!(newline_terminated(b""));
    assert!(newline_terminated(b"a\n"));
    assert!(!newline_terminated(b"a"));
    assert!(!newline_terminated(b"a\r\n\nb"));
}

#[test]
fn an_empty_marker_locates_nothing() {
    // Refused before it can be written; a manifest this crate never wrote can
    // still carry one, and it must not match every line start.
    assert_eq!(located("author\n", "", Append), None);
}
