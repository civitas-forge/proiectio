//! The clapfig configuration schema and the one builder every consumer goes through.

use clapfig::{Clapfig, Schema, SearchPath, TypedBuilder};
use serde::{Deserialize, Serialize};

/// Projection settings as `proiectio.toml` declares them.
#[derive(Schema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProiectioConfig {
    /// The manifest owner a run records its entries under.
    #[clapfig(default = "default")]
    pub(crate) owner: String,
}

pub(crate) fn builder() -> TypedBuilder<ProiectioConfig> {
    Clapfig::typed::<ProiectioConfig>()
        .app_name("proiectio")
        .persist_scope("user", SearchPath::Platform)
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
