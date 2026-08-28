use std::collections::{BTreeSet, VecDeque};
use std::convert::Infallible;

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

/// What the chain resolution finds at one component of a target's path —
/// the single question [`contained_target_chain`] asks about the
/// destination, so the rule itself reads no disk and holds no snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Hop {
    /// Nothing that continues the chain: no node, a file, a directory, or
    /// a kind the projection never writes. Resolution walks on through the
    /// name.
    Terminal,
    /// A symlink pointing at this target string. Resolution continues
    /// through it, from the link's own parent directory.
    Link(String),
    /// A symlink whose target cannot be resolved — on disk it is not
    /// UTF-8, so nothing can say where the chain continues. Graded
    /// external: a hop nobody can follow is one nobody can vouch for.
    Unresolvable,
}

/// Resolves a symlink target from the directory holding the link,
/// following the destination's own links on the way, and says where the
/// pointer lands: `Some` of the resolved path relative to the destination
/// (empty for the destination itself), or `None` when it lands outside.
///
/// This is the grading of `docs/security.lex` section 3 whole.
/// [`contained_target`] is this function with a destination that holds no
/// links at all. `parent` is the link's own parent directory relative to
/// the destination — empty at the destination root — `target` is the string
/// as written, and `hop` answers what stands at one destination-relative
/// path: [`decide`](crate::decide) answers from the plan-time
/// [`Observations`](crate::Observations), apply answers from the disk it is
/// about to publish onto. Neither the rule nor its callers rewrite a target;
/// what reaches disk is the string verbatim.
///
/// # Resolution
///
/// Components are consumed left to right against a walked prefix that
/// starts at `parent`, the way a kernel resolves a path:
///
/// - `.` and empty components resolve away, as they do on disk;
/// - `..` pops the walked prefix, and popping past the destination root is
///   the escape — so `..` after a followed link pops the *link's* parent,
///   not the spelling the target was written with;
/// - a name asks `hop`. A [`Terminal`](Hop::Terminal) hop extends the
///   walked prefix; a [`Link`](Hop::Link) splices the link's own target
///   into the components still to consume, leaving the walked prefix at
///   that link's parent; an [`Unresolvable`](Hop::Unresolvable) hop ends
///   the resolution outside.
///
/// Refused outright — for the target as written and for every followed
/// link's target alike, since each is resolved by the same rules:
///
/// - absolute targets — the external-target permission's headline case;
/// - a leading Windows drive designator — an ASCII letter and a colon,
///   with a slash (`C:/escape`) or without (`C:escape`, `c:`). Windows
///   resolves such a target against that drive rather than against the
///   link's parent, so it lands outside the destination however the rest
///   is spelled — the one colon shape that is a location and not a name;
/// - any backslash, which the containment rules never treat as a filename
///   character: `..\..\escape` is a traversal on one host and a name on
///   another, and a projection grades it identically everywhere.
///
/// The last two are judged on every host, so those two spellings get one
/// verdict everywhere. The rest of a verdict now depends on the
/// destination: `pivot/passwd` lands in-dest where `pivot` is an ordinary
/// directory and outside where `pivot` is a link to `/etc`, so the same
/// tree can need the permission in one destination and not in another
/// (`docs/security.lex` section 3 states the contract; tree *paths* keep
/// their host-independent lexical verdict).
///
/// # Cycles
///
/// Following carries a visited set of the links followed, keyed by their
/// destination-relative paths, and a link met twice ends the resolution
/// outside rather than looping — the guard apply's no-follow walk carries,
/// in the same shape. It is a cycle guard, not a hop limit: a legitimate
/// chain may be arbitrarily long. It is also blunter than a kernel's
/// `ELOOP` counter, since a target that legitimately traverses one link
/// twice grades external too; refusing a pointer nobody has to write beats
/// resolving one forever.
///
/// # Where [`contained_join`] differs
///
/// That gateway judges a path the projection will *create*, this a
/// pointer's content, so the contracts part where spelling rules have no
/// bearing on where the target lands: a name the gateway refuses for how
/// Windows *joins* it is an ordinary name in a pointer nothing joins onto
/// an ambient path — `victim:stream` addresses a stream of a sibling under
/// the destination, `NUL` a device, and both stay in-dest here.
///
/// Whether anything exists at the landing is never asked: a dangling
/// pointer is a legal link, and so is one whose chain runs out partway.
pub(crate) fn contained_target_chain<E>(
    parent: &Utf8Path,
    target: &str,
    mut hop: impl FnMut(&Utf8Path) -> std::result::Result<Hop, E>,
) -> std::result::Result<Option<Utf8PathBuf>, E> {
    let mut walked: Vec<String> = parent
        .as_str()
        .split('/')
        .filter(|component| !component.is_empty())
        .map(str::to_owned)
        .collect();
    let mut visited: BTreeSet<Utf8PathBuf> = BTreeSet::new();
    let Some(mut remaining) = split_target(target) else {
        return Ok(None);
    };
    while let Some(component) = remaining.pop_front() {
        match component.as_str() {
            "" | "." => continue,
            ".." => {
                if walked.pop().is_none() {
                    return Ok(None);
                }
                continue;
            }
            _ => {}
        }
        walked.push(component);
        let here = Utf8PathBuf::from(walked.join("/"));
        match hop(&here)? {
            Hop::Terminal => {}
            Hop::Unresolvable => return Ok(None),
            Hop::Link(target) => {
                // The chain continues from the link's own parent, so the
                // link's name comes back off the walked prefix before its
                // target's components go on the front of the queue.
                walked.pop();
                if !visited.insert(here) {
                    return Ok(None);
                }
                let Some(mut spliced) = split_target(&target) else {
                    return Ok(None);
                };
                spliced.append(&mut remaining);
                remaining = spliced;
            }
        }
    }
    Ok(Some(Utf8PathBuf::from(walked.join("/"))))
}

