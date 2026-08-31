//! The state directory as one value: the capability handle its I/O goes
//! through, and the absolute path its messages name.

use std::io::ErrorKind::NotFound;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::ambient_authority;
use cap_std::fs_utf8::Dir;

use crate::{Error, IoRole, Result};

/// A state directory: the capability handle every read and write in it goes
/// through, paired with the absolute path its error messages name.
///
/// The path is for messages only, never for rooting I/O. A manifest that does
/// not parse, or a lock another writer holds, is one the operator has to go
/// open, and the file name alone does not say which state directory holds it.
/// Only this module pairs the two, and every constructor here opens the handle
/// from the path it records, so a caller cannot read one directory while its
/// messages name another.
#[derive(Debug)]
pub(crate) struct StateDir {
    dir: Dir,
    path: Utf8PathBuf,
}

impl StateDir {
    /// The directory at absolute `path`, opened against ambient authority;
    /// `None` where nothing stands there.
    pub(crate) fn open(path: &Utf8Path) -> Option<Result<StateDir>> {
        absent_is_none(Dir::open_ambient_dir(path, ambient_authority()), path)
    }

    /// The directory `prefix` names inside `parent`, whose own absolute path
    /// is `parent_path`; `None` where nothing stands there.
    ///
    /// The handle comes from `parent` rather than from a second ambient open
    /// of the joined path, so a rename between the two opens cannot leave this
    /// handle in one directory and the messages naming another.
    pub(crate) fn open_under(
        parent: &Dir,
        parent_path: &Utf8Path,
        prefix: &Utf8Path,
    ) -> Option<Result<StateDir>> {
        absent_is_none(parent.open_dir(prefix), &parent_path.join(prefix))
    }

    /// [`open`](StateDir::open), creating the directory — and the directories
    /// above it — where there is none.
    pub(crate) fn open_or_create(path: &Utf8Path) -> Result<StateDir> {
        std::fs::create_dir_all(path).map_err(io_error(path))?;
        let dir = Dir::open_ambient_dir(path, ambient_authority()).map_err(io_error(path))?;
        Ok(StateDir {
            dir,
            path: path.to_owned(),
        })
    }

    /// [`open_under`](StateDir::open_under), creating the directory — and the
    /// directories above it — where there is none.
    pub(crate) fn open_or_create_under(
        parent: &Dir,
        parent_path: &Utf8Path,
        prefix: &Utf8Path,
    ) -> Result<StateDir> {
        let path = parent_path.join(prefix);
        parent.create_dir_all(prefix).map_err(io_error(&path))?;
        let dir = parent.open_dir(prefix).map_err(io_error(&path))?;
        Ok(StateDir { dir, path })
    }

    /// The handle every read and write in the directory goes through.
    pub(crate) fn dir(&self) -> &Dir {
        &self.dir
    }

    /// The absolute path of `name` inside the directory, for a message.
    pub(crate) fn path_of(&self, name: &str) -> Utf8PathBuf {
        self.path.join(name)
    }
}

/// One open of the directory at `path`, with its absence reported as `None`.
fn absent_is_none(opened: std::io::Result<Dir>, path: &Utf8Path) -> Option<Result<StateDir>> {
    match opened {
        Ok(dir) => Some(Ok(StateDir {
            dir,
            path: path.to_owned(),
        })),
        Err(source) if source.kind() == NotFound => None,
        Err(source) => Some(Err(io_error(path)(source))),
    }
}

fn io_error(path: &Utf8Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |source| Error::Io {
        role: IoRole::StateDirectory,
        path: path.to_owned(),
        source,
    }
}
