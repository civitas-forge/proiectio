//! The clapfig configuration schema and the one builder every consumer goes through.

use camino::Utf8PathBuf;
use clapfig::error::ClapfigError;
#[cfg(test)]
use clapfig::runtime::Shape;
use clapfig::{Clapfig, ConfigAction, Schema, SearchPath, TypedBuilder, UnknownKeyDecision};
use libproiectio::{Error, IoRole, OWNER_RULE, names_an_owner};
use serde::{Deserialize, Serialize};

/// Projection settings as `proiectio.toml` declares them.
#[derive(Schema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProiectioConfig {
    /// The manifest owner a run records its entries under.
    /// Default for `--owner`; the flag always wins.
    #[clapfig(default = "default")]
    pub(crate) owner: String,

    /// How many bytes one write may read from its sources, counting an
    /// archive as it expands rather than as it sits on disk — except a zip,
    /// whose file must fit too, since its index is read before any member.
    /// Default for `--max-source-size`; the flag always wins.
    #[clapfig(default = 524_288_000)]
    pub(crate) max_source_size: u64,
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

/// Resolves the file a `set` or `unset` is about to edit, so a platform path
/// this CLI cannot spell refuses the run before clapfig writes rather than
/// after the edit has reached the disk. A `--scope` naming anything but the
/// user scope reaches clapfig unexamined, which refuses it by name.
pub(crate) fn check_edit_path(action: &ConfigAction) -> Result<(), Error> {
    if edits_the_user_scope(action) {
        user_config_path().map(drop)
    } else {
        Ok(())
    }
}

fn edits_the_user_scope(action: &ConfigAction) -> bool {
    let scope = match action {
        ConfigAction::Set { scope, .. } | ConfigAction::Unset { scope, .. } => scope,
        _ => return false,
    };
    scope.as_deref().is_none_or(|name| name == USER_SCOPE)
}

/// The file a `set` or `unset` through the `user` scope persisted to, and
/// whether one is there now — read back here because clapfig's
/// `ConfigResult` reports neither.
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
        role: IoRole::Unstated,
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

const OWNER_KEY: &str = "owner";

/// Refuses a configured owner that names no owner; this is the layer that
/// names the configuration as where the bad value came from. `config list`
/// and `config get` do not take this route: an operator whose file carries a
/// blank owner needs to be able to read it back.
pub(crate) fn require_owner(owner: String) -> Result<String, anyhow::Error> {
    match names_an_owner(&owner) {
        true => Ok(owner),
        false => Err(anyhow::anyhow!(
            "the configured owner is {owner:?}: {OWNER_RULE}"
        )),
    }
}

/// Refuses a value `config set` must not write: an empty string parses as a
/// `String` and would land in the file, which is what this catches.
pub(crate) fn require_value(key: &str, value: &str) -> Result<(), anyhow::Error> {
    let complaint = match key {
        OWNER_KEY => (!names_an_owner(value)).then_some(OWNER_RULE),
        _ => value.is_empty().then_some("empty is not a value"),
    };
    match complaint {
        Some(reason) => Err(anyhow::anyhow!(
            "{key} cannot be set to {value:?}: {reason}"
        )),
        None => Ok(()),
    }
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

/// What the `config` help's prose list of keys is checked against.
#[cfg(test)]
pub(crate) fn declared_keys() -> Vec<String> {
    match ProiectioConfig::shape() {
        Shape::Object(schema) => schema.fields.into_iter().map(|field| field.name).collect(),
        _ => panic!("the configuration is an object of named keys"),
    }
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
