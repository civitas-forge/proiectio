use super::*;

use camino::Utf8PathBuf;
use libproiectio::{ArchiveFormat, Origin, Refusal, Refused};
use standout::cli::{ArtifactDestination, ArtifactReceipt, ArtifactRun, RunError, RunOutput};

/// A run that queued no framework warning.
const NO_WARNINGS: &[String] = &[];

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
            role: libproiectio::IoRole::Unstated,
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
        Error::ArchiveFullyStripped {
            path: path(),
            strip: 3,
            dropped: 4,
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
        assert_eq!(external.diagnostic(), format!("Error: {message}"));
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
    let declared = ExternalFailure::new(REFUSAL, "Error: refused").expect("an external failure");
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
        NO_WARNINGS,
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

    assert_eq!(emit_to(&mut out, &mut err, &handled(""), NO_WARNINGS), OK);
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
            &handled("rows\n"),
            NO_WARNINGS
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
            &handled("rows\n"),
            NO_WARNINGS
        ),
        FAILURE
    );
}

/// A declared refusal keeps its 2 even when the diagnostic cannot be written.
#[test]
fn a_write_failure_never_lowers_a_refusal() {
    let declared = ExternalFailure::new(REFUSAL, "Error: refused").expect("an external failure");
    let result = RunResult::Error(RunError::from(declared));
    let mut out = Vec::new();

    assert_eq!(
        emit_to(
            &mut out,
            &mut Failing(ErrorKind::StorageFull),
            &result,
            NO_WARNINGS
        ),
        REFUSAL
    );
}

/// Every diagnostic is one line, and the line terminator is this shell's own
/// rather than anything the message carried.
#[test]
fn diagnostics_reach_stderr_with_exactly_one_newline() {
    let declared =
        ExternalFailure::new(FAILURE, "Error: no such destination").expect("an external failure");
    let mut out = Vec::new();
    let mut err = Vec::new();

    let status = emit_to(
        &mut out,
        &mut err,
        &RunResult::Error(RunError::from(declared)),
        NO_WARNINGS,
    );

    assert_eq!(status, FAILURE);
    assert_eq!(
        String::from_utf8(err).expect("text"),
        "Error: no such destination\n"
    );
    assert!(out.is_empty());

    let usage = RunResult::Error(RunError::new("bad flag", RunErrorKind::ClapUsage));
    let mut err = Vec::new();
    assert_eq!(
        emit_to(&mut Vec::new(), &mut err, &usage, NO_WARNINGS),
        FAILURE
    );
    let text = String::from_utf8(err).expect("text");
    assert!(text.ends_with('\n') && !text.ends_with("\n\n"), "{text:?}");
}

/// A handler that returns bytes has them written, not dropped.
#[test]
fn binary_output_reaches_stdout_byte_for_byte() {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let result = RunResult::Binary(vec![0x00, 0xff, 0x0a], "export.bin".to_owned());

    assert_eq!(emit_to(&mut out, &mut err, &result, NO_WARNINGS), OK);
    assert_eq!(out, vec![0x00, 0xff, 0x0a]);
    assert!(err.is_empty());
}

#[test]
fn a_binary_write_that_fails_exits_one() {
    let result = RunResult::Binary(vec![0x01], "export.bin".to_owned());

    assert_eq!(
        emit_to(
            &mut Failing(ErrorKind::StorageFull),
            &mut Vec::new(),
            &result,
            NO_WARNINGS
        ),
        FAILURE
    );
}

fn artifact(destination: ArtifactDestination, report: Option<&str>) -> RunResult {
    let bytes = b"id,state\n1,clean\n".to_vec();
    let receipt = ArtifactReceipt::new(destination, bytes.len());
    RunResult::Artifact(ArtifactRun::new(
        bytes,
        None,
        receipt,
        report.map(str::to_owned),
    ))
}

