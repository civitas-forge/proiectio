//! File projection: map a computed tree of paths and contents onto a target
//! directory, record what was written in a manifest, and classify every path
//! before touching it.
//!
//! [`Projection`] is the entry point, a validated pair of absolute paths: the
//! destination, and the state directory holding its manifest.
//! [`Projection::status`], [`Projection::manifest`], [`Projection::plan`] and
//! [`Projection::plan_removal`] read without locking; [`Projection::begin`]
//! returns the [`Run`] that holds the single-writer guard and applies.
//!
//! A caller computes the desired tree of [`Entry`] values itself, or builds
//! one with [`load_mapping`], [`load_tree`], or [`load_archive`]. Content is
//! bytes and owners are opaque strings; nothing here names what the files
//! are for.

#![forbid(unsafe_code)]

#[cfg(unix)]
mod act;
mod archive;
mod block;
mod containment;
mod decide;
mod entry;
mod error;
// `rustix::fs::flock` is compiled out on the targets named here (Solaris has
// no `flock` at all); the list is rustix's own, minus the non-Unix `wasi`.
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
