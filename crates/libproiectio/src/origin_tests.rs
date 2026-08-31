use camino::Utf8PathBuf;

use super::*;
use crate::{Error, Refusal, Refused};

fn containment(origin: Origin) -> Error {
    Refused::one(
        Utf8PathBuf::from("../etc/passwd"),
        Refusal::Containment { through: None },
        origin,
    )
    .into()
}

#[test]
fn every_variant_renders_its_source() {
    let table = [
        (Origin::Caller, ""),
        (
            Origin::Mapping {
                path: "/etc/harness/skills.toml".into(),
            },
            "from mapping /etc/harness/skills.toml",
        ),
        (
            Origin::Tree {
                path: "/srv/skeleton".into(),
            },
            "from tree /srv/skeleton",
        ),
        (
            Origin::Archive {
                path: "/srv/vendor.tar.gz".into(),
                via: None,
            },
            "from archive /srv/vendor.tar.gz",
        ),
        (
            Origin::Archive {
                path: "/srv/vendor.tar.gz".into(),
                via: Some("/etc/harness/skills.toml".into()),
            },
            "from archive /srv/vendor.tar.gz, named by mapping /etc/harness/skills.toml",
        ),
        (Origin::Files, "from individually named files"),
    ];

    for (origin, rendering) in table {
        assert_eq!(origin.to_string(), rendering);
    }
}

// The no-origin case reads as a plain refusal rather than apologising for
// having no source to name.
#[test]
fn a_caller_computed_tree_adds_nothing_to_the_message() {
    assert_eq!(
        containment(Origin::Caller).to_string(),
        "refusing paths that violate containment: ../etc/passwd"
    );
}

#[test]
fn a_named_source_reaches_the_message() {
    assert_eq!(
        containment(Origin::Mapping {
            path: "/etc/harness/skills.toml".into(),
        })
        .to_string(),
        "refusing paths that violate containment: \
         ../etc/passwd (from mapping /etc/harness/skills.toml)"
    );
}

// A removal is decided from the manifest, so it has no source tree to name.
#[test]
fn the_default_origin_is_the_caller() {
    assert_eq!(Origin::default(), Origin::Caller);
}
