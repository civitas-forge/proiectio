//! The columns and cells a `--output csv` projection is built from, shared by
//! the projections `status` and a write pass declare so the two spell one row
//! alike.

use serde_json::Value as JsonValue;
use standout::tabular::{Column, Width};

/// A column reading one field of the row.
pub(super) fn column(key: &str) -> Column {
    header(key).key(key)
}

/// A column a callback fills, named for what it states.
pub(super) fn header(name: &str) -> Column {
    Column::new(Width::fill()).header(name)
}

/// A cell holding what a row states, and an empty one where it states nothing.
pub(super) fn cell(value: Option<String>) -> JsonValue {
    JsonValue::String(value.unwrap_or_default())
}

/// The shape the row states for the path, in one word.
pub(super) fn shape(row: &JsonValue) -> Option<String> {
    let shape = row.get("facts")?.get("shape")?;
    let named = match shape {
        JsonValue::String(name) => name.as_str(),
        JsonValue::Object(fields) => fields.keys().next()?.as_str(),
        _ => return None,
    };
    Some(named.to_lowercase())
}

/// Whether the file carries the executable bit; nothing for a path whose shape
/// has no such bit.
pub(super) fn executable(row: &JsonValue) -> Option<String> {
    let executable = row
        .get("facts")?
        .get("shape")?
        .get("File")?
        .get("executable")?
        .as_bool()?;
    Some(executable.to_string())
}

/// Where the row's link points, for a row stating a link that names one.
pub(super) fn target(row: &JsonValue) -> Option<String> {
    Some(
        row.get("facts")?
            .get("shape")?
            .get("Symlink")?
            .get("target")?
            .as_str()?
            .to_owned(),
    )
}

/// The owners holding the path, as the JSON array the row states them in. An
/// owner name is an opaque string, so any character this cell joined names
/// with could also sit inside one, and `["a+b", "c"]` and `["a", "b+c"]` would
/// reach a reader as the same cell; the array says which is which.
pub(super) fn owners(row: &JsonValue) -> Option<String> {
    let owners = row.get("facts")?.get("owners")?.as_array()?;
    if owners.is_empty() {
        return None;
    }
    serde_json::to_string(owners).ok()
}
