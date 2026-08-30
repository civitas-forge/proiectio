//! The view models the commands render through; `status` renders
//! `libproiectio::Status` itself, so its structured output is the library's own.

mod config;
mod write;

pub(crate) use config::ConfigView;
pub(crate) use write::WriteView;
pub(crate) use write::lines as write_lines;
