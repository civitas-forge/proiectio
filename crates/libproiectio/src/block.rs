//! The region mechanics behind [`EntryKind::Block`]: locating a marker line
//! in a container's bytes, splicing a body in, and stripping one out.
//!
//! Every function here is pure over `&[u8]`, so the rules
//! [`EntryKind::Block`] states are table-tested with no filesystem. The bytes
//! outside the region are moved by range and never parsed, compared, or
//! passed through a pattern substitution: conda substitutes its block for the
//! literal `__CONDA_REPLACE_ME_123__` and expands the sentinel back, so an rc
//! file containing that string is corrupted and several existing blocks
//! collapse into copies of the newest. A byte-range splice makes that class of
//! outcome impossible rather than unlikely.

use std::ops::Range;

use crate::{BlockFault, EntryKind, Placement};

/// Where a marker occurrence puts the region in a container's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Region {
    /// The region's whole extent — the marker line and the body together.
    /// Stripping this range leaves the author's side exactly.
    pub(crate) extent: Range<usize>,
    /// The body's extent, inside [`extent`](Self::extent).
    pub(crate) body: Range<usize>,
}

/// The marker and placement of a [`Block`](EntryKind::Block) kind; `None` for
/// every other kind.
pub(crate) fn block_kind(kind: &EntryKind) -> Option<(&str, Placement)> {
    match kind {
        EntryKind::Block { marker, placement } => Some((marker, *placement)),
        EntryKind::File | EntryKind::Symlink => None,
    }
}

/// The plan-time refusals a desired block earns from its own fields alone —
/// the marker rules and the body rules of [`EntryKind::Block`]. Cheap enough
/// that both the deciding stage and apply's up-front validation run it.
pub(crate) fn entry_fault(marker: &str, placement: Placement, body: &[u8]) -> Option<BlockFault> {
    if marker.is_empty() {
        return Some(BlockFault::MarkerEmpty);
    }
    if marker.contains(['\n', '\r']) {
        return Some(BlockFault::MarkerNotOneLine);
    }
    if marker.starts_with([' ', '\t']) || marker.ends_with([' ', '\t']) {
        return Some(BlockFault::MarkerEdgeWhitespace);
    }
    if occurrence(body, marker, First).is_some() {
        return Some(BlockFault::BodyCarriesMarker);
    }
    if placement == Placement::Prepend && !newline_terminated(body) {
        return Some(BlockFault::BodyNotNewlineTerminated);
    }
    None
}

/// Whether `bytes` are empty or end with `\n` — what
/// [`Append`](Placement::Append) requires of the author's side and
/// [`Prepend`](Placement::Prepend) of the body, so that the marker line the
/// other side follows with begins at a line start.
pub(crate) fn newline_terminated(bytes: &[u8]) -> bool {
    bytes.is_empty() || bytes.ends_with(b"\n")
}

/// The projection's region in `container`, or `None` where the container
/// holds no marker occurrence.
///
/// The body may carry no line equal to the marker
/// ([`entry_fault`]), so the projection's own marker is the last occurrence
/// under [`Append`](Placement::Append) and the first under
/// [`Prepend`](Placement::Prepend) — every earlier or later one is a line the
/// author wrote.
pub(crate) fn locate(container: &[u8], marker: &str, placement: Placement) -> Option<Region> {
    match placement {
        Placement::Append => {
            let (start, line_end) = occurrence(container, marker, Last)?;
            Some(Region {
                extent: start..container.len(),
                body: line_end..container.len(),
            })
        }
        Placement::Prepend => {
            let (start, line_end) = occurrence(container, marker, First)?;
            Some(Region {
                extent: 0..line_end,
                body: 0..start,
            })
        }
    }
}

