//! The single-writer lock on the state directory
//! (`docs/implementation.lex` section 7): two processes applying to one
//! destination corrupt the manifest's read-modify-write — one loads the
//! manifest, the other persists, the first persists over it. Where the guard
//! is taken and how long it is held is the run's
//! ([`Run`](crate::Run)); this module is the guard itself.
//!
//! The lock is an exclusive advisory `flock(2)` on [`LOCK_FILE_NAME`],
//! beside the manifest in the state directory and opened through the same
//! capability handle. Advisory means it binds only proiectio against
//! proiectio, matching the `docs/security.lex` trust split: other writers
//! in the destination are the invoker's to coordinate. The mechanism is
//! `rustix::fs::flock` rather than a locking crate: rustix already
//! underlies cap-std in the dependency tree, and `flock` locks belong to
//! the open file description — two opens contend even inside one process,
//! so threads and processes behave identically.
//!
//! One lock covers one state directory, which is the resource section 7
//! names: the manifest, and the recording of the destination paths that
//! manifest owns. Two projections that share a target but keep separate
//! state directories therefore do not exclude each other — separate
//! manifests, separate locks — and this lock is not what makes that
//! configuration safe: their writes to a shared path race, and whoever set
//! up two recordings over one destination owns that, as with any other
//! writer in the destination. The lock file stays in the state directory
//! because §7's resource is the manifest; a lock file in the destination
//! would land as an unowned file inside the projected tree.

use camino::Utf8Path;
use cap_std::fs_utf8::{Dir, File, OpenOptions};

use crate::observe::io_error;
use crate::{Error, LOCK_FILE_NAME, Result};

/// The single-writer guard on a state directory: while a `StateLock` is
/// alive, no other [`acquire`](StateLock::acquire) on the same state
/// directory — thread or process — succeeds. Dropping it releases the lock,
/// because closing the file description releases its `flock`.
#[derive(Debug)]
pub(crate) struct StateLock {
    /// Held for the guard's lifetime: the advisory lock belongs to this
    /// open file description, so dropping (closing) the file releases it.
    _file: File,
}

impl StateLock {
    /// Takes the exclusive advisory lock on `state`'s
    /// [`LOCK_FILE_NAME`], creating the file if
    /// absent — try-lock semantics: a lock held by another writer reports
    /// [`Error::LockHeld`] immediately, never blocks. `LockHeld` is not a
    /// refusal ([`Error::is_refusal`] is `false`), so a CLI maps it to
    /// exit 1.
    ///
    /// The file outlives the guard: release never unlinks it, since
    /// unlinking would let a late writer create and lock a fresh inode
    /// while an earlier writer still holds the unlinked one. An empty
    /// leftover file costs nothing.
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
