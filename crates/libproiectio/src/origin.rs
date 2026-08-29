use std::fmt;

use camino::Utf8PathBuf;
use serde::Serialize;

/// Where a desired tree came from, carried on the [`Plan`](crate::Plan) so a
/// refusal can name its source; [`Display`](std::fmt::Display) renders the
/// phrase refusal messages carry, as in `from mapping /etc/skills.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub enum Origin {
    /// A tree the caller computed itself, and every removal; renders as the
    /// empty string.
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
        /// mapping did.
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