/// How many whole-line marker occurrences `container` holds.
///
/// [`locate`] takes the last for [`Append`](Placement::Append) and the first
/// for [`Prepend`](Placement::Prepend), which is the projection's own only
/// while the region reads back as recorded: the body may carry no marker
/// line, so every other occurrence is a line the author wrote outside the
/// region. An author who writes a bare marker line *past* the region's outer
/// edge has written one inside it, and then nothing in the manifest says
/// which occurrence bounds the recorded region — the marker is the whole of
/// its identity. Counting is what lets the deciding stage refuse that rather
/// than strip a range it guessed at.
pub(crate) fn occurrence_count(container: &[u8], marker: &str) -> usize {
    let mut count = 0;
    let mut start = 0;
    if marker.is_empty() {
        return 0;
    }
    loop {
        if marker_line(container, marker.as_bytes(), start).is_some() {
            count += 1;
        }
        match container[start..].iter().position(|byte| *byte == b'\n') {
            Some(offset) => start += offset + 1,
            None => return count,
        }
    }
}

/// The container with `region` removed: the author's side, byte for byte.
pub(crate) fn strip(container: &[u8], region: Option<&Region>) -> Vec<u8> {
    match region {
        None => container.to_vec(),
        Some(region) => {
            let mut author = container[..region.extent.start].to_vec();
            author.extend_from_slice(&container[region.extent.end..]);
            author
        }
    }
}

/// The whole of what a block write puts on disk: `author` — the container
/// with any region of the projection's already [`strip`]ped out — with the
/// region under `marker` and `placement` put back at the chosen end.
///
/// The author's bytes are copied through by range, never parsed, compared, or
/// substituted, so writing a region cannot disturb them however they are
/// spelled.
pub(crate) fn splice(author: &[u8], marker: &str, placement: Placement, body: &[u8]) -> Vec<u8> {
    let mut spliced = Vec::with_capacity(author.len() + marker.len() + 1 + body.len());
    let (before, after): (&[u8], &[u8]) = match placement {
        Placement::Append => (author, body),
        Placement::Prepend => (body, author),
    };
    spliced.extend_from_slice(before);
    spliced.extend_from_slice(marker.as_bytes());
    spliced.push(b'\n');
    spliced.extend_from_slice(after);
    spliced
}

/// Which marker occurrence [`occurrence`] answers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    First,
    Last,
}

use Which::{First, Last};

/// The first or last marker occurrence in `bytes`, as the offset the marker
/// starts at and the offset just past its line terminator.
///
/// An occurrence is a whole line: anchored at a line start — offset zero, or
/// the byte after a `\n` — matched byte-exact, and terminated by `\n`,
/// `\r\n`, or the end of `bytes`. So an indented or quoted line carrying the
/// marker text is not one.
fn occurrence(bytes: &[u8], marker: &str, which: Which) -> Option<(usize, usize)> {
    if marker.is_empty() {
        // Every line start would match, which is no occurrence at all. Such a
        // marker is refused before it can be written; a manifest this crate
        // never wrote can still carry one.
        return None;
    }
    let mut found = None;
    let mut start = 0;
    loop {
        if let Some(line_end) = marker_line(bytes, marker.as_bytes(), start) {
            found = Some((start, line_end));
            if which == First {
                return found;
            }
        }
        match bytes[start..].iter().position(|byte| *byte == b'\n') {
            Some(offset) => start += offset + 1,
            None => return found,
        }
    }
}

/// The offset just past the line terminator when `start` — known to be a line
/// start — begins a marker occurrence; `None` when it does not.
fn marker_line(bytes: &[u8], marker: &[u8], start: usize) -> Option<usize> {
    let after = start + marker.len();
    if bytes.get(start..after)? != marker {
        return None;
    }
    match bytes.get(after) {
        None => Some(after),
        Some(b'\n') => Some(after + 1),
        Some(b'\r') if bytes.get(after + 1) == Some(&b'\n') => Some(after + 2),
        Some(_) => None,
    }
}

#[cfg(test)]
#[path = "block_tests.rs"]
mod tests;
