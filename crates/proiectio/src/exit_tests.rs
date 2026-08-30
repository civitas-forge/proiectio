use super::*;

use camino::Utf8PathBuf;
use libproiectio::{ArchiveFormat, Origin, Refusal, Refused};
use standout::cli::{RunError, RunOutput};

fn refused() -> Error {
    Error::Refused(Refused::one(
        Utf8PathBuf::from("bin/tool"),
        Refusal::Drift,
        Origin::Caller,
    ))
}

fn path() -> Utf8PathBuf {
    Utf8PathBuf::from("/srv/www")
}

/// One error of every class the library reports that is not a refusal.
fn operational_failures() -> Vec<Error> {
    vec![
        Error::Io {
            path: path(),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        },
        Error::ManifestFormat {
            path: path(),
            source: serde_json::from_str::<u32>("nope").expect_err("a parse error"),
        },
        Error::ManifestVersion {
            path: path(),
            found: 2,
            supported: 1,
        },
        Error::LockHeld { path: path() },
        Error::CurrentDirectory {
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        },
        Error::PathNotUtf8 {
            path: "/srv/\u{fffd}".to_owned(),
        },
        Error::StateDirIsTarget { path: path() },
        Error::MappingFormat {
            path: path(),
            source: toml::from_str::<u32>("=").expect_err("a parse error"),
        },
        Error::MappingVersion {
            path: path(),
            found: 2,
            supported: 1,
        },
        Error::MappingContentsXorSource {
            path: path(),
            key: Utf8PathBuf::from("bin/tool"),
        },
        Error::MappingDuplicate {
            path: path(),
            key: Utf8PathBuf::from("bin/tool"),
        },
        Error::ArchiveFormatUnknown { path: path() },
        Error::ArchiveDecode {
            path: path(),
            format: ArchiveFormat::Zip,
            source: std::io::Error::from(std::io::ErrorKind::InvalidData),
        },
        Error::ArchiveMemberNameNotUtf8 {
            path: path(),
            name: "\u{fffd}".to_owned(),
        },
        Error::ArchiveMemberTargetNotUtf8 {
            path: path(),
            member: Utf8PathBuf::from("link"),
            target: "\u{fffd}".to_owned(),
        },
        Error::ArchiveMemberKind {
            path: path(),
            member: Utf8PathBuf::from("dev/null"),
        },
        Error::ArchiveMemberKindDisagrees {
            path: path(),
            member: Utf8PathBuf::from("dir/"),
        },
        Error::ArchiveMemberDuplicate {
            path: path(),
            member: Utf8PathBuf::from("bin/tool"),
        },
        Error::ArchiveMemberStripped {
            path: path(),
            member: Utf8PathBuf::from("top"),
            strip: 1,
        },
        Error::ArchiveMemberTooDeep {
            path: path(),
            member: Utf8PathBuf::from("a/b"),
            limit: 64,
        },
        Error::ArchiveTooLarge {
            path: path(),
            limit: 1,
        },
        Error::ArchiveTooManyMembers {
            path: path(),
            limit: 1,
        },
        Error::TreeNameNotUtf8 {
            path: path(),
            name: "\u{fffd}".to_owned(),
        },
        Error::TreeTargetNotUtf8 {
            path: path(),
            target: "\u{fffd}".to_owned(),
        },
        Error::TreeTooDeep {
            path: path(),
            limit: 64,
        },
        Error::DestinationTooDeep {
            path: path(),
            limit: 64,
        },
        Error::TreeNodeKind { path: path() },
        Error::FilesNodeKind { path: path() },
        Error::FilesDuplicate {
            first: path(),
            second: path(),
        },
        Error::StripOnDirectory { path: path() },
    ]
}

#[test]
fn a_refusal_is_the_only_library_error_that_exits_two() {
    assert_eq!(of_error(&refused()), REFUSAL);
    for error in operational_failures() {
        assert_eq!(of_error(&error), FAILURE, "{error}");
    }
}

#[test]
fn a_library_failure_carries_its_status_and_message_to_the_shell() {
    for error in [refused(), Error::StateDirIsTarget { path: path() }] {
        let expected = of_error(&error);
        let message = error.to_string();
        let external = failure(error)
            .downcast::<ExternalFailure>()
            .expect("an external failure");
        assert_eq!(external.exit_status().code(), expected);
        assert_eq!(external.diagnostic(), format!("Error: {message}\n"));
    }
}

