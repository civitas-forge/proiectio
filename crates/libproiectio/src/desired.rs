use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};

use crate::{Entry, Origin};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Desired {
    entries: BTreeMap<Utf8PathBuf, (Entry, Origin)>,
}

impl Desired {
    pub fn new() -> Self {
        Desired::default()
    }

    pub fn from_caller(entries: BTreeMap<Utf8PathBuf, Entry>) -> Self {
        Desired {
            entries: entries
                .into_iter()
                .map(|(path, entry)| (path, (entry, Origin::Caller)))
                .collect(),
        }
    }

    pub fn from_source(entries: BTreeMap<Utf8PathBuf, Entry>, origin: Origin) -> Self {
        Desired {
            entries: entries
                .into_iter()
                .map(|(path, entry)| (path, (entry, origin.clone())))
                .collect(),
        }
    }

    pub fn insert(&mut self, path: Utf8PathBuf, entry: Entry, origin: Origin) -> bool {
        self.entries.insert(path, (entry, origin)).is_none()
    }

    pub fn get(&self, path: &Utf8Path) -> Option<&Entry> {
        self.entries.get(path).map(|(entry, _)| entry)
    }

    pub fn origin(&self, path: &Utf8Path) -> Origin {
        self.entries
            .get(path)
            .map_or(Origin::Caller, |(_, origin)| origin.clone())
    }

    pub fn contains_key(&self, path: &Utf8Path) -> bool {
        self.entries.contains_key(path)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Utf8PathBuf, &Entry)> {
        self.entries.iter().map(|(path, (entry, _))| (path, entry))
    }

    pub fn sources(&self) -> impl Iterator<Item = (&Utf8PathBuf, &Origin)> {
        self.entries
            .iter()
            .map(|(path, (_, origin))| (path, origin))
    }
}

impl FromIterator<(Utf8PathBuf, Entry)> for Desired {
    fn from_iter<I: IntoIterator<Item = (Utf8PathBuf, Entry)>>(iter: I) -> Self {
        Desired::from_caller(iter.into_iter().collect())
    }
}
