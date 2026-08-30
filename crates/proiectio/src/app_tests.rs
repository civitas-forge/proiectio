#![allow(
    non_snake_case,
    reason = "the #[handler] macro derives `<name>__handler` from the function it wraps"
)]

use super::*;

use camino::Utf8PathBuf;
use libproiectio::{Origin, Projection, Refusal, Refused, Status};
use serde_json::Value as JsonValue;
use serial_test::serial;
use standout::OutputMode;
use standout::cli::Output;
use standout::handler;
use tempfile::TempDir;

use crate::cli;
use crate::exit;
use crate::testing::{assert_styles_resolved, assert_tags_declared, classified, harness, utf8};

fn app() -> App {
    build().expect("an app")
}

/// The library's own report of a destination, for the document `--output json`
/// is asserted against.
fn library_status(dest: &Utf8PathBuf) -> Status {
    Projection::new(dest, None)
        .expect("a projection")
        .status()
        .expect("a status")
}

/// A destination classified as one drifted, one clean and one missing path,
/// under two directories the manifest does not record.
fn classified_dir() -> (TempDir, Utf8PathBuf) {
    let dir = TempDir::new().expect("a temporary directory");
    let dest = utf8(&dir);
    classified(&dest);
    (dir, dest)
}

/// One line per classified path, the state padded to the width of the widest,
/// as `docs/cli-tour.lex` section 3 states it. The directories the projected
/// files imply are on disk and absent from the manifest, which is the library's
/// `Foreign`, so they classify on the same terms as any other unrecorded path.
#[test]
#[serial]
fn status_prints_one_line_per_recorded_path() {
    let (dir, dest) = classified_dir();

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        ["proiectio", "status", "--dest", dest.as_str()],
    );

    result.assert_success();
    assert_eq!(
        result.stdout(),
        "foreign  bin\ndrifted  bin/tool\nforeign  config\n\
         clean    config/settings.toml\nmissing  current\n"
    );
    let stdout = result.stdout();
    assert!(
        stdout.ends_with('\n') && !stdout.ends_with("\n\n"),
        "{stdout:?}"
    );
}

/// A path on disk the manifest does not record is the fourth state, and it
/// renders on the same terms as the three the manifest names.
#[test]
#[serial]
fn a_path_the_manifest_does_not_record_reads_foreign() {
    let (dir, dest) = classified_dir();
    std::fs::write(dest.join("stray.txt"), b"not ours\n").expect("a stray file");

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        ["proiectio", "status", "--dest", dest.as_str()],
    );

    result.assert_success();
    result.assert_stdout_contains("foreign  stray.txt");
}

/// With no `--dest`, the destination is the working directory.
#[test]
#[serial]
fn status_defaults_the_destination_to_the_working_directory() {
    let (dir, _) = classified_dir();

    let result = harness(&dir).run(&app(), cli::command(), ["proiectio", "status"]);

    result.assert_success();
    result.assert_stdout_contains("drifted  bin/tool");
}

/// Structured output is the library's `Status` and nothing else: no view
/// model stands between a consumer and the document the library serializes.
#[test]
#[serial]
fn status_is_the_librarys_own_status_document() {
    let (dir, dest) = classified_dir();
    let expected = serde_json::to_value(library_status(&dest)).expect("a serialized status");

    let result = harness(&dir).output_mode(OutputMode::Json).run(
        &app(),
        cli::command(),
        ["proiectio", "status", "--dest", dest.as_str()],
    );

    result.assert_success();
    let value: JsonValue = serde_json::from_str(result.stdout()).expect("a JSON document");
    assert_eq!(value, expected);
    assert_eq!(value["rows"]["bin/tool"]["verdict"], "Drifted");
    assert_eq!(value["rows"]["config/settings.toml"]["verdict"], "Clean");
    assert_eq!(value["rows"]["current"]["verdict"], "Missing");
}

/// A destination with no manifest and nothing on disk classifies nothing, and
/// the run still succeeds.
#[test]
#[serial]
fn an_unprojected_destination_reports_nothing_and_succeeds() {
    let dir = TempDir::new().expect("a temporary directory");

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        ["proiectio", "status", "--dest", utf8(&dir).as_str()],
    );

    result.assert_success();
    assert_eq!(result.stdout(), "");
}

#[test]
#[serial]
fn status_names_only_styles_the_stylesheet_declares_and_the_theme_resolves() {
    let (dir, dest) = classified_dir();
    let argv = ["proiectio", "status", "--dest", dest.as_str()];

    let debug = harness(&dir)
        .output_mode(OutputMode::TermDebug)
        .run(&app(), cli::command(), argv);
    debug.assert_success();
    assert_tags_declared("status", debug.stdout());

    let term = harness(&dir)
        .output_mode(OutputMode::Term)
        .run(&app(), cli::command(), argv);
    term.assert_success();
    assert_styles_resolved("status", term.stdout());
}

