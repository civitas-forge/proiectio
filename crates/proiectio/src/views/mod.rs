//! The view models the commands render through; `status` renders
//! `libproiectio::Status` itself, so its structured output is the library's own.

mod config;
mod run;

pub(crate) use config::ConfigView;
pub(crate) use run::lines as run_lines;
pub(crate) use run::{PlannedRun, RunView};
