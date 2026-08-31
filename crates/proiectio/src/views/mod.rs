//! The view models the commands render through; `status` renders
//! `libproiectio::Status` itself, so its structured output is the library's own.

use standout::AmbiguousWidth;
use standout::tabular::visible_width_with_policy;

mod cells;
mod config;
mod run;
mod status;

pub(crate) use config::ConfigView;
pub(crate) use run::csv as run_csv;
pub(crate) use run::lines as run_lines;
pub(crate) use run::{AbortedRun, PlannedRun, RunView, refused_rows};
pub(crate) use status::csv as status_csv;
pub(crate) use status::lines as status_lines;

/// The spaces that carry `cell` out to the width of its column.
fn pad(column: usize, cell: &str, width: AmbiguousWidth) -> String {
    " ".repeat(column.saturating_sub(visible_width_with_policy(cell, width)))
}