#[test]
#[serial]
fn config_list_reports_the_compiled_default() {
    let dir = TempDir::new().expect("a temporary directory");

    let result = harness(&dir).output_mode(OutputMode::Json).run(
        &app(),
        cli::command(),
        ["proiectio", "conf", "list"],
    );

    result.assert_success();
    let value: JsonValue = serde_json::from_str(result.stdout()).expect("a JSON document");
    assert_eq!(value["kind"], "listing");
    let entries = value["entries"].as_array().expect("the entries");
    assert!(
        entries
            .iter()
            .any(|entry| entry["key"] == "owner" && entry["value"] == "default"),
        "{entries:?}"
    );
}

#[test]
#[serial]
fn version_reports_the_binary_crates_version() {
    let dir = TempDir::new().expect("a temporary directory");

    let result = harness(&dir).run(&app(), cli::command(), ["proiectio", "--version"]);

    result.assert_success();
    assert_eq!(
        result.stdout().trim(),
        format!("proiectio {}", env!("CARGO_PKG_VERSION"))
    );
}

/// A refusal the library reports, run through the whole shell.
///
/// `status` reads and classifies; it cannot refuse — `observe` fails with I/O
/// and depth errors alone, and `classify` returns a report rather than a
/// result. So the refusal path is proven over a handler that returns the one
/// error variant that carries a refusal, through the same seam every command
/// maps its library failures with.
#[handler]
fn refusing() -> Result<Output<Status>> {
    Err(exit::failure(libproiectio::Error::Refused(Refused::one(
        Utf8PathBuf::from("bin/tool"),
        Refusal::Drift,
        Origin::Caller,
    ))))
}

fn refusing_app() -> App {
    App::builder()
        .templates(templates())
        .styles(embed_styles!("src/styles"))
        .default_theme("proiectio")
        .template_engine(Box::new(engine()))
        .command_with("status", refusing__handler, |cfg| {
            cfg.template("status.jinja")
        })
        .expect("a command")
        .build()
        .expect("an app")
}

