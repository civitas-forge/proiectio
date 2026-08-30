use std::collections::BTreeSet;

use camino::{Utf8Path, Utf8PathBuf};

use super::*;

fn path(s: &str) -> Utf8PathBuf {
    Utf8PathBuf::from(s)
}

/// One refusal of each kind, keyed by a path that names the kind.
fn one_of_each() -> Vec<(Utf8PathBuf, Refusal, Origin)> {
    vec![
        (path("drift"), Refusal::Drift, Origin::Caller),
        (path("foreign"), Refusal::Foreign, Origin::Caller),
        (path("containment"), Refusal::Containment, Origin::Caller),
        (
            path("tree"),
            Refusal::TreeConflict {
                paths: BTreeSet::from([path("tree/below")]),
            },
            Origin::Caller,
        ),
        (
            path("owner"),
            Refusal::OwnerConflict {
                owners: BTreeSet::from(["site".to_owned()]),
            },
            Origin::Caller,
        ),
        (
            path("external"),
            Refusal::ExternalTarget {
                target: "/opt".to_owned(),
            },
            Origin::Caller,
        ),
        (
            path("invalid"),
            Refusal::InvalidTarget {
                target: String::new(),
            },
            Origin::Caller,
        ),
        (
            path("block"),
            Refusal::Block {
                fault: BlockFault::MarkerEmpty,
            },
            Origin::Caller,
        ),
    ]
}

#[test]
fn one_of_each_covers_every_kind() {
    let kinds: BTreeSet<RefusalKind> = one_of_each().iter().map(|(_, r, _)| r.kind()).collect();
    // Adding a kind: add it to `one_of_each`, and this arm list fails to
    // compile until it is named here too.
    for kind in &kinds {
        match kind {
            RefusalKind::Containment
            | RefusalKind::TreeConflict
            | RefusalKind::Foreign
            | RefusalKind::Drift
            | RefusalKind::OwnerConflict
            | RefusalKind::ExternalTarget
            | RefusalKind::InvalidTarget
            | RefusalKind::Block => {}
        }
    }
    assert_eq!(kinds.len(), 8);
}

#[test]
fn precedence_is_declaration_order() {
    assert_eq!(
        one_of_each()
            .iter()
            .map(|(_, r, _)| r.kind())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>(),
        [
            RefusalKind::Containment,
            RefusalKind::TreeConflict,
            RefusalKind::Foreign,
            RefusalKind::Drift,
            RefusalKind::OwnerConflict,
            RefusalKind::ExternalTarget,
            RefusalKind::InvalidTarget,
            RefusalKind::Block,
        ]
    );
}

#[test]
fn aggregate_reports_the_least_kind() {
    let each = one_of_each();
    let mut ranked: Vec<_> = each.clone();
    ranked.sort_by_key(|(_, r, _)| r.kind());
    for pair in ranked.windows(2) {
        let (first, second) = (&pair[0], &pair[1]);
        // Fed lower-ranked first, so insertion order cannot be what decides.
        let refused = Refused::aggregate([second.clone(), first.clone()]).expect("two refusals");
        assert_eq!(
            refused.kind(),
            first.1.kind(),
            "{:?} outranks {:?}",
            first.1,
            second.1
        );
        assert_eq!(refused.paths().len(), 1);
    }
}

