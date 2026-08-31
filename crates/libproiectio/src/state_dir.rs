//! The state directory as one value: the capability handle its I/O goes
//! through, and the absolute path its messages name.

use camino::Utf8PathBuf;
use cap_std::fs_utf8::Dir;

/// A state directory: the capability handle every read and write in it goes
/// through, paired with the absolute path its error messages name.
///
/// The path is for messages only, never for rooting I/O. A manifest that does
/// not parse, or a lock another writer holds, is one the operator has to go
/// open, and the file name alone does not say which state directory holds it.
/// [`Projection`](crate::Projection), which absolutizes the path, pairs the
/// two, so a caller cannot read one directory while naming another.
#[derive(Debug)]
pub(crate) struct StateDir {
    dir: Dir,
    path: Utf8PathBuf,
}

impl StateDir {
    pub(crate) fn new(dir: Dir, path: Utf8PathBuf) -> StateDir {
        StateDir { dir, path }
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