#[test]
fn a_completed_run_exits_zero() {
    assert_eq!(status(&RunResult::Handled(RunOutput::command("ok"))), OK);
    assert_eq!(status(&RunResult::Silent), OK);
}

/// Standout spends 2 on a command line clap rejects; this CLI spends 2 on
/// refusals alone, so a usage error joins the operational failures.
#[test]
fn a_command_line_clap_rejects_exits_one() {
    let usage = RunResult::Error(RunError::new("bad flag", RunErrorKind::ClapUsage));
    assert_eq!(status(&usage), FAILURE);

    let handler = RunResult::Error(RunError::new("boom", RunErrorKind::Handler));
    assert_eq!(status(&handler), FAILURE);
}

#[test]
fn a_declared_refusal_reaches_the_shell_as_two() {
    let declared = ExternalFailure::new(REFUSAL, "Error: refused\n").expect("an external failure");
    let result = RunResult::Error(RunError::from(declared));
    assert_eq!(status(&result), REFUSAL);
}

/// A stream whose every write fails with one kind, standing in for a reader
/// that closed the pipe and for a disk that has no room left.
struct Failing(ErrorKind);

impl Write for Failing {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let _ = buffer;
        Err(std::io::Error::from(self.0))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::from(self.0))
    }
}

fn handled(text: &str) -> RunResult {
    RunResult::Handled(RunOutput::command(text))
}

#[test]
fn handled_text_reaches_stdout_as_the_template_spelled_it() {
    let mut out = Vec::new();
    let mut err = Vec::new();

    let status = emit_to(
        &mut out,
        &mut err,
        &handled("clean    config/settings.toml\n"),
    );

    assert_eq!(status, OK);
    assert_eq!(
        String::from_utf8(out).expect("text"),
        "clean    config/settings.toml\n"
    );
    assert!(err.is_empty());
}

#[test]
fn an_empty_handled_run_writes_nothing() {
    let mut out = Vec::new();
    let mut err = Vec::new();

    assert_eq!(emit_to(&mut out, &mut err, &handled("")), OK);
    assert!(out.is_empty());
}

/// `proiectio status | head` closes stdout early, which is the reader's
/// choice and not this process's failure.
#[test]
fn a_reader_that_closed_stdout_leaves_the_status_alone() {
    let mut err = Vec::new();

    assert_eq!(
        emit_to(
            &mut Failing(ErrorKind::BrokenPipe),
            &mut err,
            &handled("rows\n")
        ),
        OK
    );
}

#[test]
fn a_stdout_write_that_fails_otherwise_exits_one() {
    let mut err = Vec::new();

    assert_eq!(
        emit_to(
            &mut Failing(ErrorKind::StorageFull),
            &mut err,
            &handled("rows\n")
        ),
        FAILURE
    );
}

/// A declared refusal keeps its 2 even when the diagnostic cannot be written.
#[test]
fn a_write_failure_never_lowers_a_refusal() {
    let declared = ExternalFailure::new(REFUSAL, "Error: refused\n").expect("an external failure");
    let result = RunResult::Error(RunError::from(declared));
    let mut out = Vec::new();

    assert_eq!(
        emit_to(&mut out, &mut Failing(ErrorKind::StorageFull), &result),
        REFUSAL
    );
}

/// An external failure carries the diagnostic the handler spelled, newline
/// and all; every other error is one line the shell terminates itself.
#[test]
fn diagnostics_reach_stderr_with_exactly_one_newline() {
    let declared =
        ExternalFailure::new(FAILURE, "Error: no such destination\n").expect("an external failure");
    let mut out = Vec::new();
    let mut err = Vec::new();

    let status = emit_to(
        &mut out,
        &mut err,
        &RunResult::Error(RunError::from(declared)),
    );

    assert_eq!(status, FAILURE);
    assert_eq!(
        String::from_utf8(err).expect("text"),
        "Error: no such destination\n"
    );
    assert!(out.is_empty());

    let usage = RunResult::Error(RunError::new("bad flag", RunErrorKind::ClapUsage));
    let mut err = Vec::new();
    assert_eq!(emit_to(&mut Vec::new(), &mut err, &usage), FAILURE);
    let text = String::from_utf8(err).expect("text");
    assert!(text.ends_with('\n') && !text.ends_with("\n\n"), "{text:?}");
}
