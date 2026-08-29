//! File projection: map a computed tree of paths and contents onto a target
//! directory, record what was written in a manifest, and classify every path
//! before touching it.
//!
//! The model compares three trees pairwise: the *desired* tree the caller
//! passes ([`Entry`] values keyed by relative path), the *recorded* state in
//! the [`Manifest`], and the files on disk. Comparing them produces a
//! [`Plan`] of per-path actions; executing one returns an [`ApplyReport`];
//! [`Status`] is the classification alone, with nothing written.
//!
//! [`Projection`] is the whole surface, and it is a validated pair of
//! absolute paths: the destination, and the state directory holding its
//! manifest. Nothing public takes or returns a directory handle — the
//! projection opens what a call needs. The reads take no lock
//! ([`Projection::status`], [`Projection::manifest`], [`Projection::plan`],
//! [`Projection::plan_removal`]); [`Projection::begin`] returns the [`Run`]
//! that holds the single-writer guard and is the only thing that can apply
//! ([`Run::apply`] takes no plan). `docs/design.lex` section 3 states the
//! rules that surface keeps, [`Run`] repeats them for a reader in the
//! rustdoc, and nothing else here restates them.
//!
//! The lock is built where `flock(2)` is, and so is [`Run`];
//! [`LOCK_FILE_NAME`] is spelled on every target, so a caller elsewhere can
//! coordinate on the same file.
//!
//! The crate carries no consumer vocabulary: content arrives as bytes,
//! owners are opaque strings, and nothing here names what the files are
//! for. A caller computes the desired tree itself, loads one from a TOML
//! mapping file with [`load_mapping`], walks a source directory into one
//! with [`load_tree`], which copies the tree verbatim — bytes, executable
//! bits, and symlink targets as written — or expands an archive into one
//! with [`load_archive`]. Verbatim within what UTF-8 can
//! name: a source tree holding a name or a symlink target with no UTF-8
//! spelling fails the load rather than being projected under some other
//! name.
//!
//! An archive is a tree constructor, not a node type. [`load_archive`] and
//! a mapping's `[archives."prefix/"]` entries expand tar, tar.gz, tar.zst,
//! and zip members into **ordinary** entries, hashed and tracked one per
//! file and symlink member, so nothing past the expansion is archive-aware.
//! Directory members carry no entry, as walked directories do not.
//!
//! # Exit contract
//!
//! [`Error`] separates *refusals* — the projection declining to touch a path
//! ([`Error::Drift`], [`Error::Foreign`], [`Error::Containment`],
//! [`Error::OwnerConflict`], [`Error::ExternalTarget`],
//! [`Error::InvalidTarget`], [`Error::TreeConflict`], [`Error::Block`]) —
//! from I/O and format failures. A CLI derives its 0/1/2 exit contract from
//! one match; see [`Error::is_refusal`]. Four of the refusals also name the
//! [`Origin`] of the tree that provoked them.

#![forbid(unsafe_code)]

#[cfg(unix)]
mod act;
mod archive;
mod block;
mod containment;
mod decide;
mod entry;
mod error;
// Narrower than the `unix` the other I/O modules carry: `rustix::fs::flock`,
// the lock's mechanism, is compiled out on the targets named here (Solaris
// has no `flock` at all), so `cfg(unix)` alone would select a module that
// cannot build there. The list is rustix's own, minus the non-Unix `wasi`,
// and has to follow `rustix::fs::flock`'s `cfg` when that dependency moves —
// nothing checks the two against each other. `run`, whose `Run` owns the
// guard, carries the same gate for the same reason, and the re-exports below
// repeat it because a `use` of a module a `cfg` removed does not compile.
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
mod origin;
mod plan;
mod projection;
mod report;
#[cfg(all(
    unix,
    not(any(
        target_os = "espidf",
        target_os = "horizon",
        target_os = "solaris",
        target_os = "vita"
    ))
))]
mod run;
mod status;
#[cfg(all(test, unix))]
mod test_support;
#[cfg(unix)]
mod tree;

#[cfg(unix)]
pub(crate) use act::*;
pub use archive::*;
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
pub(crate) use lock::*;
pub use manifest::*;
pub use mapping::*;
pub use observe::*;
pub use origin::*;
pub use plan::*;
pub use projection::*;
pub use report::*;
#[cfg(all(
    unix,
    not(any(
        target_os = "espidf",
        target_os = "horizon",
        target_os = "solaris",
        target_os = "vita"
    ))
))]
pub use run::*;
pub use status::*;
#[cfg(unix)]
pub use tree::*;
