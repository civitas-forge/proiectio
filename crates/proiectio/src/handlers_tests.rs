use super::*;

use libproiectio::PathState;
use tempfile::TempDir;

use crate::testing::{classified, utf8};

/// The adapter, called as the typed function the `#[handler]` macro preserves:
/// the two options become a [`Projection`], and what comes back is the
/// library's own report, unmapped.
#[test]
fn the_two_options_become_the_projection_the_library_classifies() {
    let dir = TempDir::new().expect("a temporary directory");
    let dest = utf8(&dir);
    classified(&dest);

    let Output::Render(report) = status(dest.to_string(), None).expect("a status") else {
        panic!("expected rendered data");
    };

    let verdicts: Vec<_> = report
        .iter()
        .map(|(path, row)| (path.to_string(), row.verdict))
        .collect();
    assert_eq!(
        verdicts,
        vec![
            ("bin".to_owned(), PathState::Foreign),
            ("bin/tool".to_owned(), PathState::Drifted),
            ("config".to_owned(), PathState::Foreign),
            ("config/settings.toml".to_owned(), PathState::Clean),
            ("current".to_owned(), PathState::Missing),
        ]
    );
}

/// The state directory named apart from the destination is read there, so a
/// destination whose manifest lives elsewhere classifies as recorded.
#[test]
fn a_state_directory_outside_the_destination_still_names_the_manifest() {
    let dir = TempDir::new().expect("a temporary directory");
    let dest = utf8(&dir);
    classified(&dest);

    let Output::Render(report) =
        status(dest.to_string(), Some(dest.join(".proiectio").to_string())).expect("a status")
    else {
        panic!("expected rendered data");
    };

    assert_eq!(report.rows.len(), 5);
}

/// A destination the projection cannot open is an operational failure, and it
/// reaches the shell declaring status 1.
#[test]
fn a_destination_that_is_not_there_fails_with_status_one() {
    let dir = TempDir::new().expect("a temporary directory");
    let absent = utf8(&dir).join("absent");

    let error = status(absent.to_string(), None).expect_err("an operational failure");
    let external = error
        .downcast::<standout::cli::ExternalFailure>()
        .expect("an external failure");

    assert_eq!(external.exit_status().code(), exit::FAILURE);
}

/// A state directory that is the destination is `StateDirIsTarget`, which the
/// library does not carry as a refusal, so the shell spends 1 on it and not 2.
#[test]
fn a_state_directory_that_is_the_destination_fails_with_status_one() {
    let dir = TempDir::new().expect("a temporary directory");
    let dest = utf8(&dir);

    let error =
        status(dest.to_string(), Some(dest.to_string())).expect_err("a refused state directory");
    let external = error
        .downcast::<standout::cli::ExternalFailure>()
        .expect("an external failure");

    assert_eq!(external.exit_status().code(), exit::FAILURE);
}
