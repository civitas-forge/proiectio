use std::collections::BTreeSet;

use camino::{Utf8Path, Utf8PathBuf};

use crate::{Error, Result};

/// Joins an untrusted tree path onto the destination, refusing everything
/// that would land outside it.
///
/// This is the sole gateway a desired-tree path passes through on its way
/// to becoming an on-disk location (`docs/security.lex` section 2,
/// <https://github.com/civitas-forge/proiectio/blob/main/docs/security.lex>):
/// `dest` is trusted — the invoker chose it — and `rel` is hostile, computed
/// by whoever authored the mapping, source tree, or archive. Later stages
/// never join `dest` with tree input themselves; every in-dest path they
/// touch came out of this function.
///
/// # Contract
///
/// Refused, each as [`Error::Containment`] carrying `rel` verbatim:
///
/// - absolute paths — a leading `/`, and Windows forms (`C:...`, `\\server`)
///   judged lexically so the verdict is identical on every platform;
/// - any backslash: `\` is a separator on Windows and would smuggle
///   components (`..\..\x`) past a `/`-only split, so it never counts as a
///   filename character in a projected tree;
/// - empty and `.` components — which covers `a//b`, `./x`, and trailing
///   slashes;
/// - `..` climbing past the destination after normalization;
/// - a path that normalizes to nothing (`""`, `a/..`): it names the
///   destination itself, not a path inside it.
///
/// Everything else is normalized **lexically** — `a/../b` joins as
/// `dest/b` — and joined onto `dest`. No filesystem access: the function
/// never canonicalizes (canonicalize follows symlinks and requires the path
/// to exist; `clippy.toml` bans it crate-wide). The other half of
/// containment — no write through a symlinked ancestor — is a check against
/// the disk and belongs to apply, not to this function.
pub fn contained_join(dest: &Utf8Path, rel: &Utf8Path) -> Result<Utf8PathBuf> {
    match normalize(rel) {
        Some(normalized) => Ok(dest.join(normalized)),
        None => Err(Error::Containment {
            paths: BTreeSet::from([rel.to_owned()]),
        }),
    }
}

/// Lexical normalization of a relative tree path; `None` is a refusal.
///
/// Hand-rolled rather than `path-absolutize`/`normpath`: both normalize
/// through `std::path::Path`, whose separator semantics follow the build
/// platform — on Unix a hostile `..\..\x` is one opaque component to them —
/// while this split over `/` gives the same verdict everywhere.
fn normalize(rel: &Utf8Path) -> Option<Utf8PathBuf> {
    let raw = rel.as_str();
    if raw.contains('\\') || rel.is_absolute() || has_windows_drive_prefix(raw) {
        return None;
    }
    let mut kept: Vec<&str> = Vec::new();
    for component in raw.split('/') {
        match component {
            "" | "." => return None,
            ".." => {
                kept.pop()?;
            }
            name => kept.push(name),
        }
    }
    if kept.is_empty() {
        return None;
    }
    Some(kept.iter().collect())
}

/// `C:...` — an ASCII drive letter followed by a colon — which Windows
/// resolves outside any destination. Detected lexically so a tree authored
/// for Windows is refused on every platform. An empty `raw` never matches;
/// it is refused as normalizing to nothing.
fn has_windows_drive_prefix(raw: &str) -> bool {
    let mut chars = raw.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic()
    )
}

#[cfg(test)]
#[path = "containment_tests.rs"]
mod tests;
