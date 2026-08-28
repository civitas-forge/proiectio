//! File projection: map a computed tree of paths and contents onto a target
//! directory, record what was written in a manifest, and classify every path
//! before touching it.
//!
//! The model compares three trees pairwise: the *desired* tree the caller
//! passes ([`Entry`] values keyed by relative path), the *recorded* state in
//! the [`Manifest`], and the files on disk. Planning turns the comparison
//! into a [`Plan`] of per-path actions; applying executes the plan and
//! returns an [`ApplyReport`]; [`Status`] is the classification alone, with
//! nothing written.
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
//! [`Error::OwnerConflict`], [`Error::ExternalTarget`]) — from I/O and
//! format failures. A CLI derives its 0/1/2 exit contract from one match;
//! see [`Error::is_refusal`].

#![forbid(unsafe_code)]

mod containment;
mod entry;
mod error;
mod manifest;
mod mapping;
mod observe;
mod plan;
mod projection;
mod report;
mod status;
#[cfg(all(test, unix))]
mod test_support;

pub use containment::*;
pub use entry::*;
pub use error::*;
pub use manifest::*;
pub use mapping::*;
pub use observe::*;
pub use plan::*;
pub use projection::*;
pub use report::*;
pub use status::*;
