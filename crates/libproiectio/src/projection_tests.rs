use camino::Utf8Path;

use super::*;

fn projection(target: &str, state_dir: Option<&str>) -> Result<Projection> {
    Projection::new(Utf8Path::new(target), state_dir.map(Utf8Path::new))
}

#[test]
fn absolute_paths_are_kept_as_given() {
    let projection = projection("/srv/site", Some("/srv/site/.proiectio")).expect("a projection");

    assert_eq!(projection.target(), "/srv/site");
    assert_eq!(projection.state_dir(), "/srv/site/.proiectio");
}

#[test]
fn state_prefix_is_the_state_dirs_path_inside_the_target() {
    let projection = projection("/srv/site", Some("/srv/site/.proiectio")).expect("a projection");

    assert_eq!(projection.state_prefix(), Some(Utf8Path::new(".proiectio")));
}

#[test]
fn state_prefix_is_none_for_a_state_dir_outside_the_target() {
    let projection = projection("/srv/site", Some("/var/state/site")).expect("a projection");

    assert_eq!(projection.state_prefix(), None);
}

#[test]
fn an_omitted_state_dir_defaults_to_proiectio_under_the_target() {
    let projection = projection("/srv/site", None).expect("a projection");

    assert_eq!(projection.state_dir(), "/srv/site/.proiectio");
    assert_eq!(projection.state_prefix(), Some(Utf8Path::new(".proiectio")));
}

#[test]
fn a_state_dir_equal_to_the_target_is_rejected() {
    assert!(matches!(
        projection("/srv/site", Some("/srv/site")).unwrap_err(),
        Error::StateDirIsTarget { path } if path == "/srv/site"
    ));
}

#[test]
fn a_state_dir_spelling_the_target_through_parent_components_is_rejected() {
    assert!(matches!(
        projection("/srv/site", Some("/srv/site/cache/..")).unwrap_err(),
        Error::StateDirIsTarget { path } if path == "/srv/site"
    ));
}

#[test]
fn parent_components_collapse_before_the_paths_are_compared() {
    let projection =
        projection("/srv/www/../site", Some("/srv/site/./.proiectio")).expect("a projection");

    assert_eq!(projection.target(), "/srv/site");
    assert_eq!(projection.state_dir(), "/srv/site/.proiectio");
    assert_eq!(projection.state_prefix(), Some(Utf8Path::new(".proiectio")));
}

#[test]
fn relative_paths_resolve_against_the_current_directory() {
    let cwd = absolutize(Utf8Path::new(".")).expect("a current directory");
    let projection = projection("site", Some("state")).expect("a projection");

    assert_eq!(projection.target(), cwd.join("site"));
    assert_eq!(projection.state_dir(), cwd.join("state"));
}

#[test]
fn a_projection_starts_with_no_pruned_components() {
    let projection = projection("/srv/site", None).expect("a projection");

    assert!(projection.pruned_components().is_empty());
}

#[test]
fn pruned_components_are_deduplicated_and_sorted() {
    let projection = projection("/srv/site", None)
        .expect("a projection")
        .with_pruned_components(["vendor", ".git", "vendor"])
        .expect("path components");

    assert_eq!(
        projection
            .pruned_components()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec![".git", "vendor"]
    );
}

#[test]
fn setting_pruned_components_replaces_the_previous_set() {
    let projection = projection("/srv/site", None)
        .expect("a projection")
        .with_pruned_components([".git", "vendor"])
        .expect("path components")
        .with_pruned_components(["cache"])
        .expect("path components");

    assert_eq!(
        projection.pruned_components(),
        &std::collections::BTreeSet::from(["cache".to_owned()])
    );
}

#[test]
fn a_pruned_name_must_be_one_path_component() {
    for component in ["", ".", "..", "a/b", "nul\0byte"] {
        assert!(matches!(
            projection("/srv/site", None)
                .expect("a projection")
                .with_pruned_components([component])
                .unwrap_err(),
            Error::InvalidPrunedComponent { component: rejected } if rejected == component
        ));
    }
}

#[test]
fn an_in_target_state_directory_cannot_enter_a_pruned_component() {
    for (state_dir, component) in [
        (None, ".proiectio"),
        (Some("/srv/site/state/.git/data"), ".git"),
    ] {
        assert!(matches!(
            projection("/srv/site", state_dir)
                .expect("a projection")
                .with_pruned_components([component])
                .expect_err("state must remain in scope"),
            Error::StateDirPruned { path, component: rejected }
                if path == Utf8Path::new(state_dir.unwrap_or("/srv/site/.proiectio"))
                    && rejected == component
        ));
    }
}

#[test]
fn an_external_state_directory_may_share_a_pruned_component_name() {
    projection("/srv/site", Some("/var/state/.git/proiectio"))
        .expect("a projection")
        .with_pruned_components([".git"])
        .expect("only destination-relative components are pruned");
}

#[test]
fn a_state_directory_states_whether_it_is_there() {
    use crate::test_support::Tree;

    let dest = Tree::new().materialize();
    let elsewhere = Tree::new().materialize();

    for (case, state_dir) in [
        ("beside the destination", elsewhere.path("never-created")),
        ("under the destination", dest.path("never-created")),
    ] {
        let projection = Projection::new(dest.root(), Some(&state_dir)).expect("a projection");

        assert!(
            !projection.state_dir_exists().expect("a state directory"),
            "{case}"
        );
        std::fs::create_dir(&state_dir).expect("a state directory");
        assert!(
            projection.state_dir_exists().expect("a state directory"),
            "{case}"
        );
    }
}

#[test]
fn the_default_state_directory_is_absent_until_something_creates_it() {
    use crate::test_support::Tree;

    let dest = Tree::new().materialize();
    let projection = Projection::new(dest.root(), None).expect("a projection");

    assert!(!projection.state_dir_exists().expect("a state directory"));
    std::fs::create_dir(projection.state_dir()).expect("a state directory");
    assert!(projection.state_dir_exists().expect("a state directory"));
}

#[test]
fn a_destination_that_is_not_there_is_named_as_the_destination() {
    use crate::test_support::Tree;

    let dest = Tree::new().materialize();
    let gone = dest.path("gone");
    let projection = Projection::new(&gone, None).expect("a projection");

    let error = projection.status().expect_err("no such destination");

    assert!(
        matches!(
            &error,
            Error::Io {
                role: IoRole::Destination,
                path,
                source,
            } if *path == gone && source.kind() == std::io::ErrorKind::NotFound
        ),
        "got {error:?}"
    );
    assert_eq!(
        error.to_string(),
        format!("destination {gone}: No such file or directory (os error 2)")
    );
}
