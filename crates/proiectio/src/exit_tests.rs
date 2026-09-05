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

/// A refusal is the one status Standout would not choose for itself, so it
/// goes through the seam that pins both the status and the bytes, and the
/// framework adds no prefix of its own.
#[test]
fn a_refusal_pins_its_status_and_its_message() {
    let message = refused().to_string();

    let app = failure(refused())
        .downcast::<AppFailure>()
        .expect("an app failure");

    assert_eq!(app.exit_status().code(), REFUSAL);
    assert_eq!(app.diagnostic(), format!("Error: {message}\n"));
}

/// An operational failure pins nothing: Standout already spends 1 on a
/// handler error, frames the message itself, and keeps it off stderr under a
/// structured encoding.
#[test]
fn an_operational_failure_leaves_the_status_and_the_framing_to_standout() {
    let error = Error::StateDirIsTarget { path: path() };
    let message = error.to_string();

    let stated = failure(error);

    assert!(
        stated.downcast_ref::<AppFailure>().is_none(),
        "an operational failure pinned a status: {stated}"
    );
    assert_eq!(stated.to_string(), message);
    assert!(
        stated.downcast_ref::<Stated>().is_some(),
        "the escaped message carries the library error: {stated}"
    );
}

/// A Unix filename may hold an escape sequence; it leaves as the characters
/// it is, whichever of the two seams the error takes.
#[test]
fn a_diagnostic_carrying_control_characters_is_escaped_before_it_is_handed_over() {
    const OSC: &str = "\u{1b}]52;c;cGF5bG9hZA==\u{7}";

    let deep = || Error::DestinationTooDeep {
        path: Utf8PathBuf::from(format!("/srv/{OSC}")),
        limit: 64,
    };
    let refused_deeply = || {
        Error::Refused(Refused::one(
            Utf8PathBuf::from(format!("bin/{OSC}")),
            Refusal::Drift,
            Origin::Caller,
        ))
    };
    let cases = [
        ("an operational failure", failure(deep()).to_string()),
        (
            "a refusal",
            failure(refused_deeply())
                .downcast::<AppFailure>()
                .expect("an app failure")
                .diagnostic()
                .to_owned(),
        ),
    ];

    for (case, text) in cases {
        assert!(
            text.contains(r"\u{1b}]52;c;cGF5bG9hZA==\u{7}"),
            "{case}: {text:?}"
        );
        assert!(
            !text.trim_end_matches('\n').chars().any(char::is_control),
            "{case}: a control character reached the terminal: {text:?}"
        );
    }
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
