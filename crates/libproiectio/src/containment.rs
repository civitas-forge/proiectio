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
/// - component shapes Windows resolves somewhere other than an ordinary
///   file under the destination, refused in *every* component on every
///   host: a colon — drive prefixes, where `Path::push` on Windows replaces
///   the accumulated path (`a/C:evil`), and NTFS alternate data streams
///   (`victim:stream`) — a trailing dot or space, which Windows strips
///   before resolving (`".. "` would resolve as `..` there), and reserved
///   device names (`NUL`, `CON`, `AUX`, `PRN`, `COM1`–`COM9`,
///   `LPT1`–`LPT9`, case-insensitive, with or without an extension);
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
    if raw.contains('\\') || raw.starts_with('/') {
        return None;
    }
    let mut kept: Vec<&str> = Vec::new();
    for component in raw.split('/') {
        match component {
            "" | "." => return None,
            ".." => {
                kept.pop()?;
            }
            name if windows_resolves_specially(name) => return None,
            name => kept.push(name),
        }
    }
    if kept.is_empty() {
        return None;
    }
    Some(kept.iter().collect())
}

/// Component shapes Windows resolves somewhere other than an ordinary file
/// of that name, checked against every component on every host so the
/// verdict is platform-identical:
///
/// - a colon: `C:evil` is a drive prefix — `Path::push` on Windows
///   *replaces* the accumulated path when handed one, so `a/C:evil` would
///   escape `dest` — and `victim:stream` addresses an NTFS alternate data
///   stream of `victim` rather than a file named `victim:stream`;
/// - a trailing dot or space: Windows strips them before resolving, so
///   `".. "` kept as a name would resolve as `..` there and climb out;
/// - a reserved device name: `dest\NUL` opens a device, not a file in
///   `dest`.
fn windows_resolves_specially(component: &str) -> bool {
    component.contains(':')
        || component.ends_with('.')
        || component.ends_with(' ')
        || is_windows_reserved_device(component)
}

/// `CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9` —
/// case-insensitive, judged on the portion before the first dot because the
/// device names win even with an extension attached (`NUL.txt`).
fn is_windows_reserved_device(component: &str) -> bool {
    let base = component.split('.').next().unwrap_or(component);
    if ["con", "prn", "aux", "nul"]
        .iter()
        .any(|device| base.eq_ignore_ascii_case(device))
    {
        return true;
    }
    let mut chars = base.chars();
    let prefix: String = chars
        .by_ref()
        .take(3)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let rest = chars.as_str();
    (prefix == "com" || prefix == "lpt")
        && rest.len() == 1
        && rest.chars().all(|digit| ('1'..='9').contains(&digit))
}

#[cfg(test)]
#[path = "containment_tests.rs"]
mod tests;
