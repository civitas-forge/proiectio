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
    // The state files would sit at the destination root with no subtree
    // to exclude, and the projection's own manifest would classify as
    // foreign.
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

/// The fact a caller reads to tell an empty manifest it meant from one it
/// misspelled, for a state directory beside the destination and for one under
/// it. A read never creates either, so the answer stays no until a write does.
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

/// The default state directory answers the same way: absent until a write
/// creates it, which is the ordinary state of a fresh destination.
#[test]
fn the_default_state_directory_is_absent_until_something_creates_it() {
    use crate::test_support::Tree;

    let dest = Tree::new().materialize();
    let projection = Projection::new(dest.root(), None).expect("a projection");

    assert!(!projection.state_dir_exists().expect("a state directory"));
    std::fs::create_dir(projection.state_dir()).expect("a state directory");
    assert!(projection.state_dir_exists().expect("a state directory"));
}
