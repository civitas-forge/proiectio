use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;

use crate::{Entry, Error, Result};

/// The mapping format version this crate accepts.
pub const MAPPING_VERSION: u32 = 1;

/// Loads a TOML mapping file into the desired tree `plan` takes.
///
/// The mapping format (`docs/cli-tour.lex` section 5,
/// <https://github.com/civitas-forge/proiectio/blob/main/docs/cli-tour.lex>):
/// a `version`, `[files."path"]` tables carrying `contents` *or* `source`
/// (exactly one) plus an optional `executable` override, and
/// `[links."path"]` tables carrying `target`. `[archives."prefix/"]` tables
/// parse structurally but fail with
/// [`Error::MappingArchiveUnimplemented`] until archive extraction lands.
///
/// The trust split (`docs/security.lex` section 1): `path` is the invoker's
/// and is trusted — it, and every `source` it references, may point anywhere
/// the invoker can read. The mapping's *content* is not trusted: every
/// projected key — the `[files]` and `[links]` table keys, which become the
/// tree's relative paths, and each `[archives]` prefix, judged without its
/// conventional trailing `/` — passes the containment gateway
/// ([`contained_join`](crate::contained_join)'s lexical contract), and the
/// offenders come back aggregated in one
/// [`Error::Containment`] naming each key verbatim. Keys land in the
/// returned tree lexically normalized (`a/../b` becomes `b`), so one
/// on-disk location has one key; two entries claiming the same normalized
/// key fail as [`Error::MappingDuplicate`].
///
/// A `source` resolves as a path join against the mapping file's own
/// directory — never the current directory — so a mapping and its assets
/// travel together. A rooted `source` therefore supplants that directory:
/// on Unix it is read as given, while on Windows a drive-less `/`-rooted
/// source would borrow the mapping's drive, as path joins do there. Link
/// targets are carried
/// verbatim and unjudged: grading a target in-dest or external needs the
/// destination and happens at plan time.
///
/// The executable bit: for `contents` entries the platform default (not
/// executable), for `source` entries the source file's own bit; an explicit
/// `executable` in the entry overrides either.
///
/// All validation — version, shape, keys, duplicates, the
/// `contents`/`source` rule — runs before any `source` file is read, so a
/// mapping fails on its own defects without touching the filesystem.
///
/// # Panics
///
/// Panics if `path` is relative: the crate never consults the current
/// directory, so a relative path here has no meaning it could honor.
pub fn load_mapping(path: &Utf8Path) -> Result<BTreeMap<Utf8PathBuf, Entry>> {
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

/// Parses mapping TOML already read from `path`; the split point that lets
/// tests table-test everything but the `source` reads with no filesystem.
fn parse(path: &Utf8Path, text: &str) -> Result<BTreeMap<Utf8PathBuf, Entry>> {
    // Read the declared version leniently before strict decoding, so an
    // unsupported future format — likely carrying keys this version does
    // not know — reports `MappingVersion`, not `MappingFormat`.
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

    // Normalize every projected key, aggregating the refused ones so a
    // hostile mapping is reported whole, each key verbatim.
    let mut refused = BTreeSet::new();
    let mut files = Vec::new();
    for (key, table) in doc.files {
        let key = Utf8PathBuf::from(key);
        match normalize_key(&key) {
            Some(normalized) => files.push((key, normalized, table)),
            None => {
                refused.insert(key);
            }
        }
    }
    let mut links = Vec::new();
    for (key, table) in doc.links {
        let key = Utf8PathBuf::from(key);
        match normalize_key(&key) {
            Some(normalized) => links.push((normalized, table)),
            None => {
                refused.insert(key);
            }
        }
    }
    // An archive key is a projected prefix, so it is confined like every
    // other projected path — judged without its conventional trailing `/`,
    // which names the same prefix.
    for key in doc.archives.keys() {
        let prefix = key.strip_suffix('/').unwrap_or(key);
        if normalize_key(Utf8Path::new(prefix)).is_none() {
            refused.insert(Utf8PathBuf::from(key.clone()));
        }
    }
    if !refused.is_empty() {
        return Err(Error::Containment { paths: refused });
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

    // Resolve each file entry's `contents`/`source` choice before any read.
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

    if !doc.archives.is_empty() {
        return Err(Error::MappingArchiveUnimplemented {
            path: path.to_owned(),
            keys: doc.archives.into_keys().map(Utf8PathBuf::from).collect(),
        });
    }

    // Only now touch the filesystem: read the referenced sources, relative
    // ones against the mapping file's own directory.
    let dir = path
        .parent()
        .expect("an absolute mapping file path has a parent");
    let mut tree = BTreeMap::new();
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
        tree.insert(normalized, entry);
    }
    for (normalized, table) in links {
        tree.insert(
            normalized,
            Entry::Symlink {
                target: table.target,
            },
        );
    }
    Ok(tree)
}

/// Runs one projected key through the containment gateway's normalize-only
/// half and returns it lexically normalized; `None` is a containment
/// refusal, which the caller aggregates into one [`Error::Containment`]
/// naming every offender. The gateway's verdict on a relative key does not
/// depend on any destination — the mapping is parsed before one exists —
/// which is exactly the half `contained_normalize` carries.
fn normalize_key(key: &Utf8Path) -> Option<Utf8PathBuf> {
    crate::containment::contained_normalize(key).ok()
}

/// Where a `[files]` entry's bytes come from, decided before any read.
enum Body {
    /// Inline `contents`, byte-for-byte the TOML string.
    Contents(String),
    /// A `source` path, as written in the mapping.
    Source(String),
}

/// Lenient first pass: the declared version alone, unknown keys ignored.
#[derive(Deserialize)]
struct VersionProbe {
    version: u32,
}

/// The mapping document, decoded strictly: an unknown key anywhere is a
/// [`Error::MappingFormat`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    /// Checked against [`MAPPING_VERSION`] by the lenient pass before this
    /// struct decodes; present here so strict decoding accepts the key.
    #[expect(dead_code, reason = "validated via the lenient pass")]
    version: u32,
    #[serde(default)]
    files: BTreeMap<String, FileTable>,
    #[serde(default)]
    links: BTreeMap<String, LinkTable>,
    #[serde(default)]
    archives: BTreeMap<String, ArchiveTable>,
}

/// One `[files."path"]` table.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileTable {
    contents: Option<String>,
    source: Option<String>,
    executable: Option<bool>,
}

/// One `[links."path"]` table.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkTable {
    target: String,
}

/// One `[archives."prefix/"]` table: parsed structurally so a mapping using
/// archives fails on shape errors honestly, then refused whole as
/// [`Error::MappingArchiveUnimplemented`] until extraction is implemented.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    dead_code,
    reason = "parsed structurally; extraction is not implemented yet"
)]
struct ArchiveTable {
    source: String,
    strip: Option<u32>,
}

/// Whether the source file's owner-executable bit is set — the metadata
/// copied when a `[files]` entry gives no explicit `executable`.
#[cfg(unix)]
fn is_executable(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o100 != 0
}

/// On a platform without an executable bit the filesystem's answer is "not
/// executable" — the same default inline `contents` get.
#[cfg(not(unix))]
fn is_executable(_meta: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
#[path = "mapping_tests.rs"]
mod tests;
