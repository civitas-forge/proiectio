//! The clapfig configuration schema and the one builder every consumer goes through.

use camino::Utf8PathBuf;
use clapfig::error::ClapfigError;
use clapfig::runtime::{LeafType, Shape};
use clapfig::{Clapfig, ConfigAction, Schema, SearchPath, TypedBuilder, UnknownKeyDecision};
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
/// The one scope [`builder`] registers, and so the only name a `--scope` can
/// carry: clapfig refuses every other by name, naming the scopes it has.
const USER_SCOPE: &str = "user";

pub(crate) fn builder() -> TypedBuilder<ProiectioConfig> {
    Clapfig::typed::<ProiectioConfig>()
        .app_name(APP)
        .persist_scope(USER_SCOPE, SearchPath::Platform)
        .on_unknown_key(|key| {
            if key.leaf.starts_with(COMMENT_KEY_PREFIX) {
                UnknownKeyDecision::Accept
            } else {
                UnknownKeyDecision::Reject
            }
        })
}

/// The file an edit through the `user` scope lands in, and whether a file is
/// there once the edit has run.
pub(crate) struct Edit {
    pub(crate) path: Utf8PathBuf,
    pub(crate) present: bool,
}

/// Resolves the file a `set` or `unset` is about to edit, so that a platform
/// path this CLI cannot spell refuses the run before clapfig writes rather
/// than after. Clapfig persists through a `PathBuf`, which carries paths the
/// report cannot; reading one back afterwards would fail a run whose edit had
/// already reached the disk.
///
/// A `--scope` naming anything else reaches clapfig unexamined, which refuses
/// it by name — resolving the user scope's path first would answer a wrong
/// scope with a complaint about a file it never meant.
pub(crate) fn check_edit_path(action: &ConfigAction) -> Result<(), Error> {
    if edits_the_user_scope(action) {
        user_config_path().map(drop)
    } else {
        Ok(())
    }
}

/// Whether `action` is an edit landing in the file this CLI resolves: one of
/// the two editing actions, naming the one registered scope or no scope at
/// all. Every other action either edits nothing or names a scope clapfig
/// refuses.
fn edits_the_user_scope(action: &ConfigAction) -> bool {
    let scope = match action {
        ConfigAction::Set { scope, .. } | ConfigAction::Unset { scope, .. } => scope,
        _ => return false,
    };
    scope.as_deref().is_none_or(|name| name == USER_SCOPE)
}

/// The file a `set` or `unset` through the `user` scope persisted to, and
/// whether one is there now. Clapfig reports neither: `ConfigResult::ValueSet`
/// and `ValueUnset` carry no path, and `unset_value` returns the same
/// `ValueUnset` whether it rewrote a file or found none to read. So the file a
/// set wrote is known from the set having succeeded, and the one an unset
/// wrote is read back here.
pub(crate) fn persisted_edit() -> Result<Edit, Error> {
    let path = user_config_path()?;
    Ok(Edit {
        present: path.is_file(),
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
    let segments: Vec<&str> = key.split('.').collect();
    leaf_type_in(&ProiectioConfig::shape(), &segments)
}

/// Walks `shape` the way clapfig's own `doc_for_shape` walks it, so the schema
/// answers for every key a listing can carry: a map's first segment is the
/// entry key the writer chose, and the rest name fields of the item shape. A
/// segment that lands on an array or a tagged union has no single leaf type,
/// and the value keeps clapfig's spelling.
fn leaf_type_in(shape: &Shape, segments: &[&str]) -> Option<LeafType> {
    match shape {
        Shape::Leaf(leaf) if segments.is_empty() => Some(leaf.ty.clone()),
        Shape::Object(schema) => {
            let (name, rest) = segments.split_first()?;
            let field = schema.fields.iter().find(|field| field.name == *name)?;
            leaf_type_in(&field.field, rest)
        }
        Shape::Map(map) => leaf_type_in(&map.item, segments.split_first()?.1),
        _ => None,
    }
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
