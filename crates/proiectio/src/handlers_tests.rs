use super::*;

use std::rc::Rc;

use libproiectio::PathState;
use standout::WarningBuffer;
use standout::cli::AppFailure;
use standout::dispatch::Extensions;
use tempfile::TempDir;

use crate::testing::{OWNER, classified, projected, utf8};

/// A context carrying what a run reads and writes beside its arguments: the
/// `--force` a rendered run reads its hint off, and the buffer `ctx.warn`
/// pushes to, which the framework hands a real run and a test reads back.
fn context() -> CommandContext {
    let (ctx, _) = warned_context();
    ctx
}

/// An operational failure pins no status of its own, so Standout's 1 for a
/// handler error is what the process leaves with.
#[track_caller]
fn unpinned(error: &anyhow::Error) {
    assert!(
        error.downcast_ref::<AppFailure>().is_none(),
        "an operational failure pinned a status: {error}"
    );
}

fn warned_context() -> (CommandContext, WarningBuffer) {
    let mut state = Extensions::new();
    state.insert(Forced::default());
    let mut ctx = CommandContext::new(vec!["status".to_owned()], Rc::new(state));
    let warnings = WarningBuffer::new();
    ctx.extensions.insert(warnings.clone());
    (ctx, warnings)
}

/// The status one classification leaves the process with, and the warnings it
/// wrote about the run. The status rides the handler's own output now, so the
/// typed call answers for it without a cell beside it.
fn checked(dest: &Utf8Path, state_dir: Option<&Utf8Path>, check: bool) -> (u8, Vec<String>) {
    let (ctx, warnings) = warned_context();
    let stated = status(
        dest.to_string(),
        state_dir.map(Utf8Path::to_string),
        check,
        &ctx,
    )
    .expect("a status");
    (stated.exit_status().code(), warnings.take())
}

/// The adapter, called as the typed function the `#[handler]` macro
/// preserves; what comes back is the library's own report, unmapped.
#[test]
fn the_two_options_become_the_projection_the_library_classifies() {
    let dir = TempDir::new().expect("a temporary directory");
    let dest = utf8(&dir);
    classified(&dest);

    let Output::Render(report) =
        status(dest.to_string(), None, false, &context()).expect("a status")
    else {
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
        &context(),
    )
    .expect("a status") else {
        panic!("expected rendered data");
    };

    assert_eq!(report.rows.len(), 3);
}

/// One row of the `--check` contract: what to call the destination, what to
/// do to it after it is projected, and the status `--check` leaves with.
type Case = (&'static str, Option<fn(&Utf8Path)>, u8);

/// Everything clean passes; each of the three ways a path can differ spends
/// the refusal status. Plain `status` leaves 0 on all four.
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

/// The typo a gate has to fail on: the missing directory itself is what
/// `--check` spends the refusal on.
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

/// The default state directory's absence is the ordinary state of a fresh
/// destination: no warning, and `--check` passes.
#[test]
fn the_default_state_directory_being_absent_is_neither_a_warning_nor_a_refusal() {
    let dir = TempDir::new().expect("a temporary directory");
    let dest = utf8(&dir);

    let (checking, warnings) = checked(&dest, None, true);

    assert_eq!(checking, exit::OK);
    assert!(warnings.is_empty(), "{warnings:?}");
}

/// One removal of everything the owner holds, through the typed call, and the
/// warnings it wrote about the run. Naming the owner keeps the run off the
/// machine's configuration.
fn removal(dest: &Utf8Path, state_dir: Option<&Utf8Path>, dry_run: bool) -> Vec<String> {
    let (ctx, warnings) = warned_context();
    rm(
        dest.to_string(),
        state_dir.map(Utf8Path::to_string),
        Vec::new(),
        Some(OWNER.to_owned()),
        dry_run,
        false,
        &ctx,
    )
    .expect("a removal");
    warnings.take()
}

/// A `--state-dir` the filesystem does not have reads as the empty manifest;
/// the removal unlinks nothing and says on stderr why, in both tenses.
#[test]
fn rm_warns_about_a_named_state_directory_that_is_not_there() {
    for dry_run in [false, true] {
        let dir = TempDir::new().expect("a temporary directory");
        let dest = utf8(&dir);
        projected(&dest);
        let absent = dest.join("no-such-state");

        let warnings = removal(&dest, Some(&absent), dry_run);

        assert_eq!(
            warnings,
            vec![format!(
                "state dir {absent} does not exist; treating manifest as empty"
            )],
            "a {} removal",
            if dry_run { "dry" } else { "real" }
        );
        assert!(dest.join("bin/tool").exists(), "dry_run: {dry_run}");
    }
}

#[test]
fn rm_over_the_default_state_directory_warns_about_nothing() {
    let dir = TempDir::new().expect("a temporary directory");
    let dest = utf8(&dir);

    let warnings = removal(&dest, None, false);

    assert!(warnings.is_empty(), "{warnings:?}");
}

/// A destination the removal cannot open is an operational failure and the
/// whole report.
#[test]
fn a_removal_that_cannot_open_the_destination_does_not_warn() {
    let dir = TempDir::new().expect("a temporary directory");
    let absent = utf8(&dir).join("absent");
    let (ctx, warnings) = warned_context();

    let Err(error) = rm(
        absent.to_string(),
        Some(absent.join("no-such-state").to_string()),
        Vec::new(),
        Some(OWNER.to_owned()),
        false,
        false,
        &ctx,
    ) else {
        panic!("a removal over a destination that is not there reported a run");
    };
    unpinned(&error);
    assert!(
        warnings.take().is_empty(),
        "a removal that opened nothing warned about the state directory"
    );
}

/// A destination the projection cannot open is an operational failure, and it
/// reaches the shell declaring status 1.
#[test]
fn a_destination_that_is_not_there_fails_with_status_one() {
    let dir = TempDir::new().expect("a temporary directory");
    let absent = utf8(&dir).join("absent");

    let error =
        status(absent.to_string(), None, false, &context()).expect_err("an operational failure");
    unpinned(&error);
}

#[test]
fn a_destination_that_is_not_there_warns_about_no_state_directory() {
    let dir = TempDir::new().expect("a temporary directory");
    let absent = utf8(&dir).join("absent");
    let (ctx, warnings) = warned_context();

    let error = status(
        absent.to_string(),
        Some(absent.join("no-such-state").to_string()),
        true,
        &ctx,
    )
    .expect_err("an operational failure");
    unpinned(&error);
    assert!(
        warnings.take().is_empty(),
        "a run that classified nothing warned about the state directory"
    );
}

/// A state directory that is the destination is `StateDirIsTarget`, which the
/// library does not carry as a refusal, so the shell spends 1 on it and not 2.
#[test]
fn a_state_directory_that_is_the_destination_fails_with_status_one() {
    let dir = TempDir::new().expect("a temporary directory");
    let dest = utf8(&dir);

    let error = status(dest.to_string(), Some(dest.to_string()), false, &context())
        .expect_err("a refused state directory");
    unpinned(&error);
}
