//! The clapfig configuration schema and the one builder every consumer goes through.

use std::path::PathBuf;

use camino::Utf8PathBuf;
use clapfig::error::ClapfigError;
use clapfig::runtime::{LeafType, Shape};
use clapfig::{Clapfig, Schema, SearchPath, TypedBuilder, UnknownKeyDecision};
use libproiectio::Error;
use serde::{Deserialize, Serialize};

use crate::exit;

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

pub(crate) fn builder() -> Result<TypedBuilder<ProiectioConfig>, ClapfigError> {
    Ok(Clapfig::typed::<ProiectioConfig>()
        .app_name(APP)
        .persist_scope("user", SearchPath::Path(user_config_dir()?))
        .on_unknown_key(|key| {
            if key.leaf.starts_with(COMMENT_KEY_PREFIX) {
                UnknownKeyDecision::Accept
            } else {
                UnknownKeyDecision::Reject
            }
        }))
}

/// A comment key is a note the file's author left, not a setting: the loader
/// accepts one wherever `config schema` allowlists it, and a listing of the
/// configuration leaves it where the writer put it.
pub(crate) fn is_comment_key(key: &str) -> bool {
    key.split('.')
        .any(|segment| segment.starts_with(COMMENT_KEY_PREFIX))
}

pub(crate) fn user_config_path() -> Result<Utf8PathBuf, anyhow::Error> {
    let path = user_config_dir()?.join(FILE);
    Utf8PathBuf::from_path_buf(path).map_err(|path| {
        exit::failure(Error::PathNotUtf8 {
            path: path.to_string_lossy().into_owned(),
        })
    })
}

fn user_config_dir() -> Result<PathBuf, ClapfigError> {
    directories::ProjectDirs::from("", "", APP)
        .map(|dirs| dirs.config_dir().to_path_buf())
        .ok_or(ClapfigError::NoPersistPath)
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