/// One row of the shell contract: what to call the case, the app the run goes
/// through, the command line, and the status the process exits with.
type Row = (&'static str, fn() -> App, &'static [&'static str], u8);

/// The whole 0/1/2 contract of `docs/cli-tour.lex` section 2 in one table,
/// under both a rendered and a machine-readable format.
#[test]
#[serial]
fn every_outcome_pins_the_status_the_process_exits_with() {
    let rows: [Row; 6] = [
        ("a classified destination", app, &["status"], exit::OK),
        ("the config listing", app, &["conf", "list"], exit::OK),
        (
            "a destination that is not there",
            app,
            &["status", "--dest", "absent"],
            exit::FAILURE,
        ),
        (
            "a state directory that is the destination",
            app,
            &["status", "--state-dir", "."],
            exit::FAILURE,
        ),
        (
            "a command line clap rejects",
            app,
            &["status", "--nope"],
            exit::FAILURE,
        ),
        ("a refusal", refusing_app, &["status"], exit::REFUSAL),
    ];

    for mode in [OutputMode::Text, OutputMode::Json] {
        for (case, built, args, expected) in rows {
            let (dir, _) = classified_dir();
            let mut argv = vec!["proiectio"];
            argv.extend_from_slice(args);

            let result = harness(&dir)
                .output_mode(mode)
                .run(&built(), cli::command(), argv);

            assert_eq!(
                exit::status(result.outcome()),
                expected,
                "`proiectio {}` under {mode:?}, in the {case} case",
                args.join(" ")
            );
        }
    }
}

/// A path is data, not markup. One whose components are spelled as style tags
/// reaches the terminal as the characters they are, and structured output
/// carries them unescaped.
#[test]
#[serial]
fn a_path_spelled_like_a_style_tag_renders_as_itself() {
    const DIRECTORY: &str = "[clean]";
    const FILE: &str = "[clean]/y";

    let (dir, dest) = classified_dir();
    std::fs::create_dir(dest.join(DIRECTORY)).expect("a stray directory");
    std::fs::write(dest.join(FILE), b"not ours\n").expect("a stray file");
    let argv = ["proiectio", "status", "--dest", dest.as_str()];

    let text = harness(&dir).run(&app(), cli::command(), argv);
    text.assert_success();
    text.assert_stdout_contains(&format!("foreign  {DIRECTORY}"));
    text.assert_stdout_contains(&format!("foreign  {FILE}"));

    let term = harness(&dir)
        .output_mode(OutputMode::Term)
        .run(&app(), cli::command(), argv);
    term.assert_success();
    term.assert_stdout_contains(FILE);

    let json = harness(&dir)
        .output_mode(OutputMode::Json)
        .run(&app(), cli::command(), argv);
    json.assert_success();
    let value: JsonValue = serde_json::from_str(json.stdout()).expect("a JSON document");
    assert_eq!(value["rows"][DIRECTORY]["verdict"], "Foreign");
    assert_eq!(value["rows"][FILE]["verdict"], "Foreign");
}

/// An unknown tag spelling reaches the terminal whole rather than as the `?`
/// marker Standout gives a style it cannot resolve.
#[test]
#[serial]
fn a_path_spelled_like_an_unknown_tag_renders_as_itself() {
    const DIRECTORY: &str = "[nope]";

    let (dir, dest) = classified_dir();
    std::fs::create_dir(dest.join(DIRECTORY)).expect("a stray directory");
    std::fs::write(dest.join(DIRECTORY).join("y"), b"not ours\n").expect("a stray file");

    let result = harness(&dir).output_mode(OutputMode::Term).run(
        &app(),
        cli::command(),
        ["proiectio", "status", "--dest", dest.as_str()],
    );

    result.assert_success();
    result.assert_stdout_contains(DIRECTORY);
    assert_styles_resolved("a path spelled like a tag", result.stdout());
}

/// A config value is data too, and `config set` reports the value it stored.
#[test]
#[serial]
fn a_config_value_spelled_like_a_style_tag_renders_as_itself() {
    const SPELLED: &str = "[ok]site[/ok]";

    let dir = TempDir::new().expect("a temporary directory");
    let argv = ["proiectio", "conf", "set", "owner", SPELLED];

    let text = harness(&dir).run(&app(), cli::command(), argv);
    text.assert_success();
    text.assert_stdout_contains(&format!("set owner = {SPELLED}"));

    let json = harness(&dir)
        .output_mode(OutputMode::Json)
        .run(&app(), cli::command(), argv);
    json.assert_success();
    let value: JsonValue = serde_json::from_str(json.stdout()).expect("a JSON document");
    assert_eq!(value["value"], SPELLED);
}

/// Every config line the CLI prints is clapfig's own, so a string value keeps
/// the spelling the active format gives it.
#[test]
#[serial]
fn config_lines_are_the_spelling_clapfig_rendered() {
    let dir = TempDir::new().expect("a temporary directory");

    let listing = harness(&dir).run(&app(), cli::command(), ["proiectio", "conf", "list"]);
    listing.assert_success();
    let rendered = rendered_field(&dir, ["proiectio", "conf", "list"]);
    assert_eq!(listing.stdout(), format!("{rendered}\n"));

    let get = harness(&dir).run(
        &app(),
        cli::command(),
        ["proiectio", "conf", "get", "owner"],
    );
    get.assert_success();
    let rendered = rendered_field(&dir, ["proiectio", "conf", "get", "owner"]);
    assert_eq!(get.stdout(), format!("{rendered}\n"));
}

/// The `rendered` field of a config view, read back through the same run under
/// `--output json`.
fn rendered_field<const N: usize>(dir: &TempDir, argv: [&str; N]) -> String {
    let result = harness(dir)
        .output_mode(OutputMode::Json)
        .run(&app(), cli::command(), argv);
    result.assert_success();
    let value: JsonValue = serde_json::from_str(result.stdout()).expect("a JSON document");
    value["rendered"]
        .as_str()
        .expect("a rendered spelling")
        .to_owned()
}

/// Unix argv is bytes, not text. An argument that is not UTF-8 reaches clap,
/// which rejects it with the diagnostic and the 1 the tour spends on usage.
#[cfg(unix)]
#[test]
#[serial]
fn an_argument_that_is_not_utf8_is_rejected_rather_than_panicking() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let (dir, _) = classified_dir();
    let argv = vec![
        OsString::from("proiectio"),
        OsString::from("status"),
        OsString::from("--dest"),
        OsString::from_vec(vec![0x2f, 0xff]),
    ];

    let result = harness(&dir).run(&app(), cli::command(), argv);

    assert_eq!(exit::status(result.outcome()), exit::FAILURE);
}

#[test]
fn a_bracket_leaves_the_markup_pass_as_the_bracket_it_was() {
    assert_eq!(verbatim("[clean]x[/clean]"), r"\[clean\]x\[/clean\]");
    assert_eq!(verbatim("plain/path"), "plain/path");
    assert_eq!(verbatim(r"C:\dir"), r"C:\dir");
}
