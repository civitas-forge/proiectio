use std::fmt;

use camino::Utf8PathBuf;
use serde::Serialize;

/// Where a desired tree came from, carried so a refusal can name it.
///
/// A member path is not enough to act on: `refusing paths that violate
/// containment: ../etc/passwd` says which path the projection declined and
/// nothing about which file to go and edit. An origin travels with the
/// [`Plan`](crate::Plan) rather than being attached by whichever loader
/// built the tree, so a refusal the deciding stage produces names the source
/// as well as one produced while parsing it, and every source names itself
/// the same way.
///
/// It is carried by the four refusals whose offending value is a path or a
/// pointer the source chose: [`Error::Containment`](crate::Error::Containment),
/// [`Error::TreeConflict`](crate::Error::TreeConflict),
/// [`Error::ExternalTarget`](crate::Error::ExternalTarget) and
/// [`Error::InvalidTarget`](crate::Error::InvalidTarget).
///
/// [`Display`](std::fmt::Display) renders the phrase those messages carry —
/// `from mapping /etc/harness/skills.toml` — and renders
/// [`Caller`](Origin::Caller) as the empty string, so a tree the caller
/// computed reads as a plain refusal instead of apologising for having no
/// source to name.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub enum Origin {
    /// A tree the caller computed itself, and every removal — nothing to
    /// name. Renders as nothing.
    #[default]
    Caller,
    /// A TOML mapping file, at this absolute path
    /// ([`load_mapping`](crate::load_mapping)).
    Mapping {
        /// The mapping file's location.
        path: Utf8PathBuf,
    },
    /// A walked source directory, at this absolute path
    /// ([`load_tree`](crate::load_tree)).
    Tree {
        /// The source directory's location.
        path: Utf8PathBuf,
    },
    /// An expanded archive ([`load_archive`](crate::load_archive)).
    Archive {
        /// The archive's location.
        path: Utf8PathBuf,
        /// The mapping whose `[archives]` table named this archive, where a
        /// mapping did. One mapping may name several archives, and a member
        /// path says neither which archive to open nor which line to edit.
        via: Option<Utf8PathBuf>,
    },
    /// Files named one at a time on the invocation rather than through a
    /// mapping, a tree, or an archive.
    Files,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::Caller => Ok(()),
            Origin::Mapping { path } => write!(f, "from mapping {path}"),
            Origin::Tree { path } => write!(f, "from tree {path}"),
            Origin::Archive { path, via: None } => write!(f, "from archive {path}"),
            Origin::Archive {
                path,
                via: Some(mapping),
            } => write!(f, "from archive {path}, named by mapping {mapping}"),
            Origin::Files => f.write_str("from individually named files"),
        }
    }
}

#[cfg(test)]
#[path = "origin_tests.rs"]
mod tests;
