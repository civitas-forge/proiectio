use std::cell::Cell;
use std::io::{self, Read};

use serde::Serialize;

/// The bounds one load holds its input to.
///
/// A load reads bytes a caller did not write — an archive's members, a source
/// tree's files, a mapping file and the files it names — and holds every one
/// of them in memory at once, so a single source can ask a run for as much
/// memory as it likes. [`max_source_bytes`](Self::max_source_bytes) is the
/// ceiling on that, spent once per load and shared by every source the load
/// touches.
///
/// A caller starts from [`Limits::default`] and names the bounds it wants
/// changed, rather than spelling a struct literal: the crate holds hard-coded
/// bounds on an archive's member count and its nesting depth that a later
/// version may make configurable here, and a literal naming every field would
/// stop compiling the day one of them arrives.
///
/// ```
/// # use libproiectio::Limits;
/// let tight = Limits::default().with_max_source_bytes(1 << 20);
/// assert_eq!(Limits::default().max_source_bytes, Limits::DEFAULT_MAX_SOURCE_BYTES);
/// assert!(tight.max_source_bytes < Limits::default().max_source_bytes);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Limits {
    /// How many bytes one load may read into memory, summed over every source
    /// it reads: an archive's decompressed stream and members, a walked
    /// tree's files, the loose files named on their own, a mapping file's own
    /// text, and each file a mapping's `source` names. The compressed size of
    /// an archive on disk is not what is counted — what it expands to is.
    ///
    /// A zip is the one exception, and it is weighed both ways: a zip's
    /// index is parsed whole before any member is read, so the zip file
    /// itself has to fit this bound as well as what it expands to.
    pub max_source_bytes: u64,
}

impl Limits {
    /// The bound [`Limits::default`] carries: 500 MiB.
    pub const DEFAULT_MAX_SOURCE_BYTES: u64 = 500 << 20;

    /// The same bounds with [`max_source_bytes`](Self::max_source_bytes) set
    /// to `bytes`.
    #[must_use]
    pub fn with_max_source_bytes(mut self, bytes: u64) -> Self {
        self.max_source_bytes = bytes;
        self
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_source_bytes: Self::DEFAULT_MAX_SOURCE_BYTES,
        }
    }
}

/// What is left of one load's [`Limits::max_source_bytes`], and whether
/// something ran it out.
pub(crate) struct Budget {
    limit: u64,
    remaining: Cell<u64>,
    exhausted: Cell<bool>,
}

impl Budget {
    pub(crate) fn new(limits: Limits) -> Self {
        Self {
            limit: limits.max_source_bytes,
            remaining: Cell::new(limits.max_source_bytes),
            exhausted: Cell::new(false),
        }
    }

    /// The bound this budget was opened at, which is what a diagnostic names.
    pub(crate) fn limit(&self) -> u64 {
        self.limit
    }

    pub(crate) fn remaining(&self) -> u64 {
        self.remaining.get()
    }

    pub(crate) fn exhausted(&self) -> bool {
        self.exhausted.get()
    }

    pub(crate) fn spend(&self, bytes: u64) -> bool {
        match self.remaining.get().checked_sub(bytes) {
            Some(left) => {
                self.remaining.set(left);
                true
            }
            None => {
                self.exhausted.set(true);
                false
            }
        }
    }

    /// Reads all of `reader` into a fresh buffer, taking at most one byte
    /// past what is left and spending what it read; a size the source
    /// declares never sizes the buffer. `Ok(None)` is the budget run out.
    pub(crate) fn read_to_end(&self, reader: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
        let mut contents = Vec::new();
        let read = reader
            .take(self.remaining.get().saturating_add(1))
            .read_to_end(&mut contents)?;
        if self.spend(read as u64) {
            Ok(Some(contents))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
#[path = "limits_tests.rs"]
mod tests;
