use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

use crate::{EntryKind, Origin};

/// The shape a row states for a node the projection knows by its recorded
/// kind rather than by an [`Entry`](crate::Entry): a manifest entry or an
/// action's expected signature. Both stages spell it here, so a plan's row
/// and the apply row for the same path cannot drift apart.
pub(crate) fn recorded_shape(kind: &EntryKind, executable: bool) -> PathShape {
    match kind {
        EntryKind::File => PathShape::File { executable },
        EntryKind::Symlink => PathShape::Symlink { target: None },
        EntryKind::Block { .. } => PathShape::Block,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum PathShape {
    File { executable: bool },
    Symlink { target: Option<String> },
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PathFacts {
    /// What the row is about on disk: the entry a write carries, the node a
    /// removal expects, or the manifest's own record where the verdict
    /// decides no node itself. `None` where nothing names a shape — a
    /// refusal, or a path no manifest entry records.
    pub shape: Option<PathShape>,
    pub owners: BTreeSet<String>,
    pub origin: Option<Origin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row<V> {
    pub facts: Option<PathFacts>,
    pub verdict: V,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report<V> {
    pub rows: BTreeMap<Utf8PathBuf, Row<V>>,
}

impl<V> Default for Report<V> {
    fn default() -> Self {
        Self {
            rows: BTreeMap::new(),
        }
    }
}

impl<V: Ord + Clone> Report<V> {
    pub fn summary(&self) -> BTreeMap<V, usize> {
        let mut counts = BTreeMap::new();
        for row in self.rows.values() {
            *counts.entry(row.verdict.clone()).or_insert(0) += 1;
        }
        counts
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Utf8Path, &Row<V>)> {
        self.rows.iter().map(|(path, row)| (path.as_path(), row))
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
