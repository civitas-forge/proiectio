use camino::Utf8PathBuf;

use super::*;

#[test]
fn absolute_paths_are_kept_as_given() {
    let projection = Projection::new(
        Utf8PathBuf::from("/srv/site"),
        Utf8PathBuf::from("/srv/site/.proiectio"),
    );

    assert_eq!(projection.target(), "/srv/site");
    assert_eq!(projection.state_dir(), "/srv/site/.proiectio");
}

#[test]
fn state_prefix_is_the_state_dirs_path_inside_the_target() {
    let projection = Projection::new(
        Utf8PathBuf::from("/srv/site"),
        Utf8PathBuf::from("/srv/site/.proiectio"),
    );

    assert_eq!(projection.state_prefix(), Some(Utf8Path::new(".proiectio")));
}

#[test]
fn state_prefix_is_none_for_a_state_dir_outside_the_target() {
    let projection = Projection::new(
        Utf8PathBuf::from("/srv/site"),
        Utf8PathBuf::from("/var/state/site"),
    );

    assert_eq!(projection.state_prefix(), None);
}

#[test]
fn state_prefix_is_none_for_a_state_dir_equal_to_the_target() {
    let projection = Projection::new(
        Utf8PathBuf::from("/srv/site"),
        Utf8PathBuf::from("/srv/site"),
    );

    assert_eq!(projection.state_prefix(), None);
}

#[test]
#[should_panic(expected = "target must be absolute")]
fn a_relative_target_is_rejected() {
    Projection::new(
        Utf8PathBuf::from("site"),
        Utf8PathBuf::from("/srv/site/.proiectio"),
    );
}

#[test]
#[should_panic(expected = "state_dir must be absolute")]
fn a_relative_state_dir_is_rejected() {
    Projection::new(Utf8PathBuf::from("/srv/site"), Utf8PathBuf::from("state"));
}
