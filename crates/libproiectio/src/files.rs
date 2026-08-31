use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::ambient_authority;
use cap_std::fs::Dir;

use crate::limits::Budget;
use crate::tree::{is_executable, open_file_nofollow};
use crate::{Desired, Entry, Error, IoRole, Limits, Origin, Result};

/// Loads each path as an entry keyed by its own basename. One budget of
/// [`Limits::max_source_bytes`] covers everything the call holds: each
/// file's bytes, the basename it is keyed by, and each symlink's target.
pub fn load_files(paths: &[Utf8PathBuf], limits: Limits) -> Result<Desired> {
    let mut named: BTreeMap<String, Utf8PathBuf> = BTreeMap::new();
    for path in paths {
        let path = crate::absolutize(path)?;
        let Some(name) = path.file_name().map(str::to_owned) else {
            return Err(Error::FilesNodeKind { path });
        };
        match named.get(&name) {
            Some(first) if *first == path => {}
            Some(first) => {
                return Err(Error::FilesDuplicate {
                    first: first.clone(),
                    second: path,
                });
            }
            None => {
                named.insert(name, path);
            }
        }
    }

    let budget = Budget::new(limits);
    let mut tree = BTreeMap::new();
    for (name, path) in named {
        let entry = load_one(&path, &budget)?;
        // The same rule the tree walk holds itself to: a file's bytes are
        // already spent, and what is held besides them — the basename this
        // entry is keyed by, a symlink's target — is spent here. A hundred
        // thousand empty files are a hundred thousand keys and no bytes.
        let held = name.len()
            + match &entry {
                Entry::File { .. } => 0,
                Entry::Symlink { target } => target.len(),
                Entry::Block { body, marker, .. } => body.len() + marker.len(),
            };
        if !budget.spend(held as u64) {
            return Err(Error::SourceTooLarge {
                path,
                limit: budget.limit(),
            });
        }
        tree.insert(Utf8PathBuf::from(name), entry);
    }
    Ok(Desired::from_source(tree, Origin::Files))
}

fn load_one(path: &Utf8Path, budget: &Budget) -> Result<Entry> {
    let parent = path.parent().expect("an absolute path has a parent");
    let name = path.file_name().expect("a named path has a file name");
    let io = |source| Error::Io {
        role: IoRole::NamedFile,
        path: path.to_owned(),
        source,
    };

    // Named by the path the caller handed in rather than by the parent that
    // failed to open: the caller never named the parent, and a message about a
    // directory they did not write on the command line sends them looking for
    // the wrong mistake.
    let dir = Dir::open_ambient_dir(parent, ambient_authority()).map_err(io)?;
    let file_type = dir.symlink_metadata(name).map_err(io)?.file_type();
    if file_type.is_symlink() {
        let target = dir.read_link_contents(name).map_err(io)?;
        let target =
            target
                .into_os_string()
                .into_string()
                .map_err(|raw| Error::TreeTargetNotUtf8 {
                    path: path.to_owned(),
                    target: raw.to_string_lossy().into_owned(),
                })?;
        return Ok(Entry::Symlink { target });
    }
    if !file_type.is_file() {
        return Err(Error::FilesNodeKind {
            path: path.to_owned(),
        });
    }

    let mut file = open_file_nofollow(&dir, name).map_err(io)?;
    let meta = file.metadata().map_err(io)?;
    if !meta.is_file() {
        return Err(Error::FilesNodeKind {
            path: path.to_owned(),
        });
    }
    let executable = is_executable(&meta);
    let contents =
        budget
            .read_to_end(&mut file)
            .map_err(io)?
            .ok_or_else(|| Error::SourceTooLarge {
                path: path.to_owned(),
                limit: budget.limit(),
            })?;
    Ok(Entry::File {
        contents,
        executable,
    })
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
