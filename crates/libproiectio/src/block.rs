//! The region mechanics behind [`EntryKind::Block`]: locating a marker line
//! in a container's bytes, splicing a body in, and stripping one out. Every
//! function here is pure over `&[u8]`, and the bytes outside the region are
//! moved by range, never parsed, compared, or pattern-substituted.

use std::ops::Range;

use crate::{BlockFault, EntryKind, Placement};

/// Where a marker occurrence puts the region in a container's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Region {
    /// The marker line and the body together.
    pub(crate) extent: Range<usize>,
    /// The body's extent, inside [`extent`](Self::extent).
    pub(crate) body: Range<usize>,
}

pub(crate) fn block_kind(kind: &EntryKind) -> Option<(&str, Placement)> {
    match kind {
        EntryKind::Block { marker, placement } => Some((marker, *placement)),
        EntryKind::File | EntryKind::Symlink => None,
    }
}

/// The refusal a desired block earns from its marker and body alone.
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

/// Whether `bytes` are empty or end with `\n`.
pub(crate) fn newline_terminated(bytes: &[u8]) -> bool {
    bytes.is_empty() || bytes.ends_with(b"\n")
}

/// The projection's region in `container`: the last marker occurrence under
/// [`Append`](Placement::Append), the first under
/// [`Prepend`](Placement::Prepend); `None` where there is no occurrence.
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
pub(crate) fn occurrence_count(container: &[u8], marker: &str) -> usize {
    occurrences(container, marker).count()
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

/// `author` — a container already [`strip`]ped of any region — with the
/// region under `marker` and `placement` put back at the chosen end.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    First,
    Last,
}

use Which::{First, Last};

/// The first or last marker occurrence in `bytes`, as the offset the marker
/// starts at and the offset just past its line terminator.
fn occurrence(bytes: &[u8], marker: &str, which: Which) -> Option<(usize, usize)> {
    let mut found = occurrences(bytes, marker);
    match which {
        First => found.next(),
        Last => found.last(),
    }
}

/// Every whole-line marker occurrence in `bytes`, in order, each as the
/// offset the marker starts at and the offset past its line terminator. An
/// empty marker yields nothing.
fn occurrences<'a>(bytes: &'a [u8], marker: &'a str) -> impl Iterator<Item = (usize, usize)> + 'a {
    let mut start = Some(0);
    std::iter::from_fn(move || {
        if marker.is_empty() {
            return None;
        }
        loop {
            let here = start?;
            start = bytes[here..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|offset| here + offset + 1);
            if let Some(line_end) = marker_line(bytes, marker.as_bytes(), here) {
                return Some((here, line_end));
            }
        }
    })
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