/// An artifact bound for stdout is this process's write; its report goes to
/// stderr so it cannot corrupt the bytes.
#[test]
fn an_artifact_bound_for_stdout_writes_its_bytes_and_reports_on_stderr() {
    let mut out = Vec::new();
    let mut err = Vec::new();

    let status = emit_to(
        &mut out,
        &mut err,
        &artifact(ArtifactDestination::Stdout, Some("wrote 1 row")),
        NO_WARNINGS,
    );

    assert_eq!(status, OK);
    assert_eq!(String::from_utf8(out).expect("text"), "id,state\n1,clean\n");
    assert_eq!(String::from_utf8(err).expect("text"), "wrote 1 row\n");
}

/// The framework already put a file artifact's bytes on disk, so only the
/// report is left, and stdout is free to carry it.
#[test]
fn an_artifact_written_to_a_file_reports_on_stdout_and_writes_no_bytes() {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let destination = ArtifactDestination::File(std::path::PathBuf::from("/tmp/export.csv"));

    let status = emit_to(
        &mut out,
        &mut err,
        &artifact(destination, Some("wrote /tmp/export.csv")),
        NO_WARNINGS,
    );

    assert_eq!(status, OK);
    assert_eq!(
        String::from_utf8(out).expect("text"),
        "wrote /tmp/export.csv\n"
    );
    assert!(err.is_empty());
}

#[test]
fn an_artifact_carrying_no_report_writes_only_its_bytes() {
    let mut out = Vec::new();
    let mut err = Vec::new();

    let status = emit_to(
        &mut out,
        &mut err,
        &artifact(ArtifactDestination::Stdout, None),
        NO_WARNINGS,
    );

    assert_eq!(status, OK);
    assert_eq!(String::from_utf8(out).expect("text"), "id,state\n1,clean\n");
    assert!(err.is_empty());
}

#[test]
fn a_silent_run_writes_nothing_and_exits_zero() {
    let mut out = Vec::new();
    let mut err = Vec::new();

    assert_eq!(
        emit_to(&mut out, &mut err, &RunResult::Silent, NO_WARNINGS),
        OK
    );
    assert!(out.is_empty() && err.is_empty());
}

/// A command line no handler claims exits 1 and says so, rather than leaving
/// the shell a bare status.
#[test]
fn a_command_line_no_handler_matched_says_so_on_stderr() {
    let matches = clap::Command::new("proiectio")
        .try_get_matches_from(["proiectio"])
        .expect("a parsed command line");
    let mut out = Vec::new();
    let mut err = Vec::new();

    let status = emit_to(
        &mut out,
        &mut err,
        &RunResult::NoMatch(matches),
        NO_WARNINGS,
    );

    assert_eq!(status, FAILURE);
    assert_eq!(String::from_utf8(err).expect("text"), NO_COMMAND);
    assert!(out.is_empty());
}

/// `run_to_string` captures the framework's warnings rather than printing
/// them, so the shell drains them itself, after the run's own output.
#[test]
fn captured_warnings_follow_the_output_on_stderr() {
    standout::warnings::push_warning("a template named no style");
    standout::warnings::capture_warnings_for_run();
    let warnings = standout::warnings::take_captured_warnings();
    assert_eq!(warnings, vec!["a template named no style".to_owned()]);

    let mut out = Vec::new();
    let mut err = Vec::new();
    let status = emit_to(&mut out, &mut err, &handled("clean  a\n"), &warnings);

    assert_eq!(status, OK);
    assert_eq!(String::from_utf8(out).expect("text"), "clean  a\n");
    assert_eq!(
        String::from_utf8(err).expect("text"),
        "Warning: a template named no style\n"
    );
}

/// A warning the shell cannot write is still a failed write.
#[test]
fn a_warning_that_cannot_be_written_exits_one() {
    let warnings = vec!["a template named no style".to_owned()];

    assert_eq!(
        emit_to(
            &mut Vec::new(),
            &mut Failing(ErrorKind::StorageFull),
            &handled("clean  a\n"),
            &warnings
        ),
        FAILURE
    );
}

