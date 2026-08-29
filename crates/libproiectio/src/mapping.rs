use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;

use crate::{Desired, Entry, Error, Origin, Refusal, Refused, Result};

/// The mapping format version this crate accepts.
pub const MAPPING_VERSION: u32 = 1;

/// Loads a TOML mapping file into the desired tree `plan` takes, resolving
/// each `source` against the mapping file's own directory.
///
/// The version, every projected key, and each entry's `contents`/`source`
/// choice are all judged before any `source` file is read or any archive is
/// opened.
///
/// # Panics
///
/// Panics if `path` is relative.
pub fn load_mapping(path: &Utf8Path) -> Result<Desired> {
    assert!(
        path.is_absolute(),
        "mapping path must be absolute, got {path}"
    );
    let text = fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })?;
    parse(path, &text)
}

fn parse(path: &Utf8Path, text: &str) -> Result<Desired> {
    let mapping_origin = Origin::Mapping {
        path: path.to_owned(),
    };
    let probe: VersionProbe = toml::from_str(text).map_err(|source| Error::MappingFormat {
        path: path.to_owned(),
        source,
    })?;
    if probe.version != MAPPING_VERSION {
        return Err(Error::MappingVersion {
            path: path.to_owned(),
            found: probe.version,
            supported: MAPPING_VERSION,
        });
    }
    let doc: Document = toml::from_str(text).map_err(|source| Error::MappingFormat {
        path: path.to_owned(),
        source,
    })?;

    let mut refused = BTreeSet::new();
    let mut files = Vec::new();
    for (key, table) in doc.files {
        let key = Utf8PathBuf::from(key);
        match crate::containment::contained_normalize(&key) {
            Some(normalized) => files.push((key, normalized, table)),
            None => {
                refused.insert(key);
            }
        }
    }
    let mut links = Vec::new();
    for (key, table) in doc.links {
        let key = Utf8PathBuf::from(key);
        match crate::containment::contained_normalize(&key) {
            Some(normalized) => links.push((normalized, table)),
            None => {
                refused.insert(key);
            }
        }
    }
    let mut archives = Vec::new();
    for (key, table) in doc.archives {
        let prefix = key.strip_suffix('/').unwrap_or(&key);
        match crate::containment::contained_normalize(Utf8Path::new(prefix)) {
            Some(normalized) => archives.push((normalized, table)),
            None => {
                refused.insert(Utf8PathBuf::from(key));
            }
        }
    }
    if !refused.is_empty() {
        return Err(Refused::aggregate(
            refused
                .into_iter()
                .map(|key| (key, Refusal::Containment, mapping_origin.clone())),
        )
        .expect("refused is not empty")
        .into());
    }

    let mut seen = BTreeSet::new();
    for normalized in files
        .iter()
        .map(|(_, normalized, _)| normalized)
        .chain(links.iter().map(|(normalized, _)| normalized))
    {
        if !seen.insert(normalized.clone()) {
            return Err(Error::MappingDuplicate {
                path: path.to_owned(),
                key: normalized.clone(),
            });
        }
    }
    let mut prefixes = BTreeSet::new();
    for (prefix, _) in &archives {
        if !prefixes.insert(prefix.clone()) {
            return Err(Error::MappingDuplicate {
                path: path.to_owned(),
                key: prefix.clone(),
            });
        }
    }

    let mut bodies = Vec::new();
    for (key, normalized, table) in files {
        let body = match (table.contents, table.source) {
            (Some(contents), None) => Body::Contents(contents),
            (None, Some(source)) => Body::Source(source),
            (Some(_), Some(_)) | (None, None) => {
                return Err(Error::MappingContentsXorSource {
                    path: path.to_owned(),
                    key,
                });
            }
        };
        bodies.push((normalized, body, table.executable));
    }

    let dir = path
        .parent()
        .expect("an absolute mapping file path has a parent");
    let mut tree = Desired::new();
    for (normalized, body, executable) in bodies {
        let entry = match body {
            Body::Contents(contents) => Entry::File {
                contents: contents.into_bytes(),
                executable: executable.unwrap_or(false),
            },
            Body::Source(source) => {
                // One handle for bytes and metadata, so both describe the
                // same file even if the path is swapped mid-read.
                let source_path = dir.join(source);
                let io = |source| Error::Io {
                    path: source_path.clone(),
                    source,
                };
                let mut file = fs::File::open(&source_path).map_err(io)?;
                let executable = match executable {
                    Some(explicit) => explicit,
                    None => is_executable(&file.metadata().map_err(io)?),
                };
                let mut contents = Vec::new();
                file.read_to_end(&mut contents).map_err(io)?;
                Entry::File {
                    contents,
                    executable,
                }
            }
        };
        tree.insert(normalized, entry, mapping_origin.clone());
    }
    for (normalized, table) in links {
        tree.insert(
            normalized,
            Entry::Symlink {
                target: table.target,
            },
            mapping_origin.clone(),
        );
    }
    // One byte budget across every table: the expanded trees are all merged
    // into this one, so they are all live at once.
    let budget = crate::archive::new_budget();
    for (prefix, table) in archives {
        let source = dir.join(table.source);
        let expanded = crate::archive::expand(
            &source,
            table.strip.unwrap_or(0),
            &prefix,
            Some(path),
            &budget,
        )?;
        let origin = Origin::Archive {
            path: source.clone(),
            via: Some(path.to_owned()),
        };
        for (key, entry) in expanded.iter() {
            if !tree.insert(key.clone(), entry.clone(), origin.clone()) {
                return Err(Error::MappingDuplicate {
                    path: path.to_owned(),
                    key: key.clone(),
                });
            }
        }
    }
    Ok(tree)
}

enum Body {
    Contents(String),
    Source(String),
}

/// Lenient first pass: the declared version alone, unknown keys ignored.
#[derive(Deserialize)]
struct VersionProbe {
    version: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    #[expect(dead_code, reason = "validated via the lenient pass")]
    version: u32,
    #[serde(default)]
    files: BTreeMap<String, FileTable>,
    #[serde(default)]
    links: BTreeMap<String, LinkTable>,
    #[serde(default)]
    archives: BTreeMap<String, ArchiveTable>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileTable {
    contents: Option<String>,
    source: Option<String>,
    executable: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkTable {
    target: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveTable {
    source: String,
    strip: Option<u32>,
}

fn is_executable(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o100 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
#[path = "mapping_tests.rs"]
mod tests;
