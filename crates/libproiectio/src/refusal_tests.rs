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
fn precedence_names_every_kind_exactly_once() {
    let kinds: BTreeSet<RefusalKind> = RefusalKind::PRECEDENCE.into_iter().collect();
    assert_eq!(kinds.len(), RefusalKind::PRECEDENCE.len());
    let all: BTreeSet<RefusalKind> = one_of_each().iter().map(|(_, r, _)| r.kind()).collect();
    assert_eq!(kinds, all);
}

#[test]
fn aggregate_reports_the_kind_precedence_ranks_first() {
    let each = one_of_each();
    let find = |kind: RefusalKind| {
        each.iter()
            .find(|(_, r, _)| r.kind() == kind)
            .cloned()
            .expect("one of each")
    };
    for pair in RefusalKind::PRECEDENCE.windows(2) {
        let (first, second) = (pair[0], pair[1]);
        // Fed lower-ranked first, so insertion order cannot be what decides.
        let refused = Refused::aggregate([find(second), find(first)]).expect("two refusals");
        assert_eq!(refused.kind, first, "{first:?} outranks {second:?}");
        assert_eq!(refused.paths.len(), 1);
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
    assert_eq!(refused.kind, RefusalKind::Foreign);
    assert_eq!(refused.keys().collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(refused.paths[Utf8Path::new("a")].origin, Origin::Files);
}

#[test]
fn aggregate_of_nothing_is_none() {
    assert_eq!(Refused::aggregate([]), None);
}

#[test]
fn one_takes_its_kind_from_the_refusal() {
    let refused = Refused::one(path("rc"), Refusal::Foreign, Origin::Caller);
    assert_eq!(refused.kind, RefusalKind::Foreign);
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
