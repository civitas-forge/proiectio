use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::ambient_authority;
use cap_std::fs::Dir;

use crate::tree::{is_executable, open_file_nofollow};
use crate::{Desired, Entry, Error, Origin, Result};

pub fn load_files(paths: &[Utf8PathBuf]) -> Result<Desired> {
    let mut named: BTreeMap<String, BTreeSet<Utf8PathBuf>> = BTreeMap::new();
    for path in paths {
        let path = crate::absolutize(path)?;
        let Some(name) = path.file_name().map(str::to_owned) else {
            return Err(Error::TreeNodeKind { path });
        };
        named.entry(name).or_default().insert(path);
    }
    if let Some((name, paths)) = named.iter().find(|(_, paths)| paths.len() > 1) {
        return Err(Error::FilesDuplicate {
            name: name.clone(),
            paths: paths.clone(),
        });
    }

    let mut tree = BTreeMap::new();
    for (name, paths) in named {
        let path = paths.into_iter().next().expect("a name holds one path");
        tree.insert(Utf8PathBuf::from(name), load_one(&path)?);
    }
    Ok(Desired::from_source(tree, Origin::Files))
}

fn load_one(path: &Utf8Path) -> Result<Entry> {
    let parent = path.parent().expect("an absolute path has a parent");
    let name = path.file_name().expect("a named path has a file name");
    let io = |source| Error::Io {
        path: path.to_owned(),
        source,
    };

    let dir = Dir::open_ambient_dir(parent, ambient_authority()).map_err(|source| Error::Io {
        path: parent.to_owned(),
        source,
    })?;
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
        return Err(Error::TreeNodeKind {
            path: path.to_owned(),
        });
    }

    let mut file = open_file_nofollow(&dir, name).map_err(io)?;
    let meta = file.metadata().map_err(io)?;
    if !meta.is_file() {
        return Err(Error::TreeNodeKind {
            path: path.to_owned(),
        });
    }
    let executable = is_executable(&meta);
    let mut contents = Vec::new();
    file.read_to_end(&mut contents).map_err(io)?;
    Ok(Entry::File {
        contents,
        executable,
    })
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
