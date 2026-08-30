use std::collections::{BTreeSet, VecDeque};
use std::convert::Infallible;

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};

use crate::{Error, Origin, Refusal, Refused, Result};

/// Normalizes `rel` lexically and joins it onto `dest`, refusing as
/// [`Refusal::Containment`] any path that would land outside the destination.
pub fn contained_join(dest: &Utf8Path, rel: &Utf8Path) -> Result<Utf8PathBuf> {
    match contained_normalize(rel) {
        Some(normalized) => Ok(dest.join(normalized)),
        None => Err(Refused::one(rel.to_owned(), Refusal::Containment, Origin::Caller).into()),
    }
}

/// `rel` normalized lexically under the containment contract, or `None`
/// where it is refused.
pub(crate) fn contained_normalize(rel: &Utf8Path) -> Option<Utf8PathBuf> {
    let raw = rel.as_str();
    if raw.contains('\\') || raw.contains('\0') || raw.starts_with('/') {
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
    Some(Utf8PathBuf::from(kept.join("/")))
}

pub fn absolutize(path: &Utf8Path) -> Result<Utf8PathBuf> {
    if path.is_absolute() {
        return Ok(collapse(path));
    }
    let cwd = std::env::current_dir().map_err(|source| Error::CurrentDirectory { source })?;
    let cwd = Utf8PathBuf::from_path_buf(cwd).map_err(|cwd| Error::PathNotUtf8 {
        path: cwd.to_string_lossy().into_owned(),
    })?;
    Ok(absolutize_from(&cwd, path))
}

fn absolutize_from(cwd: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    collapse(&cwd.join(path))
}

fn collapse(path: &Utf8Path) -> Utf8PathBuf {
    let mut collapsed = Utf8PathBuf::from("/");
    for component in path.components() {
        match component {
            Utf8Component::Normal(name) => collapsed.push(name),
            Utf8Component::ParentDir => {
                collapsed.pop();
            }
            _ => {}
        }
    }
    collapsed
}

/// What [`contained_target_chain`]'s caller finds at one component of a
/// target's path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Hop {
    /// Nothing that continues the chain; resolution walks on through the name.
    Terminal,
    /// A symlink pointing at this target string; resolution continues through
    /// it from the link's own parent.
    Link(String),
    /// A hop nobody can follow; resolution ends outside the destination.
    Unresolvable,
}

/// Resolves the symlink target `target` written in directory `parent`,
/// asking `hop` what stands at each destination-relative path along the way:
/// `Some` of the landing relative to the destination, or `None` where it
/// lands outside. A link met twice ends the resolution outside.
///
/// The visited set is a cycle guard, not a hop limit: a chain may be
/// arbitrarily long, and one that legitimately walks a single link twice ends
/// outside like a cycle does.
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

/// The components of a target string, or `None` for an absolute target, a
/// leading Windows drive designator, or a backslash anywhere.
fn split_target(target: &str) -> Option<VecDeque<String>> {
    if target.contains('\\') || target.starts_with('/') || starts_with_drive(target) {
        return None;
    }
    Some(target.split('/').map(str::to_owned).collect())
}

/// [`contained_target_chain`] with every hop terminal: the purely lexical
/// resolution of `target` from `parent`, reading no disk.
pub(crate) fn contained_target(parent: &Utf8Path, target: &str) -> Option<Utf8PathBuf> {
    match contained_target_chain(parent, target, |_| Ok::<Hop, Infallible>(Hop::Terminal)) {
        Ok(landing) => landing,
        Err(never) => match never {},
    }
}

/// Whether `target` is a pathname at all: not empty, and free of NUL.
pub(crate) fn is_pathname(target: &str) -> bool {
    !target.is_empty() && !target.contains('\0')
}

/// Whether `target` opens with a Windows drive designator: an ASCII letter
/// followed by a colon.
fn starts_with_drive(target: &str) -> bool {
    let mut chars = target.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic()
    )
}

/// Whether Windows resolves a component of this shape — a colon, a trailing
/// dot or space, or a reserved device name — somewhere other than an
/// ordinary file of that name. Checked on every host.
fn windows_resolves_specially(component: &str) -> bool {
    component.contains(':')
        || component.ends_with('.')
        || component.ends_with(' ')
        || is_windows_reserved_device(component)
}

/// Microsoft's documented reserved device names, case-insensitive and judged
/// on the portion before the first dot: they win even with an extension
/// attached (`NUL.txt`).
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
