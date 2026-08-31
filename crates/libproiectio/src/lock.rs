//! An exclusive advisory `flock(2)` on [`LOCK_FILE_NAME`] in the state
//! directory. One lock covers one state directory: two projections sharing a
//! destination but keeping separate state directories do not exclude each
//! other. Advisory binds proiectio against proiectio: no other writer in the
//! destination is excluded.

use cap_std::fs_utf8::{File, OpenOptions};

use crate::observe::io_error;
use crate::{Error, LOCK_FILE_NAME, Result, StateDir};

/// The single-writer guard on a state directory: while one is alive, no other
/// [`acquire`](StateLock::acquire) on the same state directory — thread or
/// process — succeeds. Dropping it releases the lock.
#[derive(Debug)]
pub(crate) struct StateLock {
    /// The `flock` belongs to this open file description, which
    /// [`Drop`](StateLock::drop) unlocks before closing.
    file: File,
}

impl StateLock {
    /// Takes the lock on `state`'s [`LOCK_FILE_NAME`], creating the file if
    /// absent; never blocks — a lock held elsewhere reports
    /// [`Error::LockHeld`]. The file outlives the guard, never unlinked.
    pub(crate) fn acquire(state: &StateDir) -> Result<StateLock> {
        let path = state.path_of(LOCK_FILE_NAME);
        let file = state
            .dir()
            .open_with(LOCK_FILE_NAME, OpenOptions::new().create(true).write(true))
            .map_err(io_error(&path))?;
        match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(StateLock { file }),
            // EWOULDBLOCK (EAGAIN on this platform family): held elsewhere.
            Err(errno) if errno == rustix::io::Errno::WOULDBLOCK => Err(Error::LockHeld { path }),
            Err(errno) => Err(io_error(&path)(errno.into())),
        }
    }
}

impl Drop for StateLock {
    // The close releases the `flock` too, but on macOS not always before the
    // next `acquire` runs: a release left to the close lands milliseconds
    // late under load, and the acquisition meanwhile meets `LockHeld`.
    fn drop(&mut self) {
        let _ = rustix::fs::flock(&self.file, rustix::fs::FlockOperation::Unlock);
    }
}

#[cfg(test)]
#[path = "lock_tests.rs"]
mod tests;