#[test]
fn aggregate_keeps_every_path_of_the_reported_kind_and_drops_the_rest() {
    let refused = Refused::aggregate([
        (path("b"), Refusal::Foreign, Origin::Caller),
        (path("a"), Refusal::Foreign, Origin::Files),
        (path("c"), Refusal::Drift, Origin::Caller),
    ])
    .expect("refusals");
    assert_eq!(refused.kind(), RefusalKind::Foreign);
    assert_eq!(refused.keys().collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(refused.paths()[Utf8Path::new("a")].origin, Origin::Files);
}

#[test]
fn aggregate_of_nothing_is_none() {
    assert_eq!(Refused::aggregate([]), None);
}

#[test]
fn one_takes_its_kind_from_the_refusal() {
    let refused = Refused::one(path("rc"), Refusal::Foreign, Origin::Caller);
    assert_eq!(refused.kind(), RefusalKind::Foreign);
    assert_eq!(refused.keys().collect::<Vec<_>>(), ["rc"]);
}

#[test]
fn messages_open_with_the_kind_and_name_each_path_with_its_detail() {
    let rendered: Vec<String> = one_of_each()
        .into_iter()
        .map(|(path, refusal, origin)| Refused::one(path, refusal, origin).to_string())
        .collect();
    assert_eq!(
        rendered,
        [
            "refusing to touch drifted paths (edited on disk): drift",
            "refusing to touch foreign paths (not written by this projection): foreign",
            "refusing paths that violate containment: containment",
            "refusing desired paths that claim overlapping locations: tree (with tree/below)",
            "refusing paths whose desired entries conflict with another owner's: \
             owner (held by site)",
            "refusing symlinks with targets outside the destination: external -> /opt",
            "refusing symlinks whose targets are not paths: invalid -> \"\"",
            "refusing block entries: block (the marker is empty)",
        ]
    );
}

#[test]
fn messages_name_each_path_own_source_after_its_detail() {
    let mapping = Origin::Mapping {
        path: "/etc/harness/skills.toml".into(),
    };
    let refused = Refused::aggregate([
        (
            path("a"),
            Refusal::TreeConflict {
                paths: BTreeSet::from([path("a/b")]),
            },
            mapping,
        ),
        (
            path("a/b"),
            Refusal::TreeConflict {
                paths: BTreeSet::from([path("a")]),
            },
            Origin::Caller,
        ),
    ])
    .expect("refusals");
    assert_eq!(
        refused.to_string(),
        "refusing desired paths that claim overlapping locations: \
         a (with a/b) (from mapping /etc/harness/skills.toml), a/b (with a)"
    );
}

#[test]
fn sourced_by_takes_the_keys_origin_or_else_the_acted_on_keys() {
    let mapping = Origin::Mapping {
        path: "/maps/deploy.toml".into(),
    };
    let plan = crate::Plan {
        dropped: BTreeSet::new(),
        owner: "own".to_owned(),
        origins: BTreeMap::from([(path("a"), Origin::Files), (path("d/f"), mapping.clone())]),
        external_targets: Default::default(),
        actions: BTreeMap::new(),
    };
    let refused = Refused::aggregate([
        (path("a"), Refusal::Drift, Origin::Caller),
        (path("d"), Refusal::Drift, Origin::Caller),
    ])
    .expect("refusals")
    .sourced_by(&plan, Utf8Path::new("d/f"));
    // `a` is a planned key with its own origin; `d` is not planned, so it
    // takes the origin of the key that was being acted on.
    assert_eq!(refused.paths()[Utf8Path::new("a")].origin, Origin::Files);
    assert_eq!(refused.paths()[Utf8Path::new("d")].origin, mapping);

    let unplanned = Refused::one(path("d"), Refusal::Foreign, Origin::Caller)
        .sourced_by(&plan, Utf8Path::new("elsewhere"));
    assert_eq!(unplanned.paths()[Utf8Path::new("d")].origin, Origin::Caller);
}

// A `Report<Refusal>` summary orders by `Refusal`, an error's aggregate by
// `RefusalKind`. The two agree only while the variants are declared in the
// same order.
#[test]
fn refusals_sort_in_the_same_order_as_their_kinds() {
    let mut refusals: Vec<Refusal> = one_of_each().into_iter().map(|(_, r, _)| r).collect();
    refusals.sort();
    let mut kinds: Vec<RefusalKind> = one_of_each().iter().map(|(_, r, _)| r.kind()).collect();
    kinds.sort();

    assert_eq!(
        refusals.iter().map(Refusal::kind).collect::<Vec<_>>(),
        kinds
    );
}
