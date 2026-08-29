//! An exclusive advisory `flock(2)` on [`LOCK_FILE_NAME`] in the state
//! directory. One lock covers one state directory: two projections sharing a
//! destination but keeping separate state directories do not exclude each
//! other. Advisory binds proiectio against proiectio: no other writer in the
//! destination is excluded.

use camino::Utf8Path;
use cap_std::fs_utf8::{Dir, File, OpenOptions};

use crate::observe::io_error;
use crate::{Error, LOCK_FILE_NAME, Result};

/// The single-writer guard on a state directory: while one is alive, no other
/// [`acquire`](StateLock::acquire) on the same state directory — thread or
/// process — succeeds. Dropping it releases the lock.
#[derive(Debug)]
pub(crate) struct StateLock {
    /// The `flock` belongs to this open file description; closing it releases.
    _file: File,
}

impl StateLock {
    /// Takes the lock on `state`'s [`LOCK_FILE_NAME`], creating the file if
    /// absent; never blocks — a lock held elsewhere reports
    /// [`Error::LockHeld`]. The file outlives the guard, never unlinked.
    pub(crate) fn acquire(state: &Dir) -> Result<StateLock> {
        let path = Utf8Path::new(LOCK_FILE_NAME);
        let file = state
            .open_with(path, OpenOptions::new().create(true).write(true))
            .map_err(io_error(path))?;
        match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(StateLock { _file: file }),
            // EWOULDBLOCK (EAGAIN on this platform family): held elsewhere.
            Err(errno) if errno == rustix::io::Errno::WOULDBLOCK => Err(Error::LockHeld {
                path: path.to_owned(),
            }),
            Err(errno) => Err(io_error(path)(errno.into())),
        }
    }
}

#[cfg(test)]
#[path = "lock_tests.rs"]
mod tests;