/// A Unix filename may hold an escape sequence; it leaves as the characters
/// it is, and no message can end a line the shell did not.
#[test]
fn a_diagnostic_carrying_control_characters_reaches_stderr_as_escapes() {
    const OSC: &str = "\u{1b}]52;c;cGF5bG9hZA==\u{7}";

    let library = failure(Error::DestinationTooDeep {
        path: Utf8PathBuf::from(format!("/srv/{OSC}")),
        limit: 64,
    })
    .downcast::<ExternalFailure>()
    .expect("an external failure");
    let cases = [
        RunResult::Error(RunError::from(library)),
        RunResult::Error(RunError::new(
            format!("read /srv/{OSC}"),
            RunErrorKind::Handler,
        )),
    ];

    for result in cases {
        let mut err = Vec::new();

        assert_eq!(
            emit_to(&mut Vec::new(), &mut err, &result, NO_WARNINGS),
            FAILURE
        );

        let text = String::from_utf8(err).expect("text");
        assert!(text.contains(r"\u{1b}]52;c;cGF5bG9hZA==\u{7}"), "{text:?}");
        assert_eq!(text.matches('\n').count(), 1, "{text:?}");
        assert!(
            !text.trim_end_matches('\n').chars().any(char::is_control),
            "a control character reached the terminal: {text:?}"
        );
    }
}

/// A warning the framework queued goes to the same stream on the same terms.
#[test]
fn a_warning_carrying_control_characters_reaches_stderr_as_escapes() {
    let warnings = vec!["a template named \u{1b}[31m".to_owned()];
    let mut err = Vec::new();

    assert_eq!(
        emit_to(&mut Vec::new(), &mut err, &handled(""), &warnings),
        OK
    );

    assert_eq!(
        String::from_utf8(err).expect("text"),
        "Warning: a template named \\u{1b}[31m\n"
    );
}

/// Clap spells a usage error over several lines; they survive, and the
/// shell still ends the message exactly once.
#[test]
fn a_diagnostic_clap_spelled_over_several_lines_keeps_them() {
    let usage = RunResult::Error(RunError::new(
        "error: unexpected argument '--nope' found\n\nUsage: proiectio status [OPTIONS]\n",
        RunErrorKind::ClapUsage,
    ));
    let mut err = Vec::new();

    assert_eq!(
        emit_to(&mut Vec::new(), &mut err, &usage, NO_WARNINGS),
        FAILURE
    );

    assert_eq!(
        String::from_utf8(err).expect("text"),
        "error: unexpected argument '--nope' found\n\nUsage: proiectio status [OPTIONS]\n"
    );
}

/// A run that recorded nothing leaves with whatever emitting it reported.
#[test]
fn an_unrecorded_verdict_leaves_the_emitted_status_alone() {
    let verdict = Verdict::default();

    for emitted in [OK, FAILURE, REFUSAL] {
        assert_eq!(verdict.over(emitted), emitted);
    }
}

/// A refused dry run rendered its plan, so the recorded refusal is what the
/// process leaves with; a failed write raises the run without lowering it.
#[test]
fn a_recorded_refusal_survives_whatever_emitting_the_run_reported() {
    let verdict = Verdict::default();
    verdict.record(REFUSAL);

    assert_eq!(verdict.over(OK), REFUSAL);
    assert_eq!(verdict.over(FAILURE), REFUSAL);
    assert_eq!(verdict.over(REFUSAL), REFUSAL);
}

/// The cell keeps the gravest status recorded in it, whichever order the
/// records arrived in.
#[test]
fn a_verdict_keeps_the_gravest_status_recorded() {
    let verdict = Verdict::default();

    verdict.record(REFUSAL);
    verdict.record(OK);

    assert_eq!(verdict.over(OK), REFUSAL);
}

/// The app holds a clone of the cell the composition root reads back, so what
/// a handler records reaches `main`.
#[test]
fn a_clone_records_into_the_same_cell() {
    let verdict = Verdict::default();
    let held = verdict.clone();

    held.record(REFUSAL);

    assert_eq!(verdict.over(OK), REFUSAL);
}
