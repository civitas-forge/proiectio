//! The document `write` renders.

use libproiectio::{ApplyReport, PlannedAction, Report};
use serde::Serialize;

/// A plan on a dry run, what apply did on a real one; untagged, so structured
/// output is the library's own either way.
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum WriteView {
    Planned(Report<PlannedAction>),
    Applied(Box<ApplyReport>),
}
