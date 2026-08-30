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
//! one with [`load_mapping`], [`load_tree`], [`load_archive`], or
//! [`load_files`]. Content is bytes and owners are opaque strings; nothing
//! here names what the files are for.
//!
//! [`Error::is_refusal`] splits refusals from runtime failures, which a CLI's
//! 0/1/2 exit contract matches on.

#![forbid(unsafe_code)]

#[cfg(not(unix))]
compile_error!(
    "libproiectio requires a Unix target: it projects through cap-std directory \
     handles and guards writes with flock(2)"
);

mod act;
mod apply_report;
mod archive;
mod block;
mod containment;
mod decide;
mod desired;
mod entry;
mod error;
mod files;
mod lock;
mod manifest;
mod mapping;
mod observe;
mod origin;
mod plan;
mod projection;
mod refusal;
mod report;
mod run;
mod status;
#[cfg(test)]
mod test_support;
mod tree;

pub(crate) use act::*;
pub use apply_report::*;
pub use archive::*;
pub use containment::*;
pub use decide::*;
pub use desired::*;
pub use entry::*;
pub use error::*;
pub use files::*;
pub(crate) use lock::*;
pub use manifest::*;
pub use mapping::*;
pub use observe::*;
pub use origin::*;
pub use plan::*;
pub use projection::*;
pub use refusal::*;
pub use report::*;
pub use run::*;
pub use status::*;
pub use tree::*;