/// The components of a target string, or `None` for the three spellings
/// that land outside the destination whatever follows them: an absolute
/// target, a leading Windows drive designator, and a backslash anywhere.
fn split_target(target: &str) -> Option<VecDeque<String>> {
    if target.contains('\\') || target.starts_with('/') || starts_with_drive(target) {
        return None;
    }
    Some(target.split('/').map(str::to_owned).collect())
}

/// [`contained_target_chain`] over a destination holding no links: the
/// purely lexical resolution of `target` from `parent`, reading nothing and
/// seeing through nothing.
///
/// This is what apply's no-follow walk grades an ancestor link with, one
/// hop at a time. The walk is itself the chain resolution against the live
/// disk — it restarts from the destination root along each followed
/// target and carries the same visited set — so grading one hop lexically
/// there resolves the whole chain, against a disk fresher than any
/// snapshot.
pub(crate) fn contained_target(parent: &Utf8Path, target: &str) -> Option<Utf8PathBuf> {
    match contained_target_chain(parent, target, |_| Ok::<Hop, Infallible>(Hop::Terminal)) {
        Ok(landing) => landing,
        Err(never) => match never {},
    }
}

/// Whether `target` is a pathname at all — the question that comes before
/// [`contained_target`]'s, since a string that names no path lands nowhere
/// to grade.
///
/// Two strings fail it, and no host accepts either: the empty string, which
/// names nothing, and one carrying a NUL byte, which terminates a pathname
/// rather than appearing in it. Both would reach the OS as a symlink target
/// and come back an error — on Linux `ENOENT` for the empty one, a failed
/// `CString` conversion for the NUL — after apply had begun, so the pure
/// stage refuses them instead ([`Refusal::InvalidTarget`](crate::Refusal)).
/// No policy lifts the refusal: there is no pointer here to permit.
///
/// This is not a promise that every target passing it is writable — a
/// target past the host's length limit is refused by the filesystem, and
/// nothing lexical can see that coming. It rules out the two strings that
/// are not pathnames on any host, so a tree gets one verdict everywhere.
pub(crate) fn is_pathname(target: &str) -> bool {
    !target.is_empty() && !target.contains('\0')
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
