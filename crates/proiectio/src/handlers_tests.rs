use super::*;

use std::rc::Rc;

use libproiectio::PathState;
use standout_dispatch::Extensions;
use tempfile::TempDir;

use crate::testing::{classified, projected, utf8};

/// A context carrying the verdict cell the shell reads the exit status back
/// from, so a typed call can be asked what status the run leaves with.
fn context(verdict: &exit::Verdict) -> CommandContext {
    let mut state = Extensions::new();
    state.insert(verdict.clone());
    CommandContext::new(vec!["status".to_owned()], Rc::new(state))
}

/// The status one classification leaves the process with, and the warnings it
/// wrote about the run. The collector is a thread-local the test harness
/// drains per run, and each test owns its own thread.
fn checked(dest: &Utf8Path, state_dir: Option<&Utf8Path>, check: bool) -> (u8, Vec<String>) {
    let _ = standout::warnings::drain_warnings();
    let verdict = exit::Verdict::default();
    status(
        dest.to_string(),
        state_dir.map(Utf8Path::to_string),
        check,
        &context(&verdict),
    )
    .expect("a status");
    (verdict.over(exit::OK), standout::warnings::drain_warnings())
}

/// The adapter, called as the typed function the `#[handler]` macro preserves:
/// the two options become a [`Projection`], and what comes back is the
/// library's own report, unmapped.
#[test]
fn the_two_options_become_the_projection_the_library_classifies() {
    let dir = TempDir::new().expect("a temporary directory");
    let dest = utf8(&dir);
    classified(&dest);

    let Output::Render(report) = status(
        dest.to_string(),
        None,
        false,
        &context(&exit::Verdict::default()),
    )
    .expect("a status") else {
        panic!("expected rendered data");
    };

    let verdicts: Vec<_> = report
        .iter()
        .map(|(path, row)| (path.to_string(), row.verdict))
        .collect();
    assert_eq!(
        verdicts,
        vec![
            ("bin/tool".to_owned(), PathState::Drifted),
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

    let Output::Render(report) = status(
        dest.to_string(),
        Some(dest.join(".proiectio").to_string()),
        false,
        &context(&exit::Verdict::default()),
    )
    .expect("a status") else {
        panic!("expected rendered data");
    };

    assert_eq!(report.rows.len(), 3);
}

/// One row of the `--check` contract: what to call the destination, what to
/// do to it after it is projected, and the status `--check` leaves with.
type Case = (&'static str, Option<fn(&Utf8Path)>, u8);

/// The verdict `--check` records: everything clean passes, and each of the
/// three ways a path can differ from the manifest spends the refusal status.
/// Plain `status` leaves 0 on all four.
#[test]
fn check_records_the_refusal_status_for_every_state_but_clean() {
    let cases: [Case; 4] = [
        ("clean", None, exit::OK),
        (
            "drifted",
            Some(|dest| {
                std::fs::write(dest.join("bin/tool"), b"#!/bin/sh\necho edited\n")
                    .expect("an edited file");
            }),
            exit::REFUSAL,
        ),
        (
            "missing",
            Some(|dest| {
                std::fs::remove_file(dest.join("current")).expect("a removed file");
            }),
            exit::REFUSAL,
        ),
        (
            "foreign",
            Some(|dest| {
                std::fs::write(dest.join("stray.txt"), b"not ours\n").expect("a stray file");
            }),
            exit::REFUSAL,
        ),
    ];

    for (case, differ, expected) in cases {
        let dir = TempDir::new().expect("a temporary directory");
        let dest = utf8(&dir);
        projected(&dest);
        if let Some(differ) = differ {
            differ(&dest);
        }

        let (checking, _) = checked(&dest, None, true);
        let (plain, _) = checked(&dest, None, false);

        assert_eq!(checking, expected, "--check on a {case} destination");
        assert_eq!(plain, exit::OK, "plain status on a {case} destination");
    }
}

/// A `--state-dir` the filesystem does not have warns, and the warning names
/// the path and what the classification did instead of reading it.
#[test]
fn a_named_state_directory_that_is_not_there_warns() {
    let dir = TempDir::new().expect("a temporary directory");
    let dest = utf8(&dir);
    classified(&dest);
    let absent = dest.join("no-such-state");

    let (plain, warnings) = checked(&dest, Some(&absent), false);

    assert_eq!(plain, exit::OK, "plain status still exits 0");
    assert_eq!(
        warnings,
        vec![format!(
            "state dir {absent} does not exist; treating manifest as empty"
        )]
    );
}

/// The typo a gate has to fail on: the report reads every path foreign and
/// says nothing is wrong with the destination, so the missing directory
/// itself is what `--check` spends the refusal on.
#[test]
fn check_refuses_a_named_state_directory_that_is_not_there() {
    let dir = TempDir::new().expect("a temporary directory");
    let dest = utf8(&dir);

    let (checking, _) = checked(&dest, Some(&dest.join("no-such-state")), true);

    assert_eq!(
        checking,
        exit::REFUSAL,
        "an empty destination hides the typo behind an empty report"
    );
}

/// The default state directory is absent until something is projected, so its
/// absence is the ordinary state of a fresh destination rather than a mistake:
/// no warning, and `--check` passes an empty destination whose empty manifest
/// agrees with it.
#[test]
fn the_default_state_directory_being_absent_is_neither_a_warning_nor_a_refusal() {
    let dir = TempDir::new().expect("a temporary directory");
    let dest = utf8(&dir);

    let (checking, warnings) = checked(&dest, None, true);

    assert_eq!(checking, exit::OK);
    assert!(warnings.is_empty(), "{warnings:?}");
}

/// A destination the projection cannot open is an operational failure, and it
/// reaches the shell declaring status 1.
#[test]
fn a_destination_that_is_not_there_fails_with_status_one() {
    let dir = TempDir::new().expect("a temporary directory");
    let absent = utf8(&dir).join("absent");

    let error = status(
        absent.to_string(),
        None,
        false,
        &context(&exit::Verdict::default()),
    )
    .expect_err("an operational failure");
    let external = error
        .downcast::<standout::cli::ExternalFailure>()
        .expect("an external failure");

    assert_eq!(external.exit_status().code(), exit::FAILURE);
}

/// A destination that is not there fails whether or not the invocation named
/// a state directory, and the failure is the whole report: nothing warns about
/// a directory the classification never got as far as reading.
#[test]
fn a_destination_that_is_not_there_warns_about_no_state_directory() {
    let _ = standout::warnings::drain_warnings();
    let dir = TempDir::new().expect("a temporary directory");
    let absent = utf8(&dir).join("absent");

    let error = status(
        absent.to_string(),
        Some(absent.join("no-such-state").to_string()),
        true,
        &context(&exit::Verdict::default()),
    )
    .expect_err("an operational failure");
    let external = error
        .downcast::<standout::cli::ExternalFailure>()
        .expect("an external failure");

    assert_eq!(external.exit_status().code(), exit::FAILURE);
    assert!(
        standout::warnings::drain_warnings().is_empty(),
        "a run that classified nothing warned about the state directory"
    );
}

/// A state directory that is the destination is `StateDirIsTarget`, which the
/// library does not carry as a refusal, so the shell spends 1 on it and not 2.
#[test]
fn a_state_directory_that_is_the_destination_fails_with_status_one() {
    let dir = TempDir::new().expect("a temporary directory");
    let dest = utf8(&dir);

    let error = status(
        dest.to_string(),
        Some(dest.to_string()),
        false,
        &context(&exit::Verdict::default()),
    )
    .expect_err("a refused state directory");
    let external = error
        .downcast::<standout::cli::ExternalFailure>()
        .expect("an external failure");

    assert_eq!(external.exit_status().code(), exit::FAILURE);
}
