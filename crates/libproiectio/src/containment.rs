use std::collections::BTreeSet;

use camino::{Utf8Path, Utf8PathBuf};

use crate::{Error, Result};

/// Joins an untrusted tree path onto the destination, refusing everything
/// that would land outside it.
///
/// This function and its normalize-only half `contained_normalize` —
/// deciding's admission check, which applies the identical contract without
/// the join — are the sole gateway a desired-tree path passes through on
/// its way to becoming an on-disk location (`docs/security.lex` section 2,
/// <https://github.com/civitas-forge/proiectio/blob/main/docs/security.lex>):
/// `dest` is trusted — the invoker chose it — and `rel` is hostile, computed
/// by whoever authored the mapping, source tree, or archive. Later stages
/// never join `dest` with tree input themselves; every in-dest path they
/// touch came out of this gateway.
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
///   device names (`CON`, `PRN`, `AUX`, `NUL`, `CONIN$`, `CONOUT$`, and
///   `COM`/`LPT` followed by `1`–`9` or a superscript `¹`/`²`/`³` —
///   case-insensitive, with or without an extension);
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
    Ok(dest.join(contained_normalize(rel)?))
}

/// The lexical half of [`contained_join`]: judges `rel` by the full
/// containment contract above and returns it normalized, without joining it
/// onto a destination. Deciding admits every desired-tree key through this —
/// same gateway, same verdicts — because a [`Plan`](crate::Plan) keys its
/// actions relative to the destination and needs no absolute join.
pub(crate) fn contained_normalize(rel: &Utf8Path) -> Result<Utf8PathBuf> {
    match normalize(rel) {
        Some(normalized) => Ok(normalized),
        None => Err(Error::Containment {
            paths: BTreeSet::from([rel.to_owned()]),
        }),
    }
}

/// Resolves a symlink target the way a filesystem would — lexically, from
/// the directory holding the link — and says where it lands: `Some` of the
/// resolved path relative to the destination (empty for the destination
/// itself), or `None` when the target lands outside it.
///
/// This is the grading of `docs/security.lex` section 3, and the sole
/// judge of it: [`decide`](crate::decide) runs it over every desired link,
/// and apply's no-follow walk runs it over the recorded link it meets, so
/// a target graded in-dest by one is in-dest to the other. `parent` is the
/// link's own parent directory relative to the destination — empty at the
/// destination root — and `target` is the string as written.
///
/// Where [`contained_join`] judges a path the projection will *create*,
/// this judges a pointer's content, so the two contracts differ where
/// spelling rules have no bearing on where the target lands: a `.` or
/// empty component resolves away as it does on disk, `..` pops, and a name
/// the gateway refuses for how Windows *joins* it is an ordinary name in a
/// pointer nothing joins onto an ambient path — `victim:stream` addresses
/// a stream of a sibling under the destination, `NUL` a device, neither of
/// them a place outside. Refused all the same, and graded external:
///
/// - absolute targets — the flag's headline case;
/// - `..` climbing past the destination;
/// - a leading Windows drive designator — an ASCII letter and a colon,
///   with a slash (`C:/escape`) or without (`C:escape`, `c:`). Windows
///   resolves such a target against that drive rather than against the
///   link's parent, so it lands outside the destination however the rest
///   is spelled — the one colon shape that is a location and not a name;
/// - any backslash, which the containment rules never treat as a filename
///   character: `..\..\escape` is a traversal on one host and a name on
///   another, and a projection grades it identically everywhere.
///
/// The last two are judged on every host, so a tree gets one verdict
/// everywhere.
///
/// No filesystem access: whether anything exists at the resolution is not
/// asked, because a dangling pointer is a legal link.
pub(crate) fn contained_target(parent: &Utf8Path, target: &str) -> Option<Utf8PathBuf> {
    if target.contains('\\') || target.starts_with('/') || starts_with_drive(target) {
        return None;
    }
    let mut kept: Vec<&str> = parent
        .as_str()
        .split('/')
        .filter(|component| !component.is_empty())
        .collect();
    for component in target.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                kept.pop()?;
            }
            name => kept.push(name),
        }
    }
    Some(Utf8PathBuf::from(kept.join("/")))
}

/// Whether `target` opens with a Windows drive designator: an ASCII letter
/// followed by a colon. `C:/x` names the root of drive C, `C:x` the drive's
/// own current directory — both places the destination does not contain,
/// and both spellings a `/`-only split would otherwise read as an ordinary
/// first component.
fn starts_with_drive(target: &str) -> bool {
    let mut chars = target.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic()
    )
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
    // Rejoin with `/` explicitly: a normalized key must be byte-identical
    // on every host, and collecting into a path would separate with the
    // platform separator.
    Some(Utf8PathBuf::from(kept.join("/")))
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

/// `CON`, `PRN`, `AUX`, `NUL`, `CONIN$`, `CONOUT$`, and `COM`/`LPT`
/// followed by `1`–`9` or a superscript `¹`/`²`/`³` — Microsoft's
/// documented reserved set — case-insensitive, judged on the portion
/// before the first dot because the device names win even with an
/// extension attached (`NUL.txt`).
fn is_windows_reserved_device(component: &str) -> bool {
    let base = component.split('.').next().unwrap_or(component);
    if ["con", "prn", "aux", "nul", "conin$", "conout$"]
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
    let rest: Vec<char> = chars.collect();
    (prefix == "com" || prefix == "lpt")
        && matches!(rest[..], [digit] if matches!(digit, '1'..='9' | '¹' | '²' | '³'))
}

#[cfg(test)]
#[path = "containment_tests.rs"]
mod tests;
