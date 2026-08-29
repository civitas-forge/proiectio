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
    pub shape: PathShape,
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
