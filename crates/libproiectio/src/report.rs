use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

use crate::Origin;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum PathShape {
    File { executable: bool },
    Symlink { target: Option<String> },
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PathFacts {
    /// What the row is about on disk; `None` where the verdict decides no
    /// node, as a refusal does.
    pub shape: Option<PathShape>,
    pub owners: BTreeSet<String>,
    pub origin: Option<Origin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row<V> {
    pub facts: Option<PathFacts>,
    pub verdict: V,
}

/// An archive member `strip` left with no path at all, named with the archive
/// that carried it. A member name is unique only within its own archive, so a
/// drop is a record rather than a keyed entry: two archives dropping the same
/// name are two records.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Dropped {
    /// The member's path as its archive spells it, normalized.
    pub member: Utf8PathBuf,
    /// The archive that carried the member.
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report<V> {
    pub rows: BTreeMap<Utf8PathBuf, Row<V>>,
    /// Archive members `strip` erased, which no row can state: they reached
    /// no path in the destination.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub dropped: BTreeSet<Dropped>,
}

impl<V> Default for Report<V> {
    fn default() -> Self {
        Self {
            rows: BTreeMap::new(),
            dropped: BTreeSet::new(),
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
        self.rows.is_empty() && self.dropped.is_empty()
    }
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
