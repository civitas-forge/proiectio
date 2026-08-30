//! The clapfig configuration schema and the one builder every consumer goes through.

use camino::Utf8PathBuf;
use clapfig::error::ClapfigError;
use clapfig::runtime::{LeafType, Shape};
use clapfig::{Clapfig, Schema, SearchPath, TypedBuilder, UnknownKeyDecision};
use libproiectio::Error;
use serde::{Deserialize, Serialize};

/// Projection settings as `proiectio.toml` declares them.
#[derive(Schema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProiectioConfig {
    /// The manifest owner a run records its entries under.
    /// Default for `--owner`; the flag always wins.
    #[clapfig(default = "default")]
    pub(crate) owner: String,
}

const APP: &str = "proiectio";
const FILE: &str = "proiectio.toml";
const COMMENT_KEY_PREFIX: &str = "//";

pub(crate) fn builder() -> TypedBuilder<ProiectioConfig> {
    Clapfig::typed::<ProiectioConfig>()
        .app_name(APP)
        .persist_scope("user", SearchPath::Platform)
        .on_unknown_key(|key| {
            if key.leaf.starts_with(COMMENT_KEY_PREFIX) {
                UnknownKeyDecision::Accept
            } else {
                UnknownKeyDecision::Reject
            }
        })
}

/// The file an edit through the `user` scope lands in, and whether the edit
/// wrote it.
pub(crate) struct Edit {
    pub(crate) path: Utf8PathBuf,
    pub(crate) wrote: bool,
}

/// What a `set` or `unset` through the `user` scope just did, read after
/// clapfig persisted it: a set always leaves the file behind, and an unset
/// with no file to remove the key from writes nothing.
pub(crate) fn persisted_edit() -> Result<Edit, Error> {
    let path = user_config_path()?;
    Ok(Edit {
        wrote: path.is_file(),
        path,
    })
}

/// A comment key is a note the file's author left, not a setting: the loader
/// accepts one wherever `config schema` allowlists it, and a listing of the
/// configuration leaves it where the writer put it.
pub(crate) fn is_comment_key(key: &str) -> bool {
    key.split('.')
        .any(|segment| segment.starts_with(COMMENT_KEY_PREFIX))
}

/// The file the `user` scope persists to, resolved the way clapfig resolves
/// [`SearchPath::Platform`] itself, which is the scope the builder registers.
fn user_config_path() -> Result<Utf8PathBuf, Error> {
    let dirs = directories::ProjectDirs::from("", "", APP).ok_or_else(|| Error::Io {
        path: Utf8PathBuf::from(FILE),
        source: std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the platform configuration directory does not resolve",
        ),
    })?;
    Utf8PathBuf::from_path_buf(dirs.config_dir().join(FILE)).map_err(|path| Error::PathNotUtf8 {
        path: path.to_string_lossy().into_owned(),
    })
}

pub(crate) fn require_key(key: &str) -> Result<(), ClapfigError> {
    let shape = ProiectioConfig::shape();
    if clapfig::meta::doc_for_shape(&shape, key).is_some() {
        return Ok(());
    }
    Err(ClapfigError::KeyNotFound {
        key: key.to_owned(),
        suggestion: clapfig::meta::nearest_key_shape(&shape, key, false),
    })
}

pub(crate) fn leaf_type(key: &str) -> Option<LeafType> {
    let mut shape = ProiectioConfig::shape();
    for segment in key.split('.') {
        let Shape::Object(schema) = shape else {
            return None;
        };
        shape = schema
            .fields
            .iter()
            .find(|field| field.name == segment)?
            .field
            .clone();
    }
    match shape {
        Shape::Leaf(leaf) => Some(leaf.ty),
        _ => None,
    }
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
