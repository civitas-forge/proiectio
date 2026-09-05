use super::*;

use camino::Utf8PathBuf;
use libproiectio::{ArchiveFormat, Origin, Refusal, Refused};
use standout::cli::AppFailure;

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

/// The seam Standout writes verbatim: an `AppFailure` pins both the status
/// and the bytes, and the framework adds no prefix of its own.
#[test]
fn a_library_failure_carries_its_status_and_message_to_the_shell() {
    for error in [refused(), Error::StateDirIsTarget { path: path() }] {
        let expected = of_error(&error);
        let message = error.to_string();
        let app = failure(error)
            .downcast::<AppFailure>()
            .expect("an app failure");
        assert_eq!(app.exit_status().code(), expected);
        assert_eq!(app.diagnostic(), format!("Error: {message}\n"));
    }
}

/// A Unix filename may hold an escape sequence; it leaves as the characters
/// it is, and the message ends exactly once, because Standout writes an
/// `AppFailure`'s diagnostic byte for byte.
#[test]
fn a_diagnostic_carrying_control_characters_is_escaped_before_it_is_handed_over() {
    const OSC: &str = "\u{1b}]52;c;cGF5bG9hZA==\u{7}";

    let app = failure(Error::DestinationTooDeep {
        path: Utf8PathBuf::from(format!("/srv/{OSC}")),
        limit: 64,
    })
    .downcast::<AppFailure>()
    .expect("an app failure");

    let text = app.diagnostic();
    assert!(text.contains(r"\u{1b}]52;c;cGF5bG9hZA==\u{7}"), "{text:?}");
    assert_eq!(text.matches('\n').count(), 1, "{text:?}");
    assert!(
        !text.trim_end_matches('\n').chars().any(char::is_control),
        "a control character reached the terminal: {text:?}"
    );
}

/// A message this CLI spelled over several lines keeps them, and still ends
/// exactly once.
#[test]
fn a_diagnostic_spelled_over_several_lines_keeps_them() {
    let escaped = crate::app::control_escaped_block(
        "error: unexpected argument '--nope' found\n\nUsage: proiectio status [OPTIONS]",
    );

    assert_eq!(
        escaped,
        "error: unexpected argument '--nope' found\n\nUsage: proiectio status [OPTIONS]"
    );
}
