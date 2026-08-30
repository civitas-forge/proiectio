use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};
use serde::ser::{SerializeSeq, SerializeStruct};
use serde::{Serialize, Serializer};

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
    pub verdict: V,
    pub facts: Option<PathFacts>,
}

/// Every path the report classifies, in path order.
///
/// A row states its own path, so no format has to spell a path as a name:
/// `rows` serializes as a sequence of records rather than as a map keyed by
/// path. Spelled as names, paths cost the two formats different things. An XML
/// element name cannot carry a `/`, and the underscore Standout substitutes
/// for one turns `a/b` and `a_b` into one element name, so the second row
/// overwrites the first. A CSV header can carry a `/` and keeps the two apart,
/// but a header spelled from paths is a header that changes with the
/// destination, and nothing can be written to read it. Carried as a value, a
/// path is only ever itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report<V> {
    pub rows: BTreeMap<Utf8PathBuf, Row<V>>,
}

/// One row as it serializes: the path it classifies, then what [`Row`] states
/// about that path.
#[derive(Serialize)]
struct RowRecord<'a, V> {
    path: &'a Utf8PathBuf,
    verdict: &'a V,
    facts: &'a Option<PathFacts>,
}

/// The `rows` sequence, one [`RowRecord`] per entry of the map.
struct Rows<'a, V>(&'a BTreeMap<Utf8PathBuf, Row<V>>);

impl<V: Serialize> Serialize for Rows<'_, V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut rows = serializer.serialize_seq(Some(self.0.len()))?;
        for (path, row) in self.0 {
            rows.serialize_element(&RowRecord {
                path,
                verdict: &row.verdict,
                facts: &row.facts,
            })?;
        }
        rows.end()
    }
}

impl<V: Serialize> Serialize for Report<V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut report = serializer.serialize_struct("Report", 1)?;
        report.serialize_field("rows", &Rows(&self.rows))?;
        report.end()
    }
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
