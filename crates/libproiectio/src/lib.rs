//! File projection: map a computed tree of paths and contents onto a target
//! directory, record what was written in a manifest, and classify every path
//! before touching it.
//!
//! The model compares three trees pairwise: the *desired* tree the caller
//! passes ([`Entry`] values keyed by relative path), the *recorded* state in
//! the [`Manifest`], and the files on disk — read once by [`observe`] into
//! an [`Observations`] snapshot. Planning ([`decide`]) turns the comparison
//! into a [`Plan`] of per-path actions; applying executes the plan and
//! returns an [`ApplyReport`]; [`Status`] is the classification
//! ([`classify`]) alone, with nothing written.
//!
//! Excluding a concurrent writer is the caller's to do: `StateLock` takes a
//! single-writer advisory lock on the state directory, and a caller that
//! can race another proiectio process acquires it before
//! `load_manifest` — the read the whole cycle hangs off — and holds the
//! guard until the manifest has been persisted. The functions here take
//! plain capability handles and never acquire it themselves, so the
//! manifest's read-modify-write survives concurrent runs only when the
//! caller brackets them that way (`docs/implementation.lex` section 7).
//! `StateLock` is built where `flock(2)` is; [`LOCK_FILE_NAME`] is spelled
//! on every target, so a caller elsewhere can coordinate on the same file.
//!
//! The crate carries no consumer vocabulary: content arrives as bytes,
//! owners are opaque strings, and nothing here names what the files are
//! for. A caller computes the desired tree itself or loads one from a TOML
//! mapping file with [`load_mapping`].
//!
//! # Exit contract
//!
//! [`Error`] separates *refusals* — the projection declining to touch a path
//! ([`Error::Drift`], [`Error::Foreign`], [`Error::Containment`],
//! [`Error::OwnerConflict`], [`Error::ExternalTarget`],
//! [`Error::TreeConflict`]) — from I/O and
//! format failures. A CLI derives its 0/1/2 exit contract from one match;
//! see [`Error::is_refusal`].

#![forbid(unsafe_code)]

#[cfg(unix)]
mod act;
mod containment;
mod decide;
mod entry;
mod error;
// Narrower than the `unix` the other I/O modules carry: `rustix::fs::flock`,
// the lock's mechanism, is compiled out on the targets named here (Solaris
// has no `flock` at all), so `cfg(unix)` alone would select a module that
// cannot build there. The list is rustix's own, minus the non-Unix `wasi`,
// and has to follow `rustix::fs::flock`'s `cfg` when that dependency moves —
// nothing checks the two against each other. The re-export below repeats the
// gate because a `use` of a module a `cfg` removed does not compile; the
// reverse slip, a module built but not re-exported, leaves `StateLock`
// unreachable and so dead code, which the `-D warnings` clippy run rejects.
#[cfg(all(
    unix,
    not(any(
        target_os = "espidf",
        target_os = "horizon",
        target_os = "solaris",
        target_os = "vita"
    ))
))]
mod lock;
mod manifest;
mod mapping;
mod observe;
mod plan;
mod projection;
mod report;
mod status;
#[cfg(all(test, unix))]
mod test_support;

#[cfg(unix)]
pub use act::*;
pub use containment::*;
pub use decide::*;
pub use entry::*;
pub use error::*;
#[cfg(all(
    unix,
    not(any(
        target_os = "espidf",
        target_os = "horizon",
        target_os = "solaris",
        target_os = "vita"
    ))
))]
pub use lock::*;
pub use manifest::*;
pub use mapping::*;
pub use observe::*;
pub use plan::*;
pub use projection::*;
pub use report::*;
pub use status::*;
