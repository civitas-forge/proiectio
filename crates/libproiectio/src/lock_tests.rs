use super::*;
use crate::test_support::{Tree, state_at};

#[test]
fn acquire_creates_the_lock_file_in_the_state_dir() {
    let state = Tree::new().materialize();
    let _guard = StateLock::acquire(&state_at(state.root())).expect("acquire uncontended lock");
    assert!(state.path(LOCK_FILE_NAME).is_file());
}

#[test]
fn dropping_the_guard_lets_the_next_writer_acquire() {
    let state = Tree::new().materialize();
    let dir = state_at(state.root());
    let guard = StateLock::acquire(&dir).expect("first acquire");
    drop(guard);
    StateLock::acquire(&dir).expect("acquire after drop");
}

// The contention contract: a second writer — its own thread, its own open
// of the state directory, as a concurrent process would hold — gets
// [`Error::LockHeld`] immediately (try-lock, never a hang), and succeeds
// once the first guard drops.
#[test]
fn contended_acquire_reports_lock_held_immediately() {
    let state = Tree::new().materialize();
    let guard = StateLock::acquire(&state_at(state.root())).expect("first acquire");

    let root = state.root().to_owned();
    let contender = std::thread::spawn({
        let root = root.clone();
        move || StateLock::acquire(&state_at(&root))
    });
    let error = contender
        .join()
        .expect("contender thread")
        .expect_err("the lock is held");
    let lock = state.path(LOCK_FILE_NAME);
    match &error {
        Error::LockHeld { path } => assert_eq!(*path, lock),
        other => panic!("expected LockHeld, got {other:?}"),
    }
    assert!(!error.is_refusal(), "a contended lock is exit-1 territory");
    assert_eq!(
        error.to_string(),
        format!("state lock {lock} is held by another writer")
    );

    drop(guard);
    let successor = std::thread::spawn(move || StateLock::acquire(&state_at(&root)).map(drop));
    successor
        .join()
        .expect("successor thread")
        .expect("acquire once the first guard dropped");
}
