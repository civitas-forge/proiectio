#![allow(
    non_snake_case,
    reason = "the #[handler] macro derives `<name>__handler` from the function it wraps"
)]

use super::*;

use camino::{Utf8Path, Utf8PathBuf};
use libproiectio::{Manifest, Origin, Projection, Refusal, Refused, Status};
use serde_json::Value as JsonValue;
use serial_test::serial;
use standout::OutputMode;
use standout::cli::Output;
use standout::handler;
use tempfile::TempDir;

use crate::cli;
use crate::exit;
use crate::testing::{
    appledouble_tarball, assert_styles_resolved, assert_tags_declared, classified, dot_tarball,
    flat_tarball, harness, manifest_of, modified, row, skeleton, stated, tarball, tour, utf8,
};

fn app() -> App {
    over(&exit::Verdict::default())
}

/// The app over a verdict the test keeps: a refused dry run renders its plan
/// and leaves the refusal there rather than in the run's result.
fn over(verdict: &exit::Verdict) -> App {
    build(verdict.clone()).expect("an app")
}

/// The status the process leaves with: what emitting the run reports, over
/// what the run recorded.
fn leaving(result: &standout_test::TestResult, verdict: &exit::Verdict) -> u8 {
    verdict.over(exit::status(result.outcome()))
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
/// as `docs/cli-tour.lex` section 3 states it.
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
        "drifted  bin/tool\nclean    config/settings.toml\nmissing  current\n"
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
    let rows = &value["rows"];
    assert_eq!(stated(rows, "bin/tool")["verdict"], "Drifted");
    assert_eq!(stated(rows, "config/settings.toml")["verdict"], "Clean");
    assert_eq!(stated(rows, "current")["verdict"], "Missing");
}

/// A destination holding the two paths one XML element name cannot tell
/// apart: `a/b` and `a_b`. The name XML would give the first is the second's
/// own name, so a document spelling paths as names reports one row for the
/// two; a row stating its own path reports both.
fn colliding_dir() -> (TempDir, Utf8PathBuf) {
    let dir = TempDir::new().expect("a temporary directory");
    let dest = utf8(&dir);
    std::fs::create_dir(dest.join("a")).expect("a directory");
    std::fs::write(dest.join("a").join("b"), b"nested\n").expect("a nested file");
    std::fs::write(dest.join("a_b"), b"flat\n").expect("a flat file");
    (dir, dest)
}

/// XML carries every path as a value, so every row a destination classifies
/// reaches a consumer whatever its paths are named.
#[test]
#[serial]
fn xml_carries_paths_as_values_and_keeps_every_row() {
    let (dir, dest) = colliding_dir();

    let result = harness(&dir).output_mode(OutputMode::Xml).run(
        &app(),
        cli::command(),
        ["proiectio", "status", "--dest", dest.as_str()],
    );

    result.assert_success();
    assert_eq!(
        result.stdout().trim(),
        "<data>\
         <rows><path>a/b</path><verdict>Foreign</verdict><facts/></rows>\
         <rows><path>a_b</path><verdict>Foreign</verdict><facts/></rows>\
         </data>"
    );
}

/// CSV writes one record per classified path under a header that names the
/// columns rather than the destination's paths, so the same reader reads
/// every destination.
#[test]
#[serial]
fn csv_writes_one_row_per_path_under_the_same_header() {
    let (colliding, colliding_dest) = colliding_dir();
    let (classified, classified_dest) = classified_dir();

    let foreign = harness(&colliding).output_mode(OutputMode::Csv).run(
        &app(),
        cli::command(),
        ["proiectio", "status", "--dest", colliding_dest.as_str()],
    );
    let recorded = harness(&classified).output_mode(OutputMode::Csv).run(
        &app(),
        cli::command(),
        ["proiectio", "status", "--dest", classified_dest.as_str()],
    );

    foreign.assert_success();
    recorded.assert_success();
    assert_eq!(
        foreign.stdout(),
        "path,verdict,shape,executable,owners\n\
         a/b,Foreign,,,\n\
         a_b,Foreign,,,\n"
    );
    assert_eq!(
        recorded.stdout(),
        "path,verdict,shape,executable,owners\n\
         bin/tool,Drifted,file,false,\"[\"\"default\"\"]\"\n\
         config/settings.toml,Clean,file,false,\"[\"\"default\"\"]\"\n\
         current,Missing,file,false,\"[\"\"default\"\"]\"\n"
    );
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
    assert!(row(&value["rows"], DIRECTORY).is_none());
    assert_eq!(stated(&value["rows"], FILE)["verdict"], "Foreign");
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
    text.assert_stdout_contains(&format!("set owner = \"{SPELLED}\""));

    let json = harness(&dir)
        .output_mode(OutputMode::Json)
        .run(&app(), cli::command(), argv);
    json.assert_success();
    let value: JsonValue = serde_json::from_str(json.stdout()).expect("a JSON document");
    assert_eq!(value["value"], SPELLED);
}

/// The term output and the `rendered` field are one spelling, so what a reader
/// copies off the terminal is what a structured consumer reads.
#[test]
#[serial]
fn config_lines_are_the_spelling_the_view_rendered() {
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

/// What `list` and `get` print goes back into the file it came from: a value
/// needing quotes carries them.
#[test]
#[serial]
fn a_config_line_parses_as_the_config_file_it_looks_like() {
    const SPELLED: &str = "me and you";

    let dir = TempDir::new().expect("a temporary directory");
    harness(&dir)
        .run(
            &app(),
            cli::command(),
            ["proiectio", "conf", "set", "owner", SPELLED],
        )
        .assert_success();

    for argv in [
        vec!["proiectio", "conf", "list"],
        vec!["proiectio", "conf", "get", "owner"],
    ] {
        let rendered = rendered_field(&dir, argv.clone());
        let parsed: toml::Table = rendered
            .parse()
            .unwrap_or_else(|error| panic!("{argv:?} printed {rendered:?}: {error}"));
        assert_eq!(parsed["owner"].as_str(), Some(SPELLED));
    }
}

/// A set and an unset change a file every invocation on the machine reads, so
/// each names the file it wrote.
#[test]
#[serial]
fn a_persisted_value_names_the_file_it_was_written_to() {
    let dir = TempDir::new().expect("a temporary directory");
    let set = ["proiectio", "conf", "set", "owner", "site"];

    let json = harness(&dir)
        .output_mode(OutputMode::Json)
        .run(&app(), cli::command(), set);
    json.assert_success();
    let value: JsonValue = serde_json::from_str(json.stdout()).expect("a JSON document");
    let path = Utf8PathBuf::from(value["path"].as_str().expect("the file the set wrote"));
    assert!(path.starts_with(utf8(&dir)), "{path}");
    assert!(
        std::fs::read_to_string(&path)
            .expect("the file the set named")
            .contains("site")
    );

    for argv in [set.to_vec(), vec!["proiectio", "conf", "unset", "owner"]] {
        let text = harness(&dir).run(&app(), cli::command(), argv.clone());
        text.assert_success();
        text.assert_stdout_contains(&format!("wrote {path}"));
    }
}

/// Clapfig treats an unset with no file to read as a successful no-op. A CLI
/// that claimed `wrote` there would name a file that does not exist, so the
/// run says which file it found nothing at.
#[test]
#[serial]
fn an_unset_with_no_file_to_edit_names_no_written_file() {
    let dir = TempDir::new().expect("a temporary directory");
    let argv = ["proiectio", "conf", "unset", "owner"];

    let json = harness(&dir)
        .output_mode(OutputMode::Json)
        .run(&app(), cli::command(), argv);
    json.assert_success();
    let value: JsonValue = serde_json::from_str(json.stdout()).expect("a JSON document");
    let path = Utf8PathBuf::from(value["path"].as_str().expect("the file the unset targeted"));
    assert_eq!(value["wrote"], false);
    assert!(!path.exists(), "the unset created {path}");

    let text = harness(&dir).run(&app(), cli::command(), argv);
    text.assert_success();
    assert!(
        !text.stdout().contains("wrote"),
        "an unset that wrote nothing claimed it wrote: {}",
        text.stdout()
    );
    text.assert_stdout_contains(&format!("no file at {path}"));

    let debug = harness(&dir)
        .output_mode(OutputMode::TermDebug)
        .run(&app(), cli::command(), argv);
    debug.assert_success();
    assert_tags_declared("an unset with no file", debug.stdout());
}

/// Unsetting is an edit like setting, so a key the schema does not declare is
/// the same typo it is there rather than a silent success.
#[test]
#[serial]
fn unsetting_a_key_the_schema_does_not_declare_fails_as_setting_one_does() {
    let dir = TempDir::new().expect("a temporary directory");

    for argv in [
        vec!["proiectio", "conf", "unset", "onwer"],
        vec!["proiectio", "conf", "set", "onwer", "site"],
    ] {
        let result = harness(&dir).run(&app(), cli::command(), argv.clone());

        assert_eq!(exit::status(result.outcome()), exit::FAILURE, "{argv:?}");
        assert!(
            result
                .error()
                .unwrap_or_default()
                .contains("Key not found: onwer"),
            "{argv:?}: {}",
            result.error().unwrap_or_default()
        );
    }
}

/// `config schema` allowlists `^//` on every object, so a file spelling a note
/// that way loads. A note is not a setting, and the listing leaves it in the
/// file the writer put it in.
#[test]
#[serial]
fn a_comment_key_the_schema_allowlists_loads_and_is_not_a_setting() {
    let dir = TempDir::new().expect("a temporary directory");
    let json = harness(&dir).output_mode(OutputMode::Json).run(
        &app(),
        cli::command(),
        ["proiectio", "conf", "set", "owner", "site"],
    );
    json.assert_success();
    let value: JsonValue = serde_json::from_str(json.stdout()).expect("a JSON document");
    let path = Utf8PathBuf::from(value["path"].as_str().expect("the file the set wrote"));
    let noted = format!(
        "\"//\" = \"a note\"\n{}",
        std::fs::read_to_string(&path).expect("the file the set named")
    );
    std::fs::write(&path, &noted).expect("a config file carrying a note");

    let listing = harness(&dir).run(&app(), cli::command(), ["proiectio", "conf", "list"]);

    listing.assert_success();
    assert_eq!(listing.stdout(), configured_listing());
}

/// The merged listing every `config list` without a scope renders here: the
/// two declared keys, `owner` set to `site` and the size bound left at its
/// compiled default.
fn configured_listing() -> String {
    format!(
        "max_source_size = {}\nowner = \"site\"\n",
        libproiectio::Limits::DEFAULT_MAX_SOURCE_BYTES
    )
}

/// The allowlist the loader honours is the one `config schema` publishes, read
/// off the emitted document rather than restated here: a validator handed that
/// schema and a file carrying a note agrees with the loader about both.
#[test]
#[serial]
fn the_emitted_schema_allowlists_the_comment_keys_the_loader_accepts() {
    let dir = TempDir::new().expect("a temporary directory");

    let schema = harness(&dir).run(&app(), cli::command(), ["proiectio", "conf", "schema"]);
    schema.assert_success();
    let emitted: JsonValue = serde_json::from_str(schema.stdout()).expect("a JSON Schema document");

    assert_eq!(
        emitted["patternProperties"]["^//"],
        serde_json::json!({}),
        "the schema publishes no comment-key allowlist: {}",
        schema.stdout()
    );
    assert_eq!(
        emitted["additionalProperties"], false,
        "the schema closes no object, so nothing needs allowlisting"
    );
}

/// A note under a table of its own is a note at every depth the schema
/// allowlists it, and the listing leaves the whole subtree in the file — the
/// key the loader saw is the table, not the leaf beneath it.
#[test]
#[serial]
fn a_comment_table_is_left_out_of_the_scope_that_reads_the_file_itself() {
    let dir = TempDir::new().expect("a temporary directory");
    let json = harness(&dir).output_mode(OutputMode::Json).run(
        &app(),
        cli::command(),
        ["proiectio", "conf", "set", "owner", "site"],
    );
    json.assert_success();
    let value: JsonValue = serde_json::from_str(json.stdout()).expect("a JSON document");
    let path = Utf8PathBuf::from(value["path"].as_str().expect("the file the set wrote"));
    std::fs::write(
        &path,
        "owner = \"site\"\n\n[\"//notes\"]\nwhy = \"a note\"\n",
    )
    .expect("a config file carrying a noted table");

    // The scoped listing reads the file, which carries only `owner`; the
    // merged one also carries the size bound's compiled default.
    for (argv, expected) in [
        (vec!["proiectio", "conf", "list"], configured_listing()),
        (
            vec!["proiectio", "conf", "list", "--scope", "user"],
            "owner = \"site\"\n".to_owned(),
        ),
    ] {
        let listing = harness(&dir).run(&app(), cli::command(), argv.clone());

        listing.assert_success();
        assert_eq!(listing.stdout(), expected, "{argv:?}");
    }
}

/// `set`, `get` and `unset` all read the same key argument, so an invocation
/// wrong in both its key and its scope reports the same one of them first
/// whichever command it named.
#[test]
#[serial]
fn the_edit_commands_agree_on_which_wrong_argument_they_report_first() {
    let dir = TempDir::new().expect("a temporary directory");

    for argv in [
        vec![
            "proiectio",
            "conf",
            "set",
            "--scope",
            "local",
            "onwer",
            "site",
        ],
        vec!["proiectio", "conf", "get", "--scope", "local", "onwer"],
        vec!["proiectio", "conf", "unset", "--scope", "local", "onwer"],
    ] {
        let result = harness(&dir).run(&app(), cli::command(), argv.clone());

        assert_eq!(exit::status(result.outcome()), exit::FAILURE, "{argv:?}");
        let error = result.error().unwrap_or_default();
        assert!(
            error.contains("Key not found: onwer"),
            "{argv:?} reported the scope before the key: {error}"
        );
    }
}

/// A note is not a setting, so it is not a key `get` answers for, any more
/// than `set` and `unset` accept one. The listing leaves it in the file for
/// the same reason.
#[test]
#[serial]
fn a_comment_key_is_not_one_the_reading_commands_answer_for() {
    let dir = TempDir::new().expect("a temporary directory");
    let json = harness(&dir).output_mode(OutputMode::Json).run(
        &app(),
        cli::command(),
        ["proiectio", "conf", "set", "owner", "site"],
    );
    json.assert_success();
    let value: JsonValue = serde_json::from_str(json.stdout()).expect("a JSON document");
    let path = Utf8PathBuf::from(value["path"].as_str().expect("the file the set wrote"));
    std::fs::write(&path, "owner = \"site\"\n\"//\" = \"a note\"\n").expect("a noted config file");

    for argv in [
        vec!["proiectio", "conf", "get", "//"],
        vec!["proiectio", "conf", "get", "//", "--scope", "user"],
    ] {
        let result = harness(&dir).run(&app(), cli::command(), argv.clone());

        assert_eq!(exit::status(result.outcome()), exit::FAILURE, "{argv:?}");
        assert!(
            result
                .error()
                .unwrap_or_default()
                .contains("Key not found: //"),
            "{argv:?}: {}",
            result.error().unwrap_or_default()
        );
    }
}

/// A scoped listing reads the file itself rather than the merged schema, so
/// the keys it prints are the writer's. One a bare TOML key cannot carry is
/// quoted, or the line it prints would not parse back.
#[test]
#[serial]
fn a_listed_key_a_bare_toml_key_cannot_carry_is_quoted() {
    let dir = TempDir::new().expect("a temporary directory");
    let json = harness(&dir).output_mode(OutputMode::Json).run(
        &app(),
        cli::command(),
        ["proiectio", "conf", "set", "owner", "site"],
    );
    json.assert_success();
    let value: JsonValue = serde_json::from_str(json.stdout()).expect("a JSON document");
    let path = Utf8PathBuf::from(value["path"].as_str().expect("the file the set wrote"));
    std::fs::write(&path, "owner = \"site\"\n\"a b\" = 1\n")
        .expect("a config file with an odd key");

    let listing = harness(&dir).run(
        &app(),
        cli::command(),
        ["proiectio", "conf", "list", "--scope", "user"],
    );

    listing.assert_success();
    let parsed: toml::Table = listing
        .stdout()
        .parse()
        .unwrap_or_else(|error| panic!("the listing printed {:?}: {error}", listing.stdout()));
    assert_eq!(parsed["a b"].as_integer(), Some(1));
    assert_eq!(parsed["owner"].as_str(), Some("site"));
}

/// `user` is the only scope the builder registers, so no other spelling can
/// reach a file — and the run that names one says which scopes exist rather
/// than reporting a write to the user scope.
#[test]
#[serial]
fn a_scope_the_builder_does_not_register_is_refused_by_name() {
    let dir = TempDir::new().expect("a temporary directory");

    for argv in [
        vec![
            "proiectio",
            "conf",
            "set",
            "--scope",
            "local",
            "owner",
            "site",
        ],
        vec!["proiectio", "conf", "unset", "--scope", "local", "owner"],
    ] {
        let result = harness(&dir).run(&app(), cli::command(), argv.clone());

        assert_eq!(exit::status(result.outcome()), exit::FAILURE, "{argv:?}");
        let error = result.error().unwrap_or_default();
        assert!(
            error.contains("Unknown scope 'local'") && error.contains("user"),
            "{argv:?}: {error}"
        );
    }
}

/// The `rendered` field of a config view, read back through the same run under
/// `--output json`.
fn rendered_field<'a>(dir: &TempDir, argv: impl IntoIterator<Item = &'a str>) -> String {
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

/// Every control character the C0 and C1 blocks carry, and the delete the
/// first of them ends with, leaves as an escape the terminal only shows.
#[test]
fn a_control_character_leaves_as_an_escape_the_terminal_only_shows() {
    assert_eq!(verbatim("\u{1b}[31mred"), r"\u{1b}\[31mred");
    assert_eq!(verbatim("two\nlines"), r"two\nlines");
    assert_eq!(verbatim("carriage\rreturn"), r"carriage\rreturn");
    assert_eq!(
        verbatim("bell\u{7}del\u{7f}csi\u{9b}"),
        r"bell\u{7}del\u{7f}csi\u{9b}"
    );

    for code in (0..=0x1f_u32).chain(0x7f..=0x9f) {
        let character = char::from_u32(code).expect("a control character");
        let escaped = verbatim(&character.to_string());
        assert!(
            !escaped.chars().any(char::is_control),
            "U+{code:04X} reached the terminal as itself: {escaped:?}"
        );
    }
}

/// A block clapfig rendered keeps the lines it spelled, and nothing else: the
/// `config gen` template and a documented `config get` are both several lines.
#[test]
fn a_rendered_block_keeps_its_lines_and_escapes_the_rest() {
    assert_eq!(
        verbatim_block("# owner\nowner = default\n"),
        "# owner\nowner = default\n"
    );
    assert_eq!(
        verbatim_block("owner = \u{1b}[31mred"),
        r"owner = \u{1b}\[31mred"
    );
}

/// A filename is data the destination supplies, not a command for the
/// terminal. One carrying an escape sequence or a newline renders as visible
/// escapes on its own single row, and structured output keeps the bytes.
#[test]
#[serial]
fn a_path_carrying_control_characters_renders_as_visible_escapes() {
    const ESCAPE: &str = "\u{1b}[31mred";
    const NEWLINE: &str = "two\nlines";
    /// The three rows `classified` leaves, plus the two files below.
    const ROWS: usize = 5;

    let (dir, dest) = classified_dir();
    std::fs::write(dest.join(ESCAPE), b"not ours\n").expect("a stray file");
    std::fs::write(dest.join(NEWLINE), b"not ours\n").expect("a stray file");
    let argv = ["proiectio", "status", "--dest", dest.as_str()];

    let text = harness(&dir).run(&app(), cli::command(), argv);
    text.assert_success();
    text.assert_stdout_contains(r"foreign  \u{1b}[31mred");
    text.assert_stdout_contains(r"foreign  two\nlines");
    let stdout = text.stdout();
    assert!(
        !stdout.contains('\u{1b}'),
        "an escape sequence reached the terminal: {stdout:?}"
    );
    assert_eq!(stdout.lines().count(), ROWS, "{stdout:?}");

    let term = harness(&dir)
        .output_mode(OutputMode::Term)
        .run(&app(), cli::command(), argv);
    term.assert_success();
    term.assert_stdout_contains(r"\u{1b}[31mred");
    term.assert_stdout_contains(r"two\nlines");
    assert_eq!(term.stdout().lines().count(), ROWS, "{:?}", term.stdout());

    let json = harness(&dir)
        .output_mode(OutputMode::Json)
        .run(&app(), cli::command(), argv);
    json.assert_success();
    let value: JsonValue = serde_json::from_str(json.stdout()).expect("a JSON document");
    assert_eq!(stated(&value["rows"], ESCAPE)["verdict"], "Foreign");
    assert_eq!(stated(&value["rows"], NEWLINE)["verdict"], "Foreign");
}

/// Configuration-derived text goes through the same filter, so a stored value
/// cannot drive the terminal either.
#[test]
#[serial]
fn a_config_value_carrying_an_escape_sequence_renders_as_itself() {
    const SPELLED: &str = "\u{1b}[31mred";

    let dir = TempDir::new().expect("a temporary directory");
    let argv = ["proiectio", "conf", "set", "owner", SPELLED];

    let text = harness(&dir).run(&app(), cli::command(), argv);
    text.assert_success();
    text.assert_stdout_contains(r#"owner = "\u001B[31mred""#);
    assert!(
        !text.stdout().contains('\u{1b}'),
        "an escape sequence reached the terminal: {:?}",
        text.stdout()
    );

    let json = harness(&dir)
        .output_mode(OutputMode::Json)
        .run(&app(), cli::command(), argv);
    json.assert_success();
    let value: JsonValue = serde_json::from_str(json.stdout()).expect("a JSON document");
    assert_eq!(value["value"], SPELLED);
}

/// Clapfig creates the file before it reports the path it wrote, so a `--file`
/// this CLI could not render has to be refused first. The run leaves with 1 and
/// the directory it named is untouched.
#[cfg(unix)]
#[test]
#[serial]
fn a_config_file_path_that_is_not_utf8_is_rejected_before_anything_is_written() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let dir = TempDir::new().expect("a temporary directory");
    let mut file = dir.path().as_os_str().to_owned();
    file.push(OsString::from_vec(vec![0x2f, 0xff]));
    let argv = vec![
        OsString::from("proiectio"),
        OsString::from("config"),
        OsString::from("gen"),
        OsString::from("--file"),
        file,
    ];

    let result = harness(&dir).run(&app(), cli::command(), argv);

    assert_eq!(exit::status(result.outcome()), exit::FAILURE);
    assert_eq!(
        std::fs::read_dir(dir.path())
            .expect("the temporary directory")
            .count(),
        0
    );
}

fn write_argv<'a>(source: &[&'a str], dest: &'a Utf8PathBuf) -> Vec<&'a str> {
    let mut argv = vec!["proiectio", "write"];
    argv.extend_from_slice(source);
    argv.extend_from_slice(&["--dest", dest.as_str()]);
    argv
}

/// A mapping file names each projected path, its content, and its executable
/// bit; the run reports one line per path and counts what it did
/// (`docs/cli-tour.lex` section 1).
#[test]
#[serial]
fn write_projects_a_mapping_file() {
    let (dir, dest, deploy) = tour();

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&[deploy.as_str()], &dest),
    );

    result.assert_success();
    assert_eq!(
        result.stdout(),
        "wrote      bin/tool              (exec)\n\
         wrote      config/settings.toml\n\
         linked     current               -> releases/1.2.3\n\
         3 written, 0 skipped\n"
    );
    assert_eq!(
        std::fs::read(dest.join("config/settings.toml")).expect("the projected file"),
        b"listen = \":8080\"\n"
    );
    assert_eq!(
        std::fs::read_link(dest.join("current")).expect("the projected link"),
        std::path::Path::new("releases/1.2.3")
    );
}

/// The destination and the owner are the invocation's, never the mapping's.
#[test]
#[serial]
fn write_records_the_owner_the_invocation_names() {
    let (dir, dest, deploy) = tour();

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&[deploy.as_str(), "--owner", "site"], &dest),
    );

    result.assert_success();
    assert_eq!(
        manifest_of(&dest).entries[Utf8Path::new("bin/tool")].owners,
        std::collections::BTreeSet::from(["site".to_owned()])
    );
}

/// Without the flag the owner is the configured one, which is what the
/// `owner` setting is for.
#[test]
#[serial]
fn write_falls_back_to_the_configured_owner() {
    let (dir, dest, deploy) = tour();
    harness(&dir)
        .run(
            &app(),
            cli::command(),
            ["proiectio", "conf", "set", "owner", "configured"],
        )
        .assert_success();

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&[deploy.as_str()], &dest),
    );

    result.assert_success();
    assert_eq!(
        manifest_of(&dest).entries[Utf8Path::new("bin/tool")].owners,
        std::collections::BTreeSet::from(["configured".to_owned()])
    );
}

/// The file the `user` scope persists to, learned the way an operator learns
/// it: `config set` reports the path it wrote.
fn user_config_file(dir: &TempDir) -> Utf8PathBuf {
    let json = harness(dir).output_mode(OutputMode::Json).run(
        &app(),
        cli::command(),
        ["proiectio", "conf", "set", "owner", "site"],
    );
    json.assert_success();
    let value: JsonValue = serde_json::from_str(json.stdout()).expect("a JSON document");
    Utf8PathBuf::from(value["path"].as_str().expect("the file the set wrote"))
}

/// `config set` is the layer that would put the phantom in the file, so it
/// refuses one: the run leaves with 1 and the file keeps the owner it had.
#[test]
#[serial]
fn config_set_refuses_an_owner_that_names_nothing() {
    let dir = TempDir::new().expect("a temporary directory");
    let path = user_config_file(&dir);
    let before = std::fs::read_to_string(&path).expect("the config file");

    for value in ["", "  "] {
        let result = harness(&dir).run(
            &app(),
            cli::command(),
            ["proiectio", "conf", "set", "owner", value],
        );

        assert_eq!(exit::status(result.outcome()), exit::FAILURE, "{value:?}");
        assert!(
            result
                .error()
                .unwrap_or_default()
                .contains(libproiectio::OWNER_RULE),
            "{value:?}: {}",
            result.error().unwrap_or_default()
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("the config file"),
            before,
            "{value:?}"
        );
    }
}

/// A file can carry the phantom without `config set` — an editor wrote it, or
/// `PROIECTIO__OWNER` arrived empty — so the run keeps the rule again where it
/// reads the owner. Both commands that record one stop before touching the
/// destination.
#[test]
#[serial]
fn a_configured_owner_that_names_nothing_stops_a_run() {
    let (dir, dest, deploy) = tour();
    let path = user_config_file(&dir);
    std::fs::write(&path, "owner = \"\"\n").expect("a config file naming no owner");
    let write = write_argv(&[deploy.as_str()], &dest);
    let remove = vec!["proiectio", "rm", "--dest", dest.as_str()];

    for argv in [write, remove] {
        let result = harness(&dir).run(&app(), cli::command(), argv.clone());

        assert_eq!(exit::status(result.outcome()), exit::FAILURE, "{argv:?}");
        assert!(
            result
                .error()
                .unwrap_or_default()
                .contains(libproiectio::OWNER_RULE),
            "{argv:?}: {}",
            result.error().unwrap_or_default()
        );
        assert!(!dest.join("bin/tool").exists(), "{argv:?}");
    }
}

/// The environment layer is refused by the same check, over a file that names
/// an owner: `PROIECTIO__OWNER` wins the resolution, so an unset variable
/// spelled into it is the owner the run would have recorded.
#[test]
#[serial]
fn an_owner_the_environment_leaves_empty_stops_a_run() {
    let (dir, dest, deploy) = tour();
    user_config_file(&dir);

    let result = harness(&dir).env("PROIECTIO__OWNER", "").run(
        &app(),
        cli::command(),
        write_argv(&[deploy.as_str()], &dest),
    );

    assert_eq!(exit::status(result.outcome()), exit::FAILURE);
    assert!(
        result
            .error()
            .unwrap_or_default()
            .contains(libproiectio::OWNER_RULE),
        "{}",
        result.error().unwrap_or_default()
    );
    assert!(!dest.join("bin/tool").exists());
}

/// The rule stops runs, not readers: a file carrying the phantom still lists,
/// which is how an operator sees what to unset.
#[test]
#[serial]
fn a_configured_owner_that_names_nothing_still_lists() {
    let dir = TempDir::new().expect("a temporary directory");
    let path = user_config_file(&dir);
    std::fs::write(&path, "owner = \"\"\n").expect("a config file naming no owner");

    let result = harness(&dir).run(&app(), cli::command(), ["proiectio", "conf", "list"]);

    result.assert_success();
    assert!(
        result.stdout().contains("owner = \"\""),
        "{}",
        result.stdout()
    );
}

/// The rule refuses a name with nothing in it, not a name with a space in it:
/// an owner spelled with one is recorded exactly as the invocation spelled it.
#[test]
#[serial]
fn an_owner_with_a_space_in_it_is_recorded_as_it_is_spelled() {
    let (dir, dest, deploy) = tour();

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&[deploy.as_str(), "--owner", "my site"], &dest),
    );

    result.assert_success();
    assert_eq!(
        manifest_of(&dest).entries[Utf8Path::new("bin/tool")].owners,
        std::collections::BTreeSet::from(["my site".to_owned()])
    );
}

/// A directory is projected verbatim, keyed relative to its root.
#[test]
#[serial]
fn write_projects_a_directory_tree() {
    let (dir, dest, _) = tour();
    let skeleton = skeleton(&utf8(&dir));

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&["--tree", skeleton.as_str()], &dest),
    );

    result.assert_success();
    assert_eq!(
        result.stdout(),
        "wrote      nested/leaf.txt\n\
         wrote      top\n\
         2 written, 0 skipped\n"
    );
}

/// An archive is a tree too, and `--strip` drops the wrapper a release
/// tarball carries.
#[test]
#[serial]
fn write_projects_an_archive_under_strip() {
    let (dir, dest, _) = tour();
    let archive = tarball(&utf8(&dir));

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&["--tree", archive.as_str(), "--strip", "1"], &dest),
    );

    result.assert_success();
    assert_eq!(
        result.stdout(),
        "wrote      nested/leaf.txt\n\
         wrote      top\n\
         2 written, 0 skipped\n"
    );
    assert_eq!(
        std::fs::read(dest.join("nested/leaf.txt")).expect("the projected member"),
        b"leaf\n"
    );
}

/// A tarball from stock macOS `tar` carries an AppleDouble `._*` sibling at
/// depth 1, which `--strip 1` leaves with no path. The rest of the archive
/// projects, and the run says which member was dropped and where it came
/// from.
#[test]
#[serial]
fn write_drops_the_members_strip_erases_and_projects_the_rest() {
    let (dir, dest, _) = tour();
    let archive = appledouble_tarball(&utf8(&dir));

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&["--tree", archive.as_str(), "--strip", "1"], &dest),
    );

    result.assert_success();
    assert_eq!(
        result.stdout(),
        format!(
            "wrote      top\n\
             dropped    ._skeleton-1.2  (no path left after strip 1) (from archive {archive})\n\
             1 written, 0 skipped\n"
        )
    );
    assert_eq!(
        std::fs::read(dest.join("top")).expect("the projected member"),
        b"top\n"
    );
    assert!(!dest.join("._skeleton-1.2").exists());
}

/// A `--strip` deeper than the archive erases every member, and that fails
/// the run rather than projecting an empty tree. An empty desired tree plans
/// a removal, so letting it through would clear everything the owner holds
/// on a mistyped number: the paths already written stay put, and the run
/// says how many members the strip consumed.
#[test]
#[serial]
fn a_strip_that_erases_every_member_fails_and_removes_nothing() {
    let (dir, dest, _) = tour();
    let archive = flat_tarball(&utf8(&dir));
    let held = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&["--tree", tarball(&utf8(&dir)).as_str()], &dest),
    );
    held.assert_success();
    let before = manifest_of(&dest);

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&["--tree", archive.as_str(), "--strip", "1"], &dest),
    );

    assert_eq!(exit::status(result.outcome()), exit::FAILURE);
    assert_eq!(manifest_of(&dest), before);
    assert!(dest.join("skeleton-1.2/top").exists());
}

/// The same drop under structured output, in both tenses: it rides the
/// library's own report, beside the rows rather than among them, since a
/// dropped member is spelled as the archive spells it and not as a path in
/// the destination. A plan flattens its rows beside `dropped` and an apply
/// nests them under `report`, so `dropped` sits at the top level either way.
#[test]
#[serial]
fn a_dropped_member_rides_the_librarys_own_report() {
    let (dir, dest, _) = tour();
    let archive = appledouble_tarball(&utf8(&dir));
    let record = serde_json::json!([{
        "member": "._skeleton-1.2",
        "prefix": "",
        "strip": 1,
        "origin": { "Archive": { "path": archive.as_str(), "via": null } },
    }]);

    let planned = harness(&dir).output_mode(OutputMode::Json).run(
        &app(),
        cli::command(),
        write_argv(
            &["--tree", archive.as_str(), "--strip", "1", "--dry-run"],
            &dest,
        ),
    );

    planned.assert_success();
    let value: JsonValue = serde_json::from_str(planned.stdout()).expect("a JSON document");
    assert_eq!(stated(&value["rows"], "top")["verdict"], "Write");
    assert!(row(&value["rows"], "._skeleton-1.2").is_none());
    assert_eq!(value["dropped"], record);

    let applied = harness(&dir).output_mode(OutputMode::Json).run(
        &app(),
        cli::command(),
        write_argv(&["--tree", archive.as_str(), "--strip", "1"], &dest),
    );

    applied.assert_success();
    let value: JsonValue = serde_json::from_str(applied.stdout()).expect("a JSON document");
    assert_eq!(
        stated(&value["report"]["rows"], "top")["verdict"],
        "Written"
    );
    assert!(row(&value["report"]["rows"], "._skeleton-1.2").is_none());
    assert!(value["report"].get("dropped").is_none());
    assert_eq!(value["dropped"], record);
}

/// Without `--strip` the wrapper is part of the tree, which is the same rule
/// read the other way.
#[test]
#[serial]
fn an_archive_without_strip_keeps_its_leading_component() {
    let (dir, dest, _) = tour();
    let archive = tarball(&utf8(&dir));

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&["--tree", archive.as_str()], &dest),
    );

    result.assert_success();
    result.assert_stdout_contains("skeleton-1.2/top");
}

/// An archive packaged as `tar czf dot.tgz -C skel .` names every member
/// with a leading `./`, which is no part of the path it projects to.
#[test]
#[serial]
fn write_projects_an_archive_whose_members_carry_a_leading_dot() {
    let (dir, dest, _) = tour();
    let archive = dot_tarball(&utf8(&dir));

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&["--tree", archive.as_str()], &dest),
    );

    result.assert_success();
    assert_eq!(
        result.stdout(),
        "wrote      x/a.txt\n\
         1 written, 0 skipped\n"
    );
    assert_eq!(
        std::fs::read(dest.join("x/a.txt")).expect("the projected member"),
        b"a\n"
    );
}

/// Loose files are a one-entry-per-basename tree.
#[test]
#[serial]
fn write_projects_loose_files_under_their_basenames() {
    let (dir, dest, _) = tour();
    let root = utf8(&dir);
    std::fs::write(root.join("motd"), b"motd\n").expect("a loose file");
    std::fs::write(root.join("banner.txt"), b"banner\n").expect("a loose file");

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(
            &[root.join("motd").as_str(), root.join("banner.txt").as_str()],
            &dest,
        ),
    );

    result.assert_success();
    assert_eq!(
        result.stdout(),
        "wrote      banner.txt\n\
         wrote      motd\n\
         2 written, 0 skipped\n"
    );
}

/// Re-running a write is a no-op: every path is skipped and its mtime
/// survives.
#[test]
#[serial]
fn re_running_a_write_skips_every_path_and_keeps_their_mtimes() {
    let (dir, dest, deploy) = tour();
    let argv = write_argv(&[deploy.as_str()], &dest);
    harness(&dir)
        .run(&app(), cli::command(), argv.clone())
        .assert_success();
    let before: Vec<_> = ["bin/tool", "config/settings.toml"]
        .map(|path| modified(&dest.join(path)))
        .to_vec();

    let result = harness(&dir).run(&app(), cli::command(), argv);

    result.assert_success();
    assert_eq!(
        result.stdout(),
        "skipped    bin/tool              (exec)\n\
         skipped    config/settings.toml\n\
         skipped    current               -> releases/1.2.3\n\
         3 unchanged\n"
    );
    let after: Vec<_> = ["bin/tool", "config/settings.toml"]
        .map(|path| modified(&dest.join(path)))
        .to_vec();
    assert_eq!(before, after);
}

/// A dry run reports the plan and writes nothing.
#[test]
#[serial]
fn a_dry_run_reports_the_plan_and_writes_nothing() {
    let (dir, dest, deploy) = tour();

    let verdict = exit::Verdict::default();
    let result = harness(&dir).run(
        &over(&verdict),
        cli::command(),
        write_argv(&[deploy.as_str(), "--dry-run"], &dest),
    );

    result.assert_success();
    assert_eq!(leaving(&result, &verdict), exit::OK);
    assert_eq!(
        result.stdout(),
        "would write      bin/tool              (exec)\n\
         would write      config/settings.toml\n\
         would link       current               -> releases/1.2.3\n"
    );
    assert!(!dest.join("bin/tool").exists());
    assert!(
        !dest.join(".proiectio").exists(),
        "a dry run created the state directory"
    );
}

/// A dry run reads outside the single-writer guard, so it reports the plan
/// while another writer holds the state directory's lock.
#[test]
#[serial]
fn a_dry_run_reports_the_plan_while_another_writer_holds_the_lock() {
    let (dir, dest, deploy) = tour();
    let held = Projection::new(&dest, None)
        .expect("a projection")
        .begin()
        .expect("a run holding the lock");

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&[deploy.as_str(), "--dry-run"], &dest),
    );

    result.assert_success();
    result.assert_stdout_contains("would write      config/settings.toml");
    drop(held);
}

/// A plan that overwrites says so, and why.
#[test]
#[serial]
fn a_dry_run_names_what_it_would_overwrite() {
    let (dir, dest, deploy) = tour();
    harness(&dir)
        .run(
            &app(),
            cli::command(),
            write_argv(&[deploy.as_str()], &dest),
        )
        .assert_success();
    std::fs::write(deploy.as_std_path(), MAPPING_WITH_CHANGED_CONTENT).expect("an edited mapping");

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&[deploy.as_str(), "--dry-run"], &dest),
    );

    result.assert_success();
    result.assert_stdout_contains("would overwrite  config/settings.toml  (content changed)");
}

const MAPPING_WITH_CHANGED_CONTENT: &[u8] = b"version = 1\n\
    \n\
    [files.\"config/settings.toml\"]\n\
    contents = \"listen = \\\":9090\\\"\\n\"\n";

/// The same three entries the tour projects, with the settings file's
/// contents changed, so a plan over a projected destination carries an
/// overwrite beside whatever else it found.
const MAPPING_WITH_CHANGED_SETTINGS: &[u8] = b"version = 1\n\
    \n\
    [files.\"config/settings.toml\"]\n\
    contents = \"listen = \\\":9090\\\"\\n\"\n\
    \n\
    [files.\"bin/tool\"]\n\
    source = \"./assets/tool.sh\"\n\
    executable = true\n\
    \n\
    [links.\"current\"]\n\
    target = \"releases/1.2.3\"\n";

/// A destination whose plan carries a refusal beside the rows a run would
/// act on: one path edited on disk, and one the mapping changed.
fn refused_and_overwritten(dir: &TempDir, dest: &Utf8PathBuf, deploy: &Utf8PathBuf) {
    harness(dir)
        .run(&app(), cli::command(), write_argv(&[deploy.as_str()], dest))
        .assert_success();
    std::fs::write(dest.join("bin/tool"), b"#!/bin/sh\necho edited\n").expect("a local edit");
    std::fs::write(deploy.as_std_path(), MAPPING_WITH_CHANGED_SETTINGS).expect("an edited mapping");
}

/// A refused dry run is still a dry run: it renders the whole plan — the rows
/// a run would act on beside the rows it refuses, each naming its reason —
/// and leaves with the refusal rather than replacing the plan with a
/// diagnostic.
#[test]
#[serial]
fn a_refused_dry_run_renders_the_whole_plan() {
    let (dir, dest, deploy) = tour();
    refused_and_overwritten(&dir, &dest, &deploy);
    let verdict = exit::Verdict::default();

    let result = harness(&dir).run(
        &over(&verdict),
        cli::command(),
        write_argv(&[deploy.as_str(), "--dry-run"], &dest),
    );

    assert_eq!(leaving(&result, &verdict), exit::REFUSAL);
    assert_eq!(
        result.stdout(),
        format!(
            "would refuse     bin/tool              (drifted) (from mapping {deploy})\n\
             would overwrite  config/settings.toml  (content changed)\n\
             would skip       current               -> releases/1.2.3\n"
        )
    );
    assert_eq!(result.error(), None);
    assert_eq!(
        std::fs::read(dest.join("bin/tool")).expect("the edited file"),
        b"#!/bin/sh\necho edited\n"
    );
}

/// And structured output is the library's own plan document, refusals and
/// all, on the same status.
#[test]
#[serial]
fn a_refused_dry_run_is_the_librarys_own_plan_document() {
    let (dir, dest, deploy) = tour();
    refused_and_overwritten(&dir, &dest, &deploy);
    let verdict = exit::Verdict::default();

    let result = harness(&dir).output_mode(OutputMode::Json).run(
        &over(&verdict),
        cli::command(),
        write_argv(&[deploy.as_str(), "--dry-run"], &dest),
    );

    assert_eq!(leaving(&result, &verdict), exit::REFUSAL);
    let value: JsonValue = serde_json::from_str(result.stdout()).expect("a JSON document");
    let planned = Projection::new(&dest, None)
        .expect("a projection")
        .plan(
            crate::testing::OWNER,
            &libproiectio::load_mapping(&deploy, libproiectio::Limits::default())
                .expect("a desired tree"),
            libproiectio::PlanOptions::default(),
        )
        .expect("a plan");
    assert_eq!(
        value,
        serde_json::to_value(planned.report()).expect("a serialized report")
    );
    let tool = stated(&value["rows"], "bin/tool");
    assert_eq!(tool["verdict"]["Refuse"]["refusal"], "Drift");
    assert_eq!(tool["facts"]["origin"]["Mapping"]["path"], deploy.as_str());
}

/// A refused real run performed nothing, so it keeps the error channel and
/// the diagnostic that names what it refused.
#[test]
#[serial]
fn a_refused_real_run_keeps_the_error_channel() {
    let (dir, dest, deploy) = tour();
    refused_and_overwritten(&dir, &dest, &deploy);
    let verdict = exit::Verdict::default();

    let result = harness(&dir).run(
        &over(&verdict),
        cli::command(),
        write_argv(&[deploy.as_str()], &dest),
    );

    assert_eq!(leaving(&result, &verdict), exit::REFUSAL);
    assert_eq!(result.stdout(), "");
    assert!(
        result
            .error()
            .unwrap_or_default()
            .contains("refusing to touch drifted paths"),
        "{}",
        result.error().unwrap_or_default()
    );
}

/// A refused row is styled like every other: the stylesheet declares its
/// family and the theme resolves it under both colour modes.
#[test]
#[serial]
fn a_refused_row_names_only_styles_the_stylesheet_declares() {
    let (dir, dest, deploy) = tour();
    refused_and_overwritten(&dir, &dest, &deploy);
    let argv = write_argv(&[deploy.as_str(), "--dry-run"], &dest);

    let debug =
        harness(&dir)
            .output_mode(OutputMode::TermDebug)
            .run(&app(), cli::command(), argv.clone());
    assert_tags_declared("a refused plan", debug.stdout());
    assert!(
        debug.stdout().contains("[refused]would refuse[/refused]"),
        "{}",
        debug.stdout()
    );

    let term = harness(&dir)
        .output_mode(OutputMode::Term)
        .run(&app(), cli::command(), argv);
    assert_styles_resolved("a refused plan", term.stdout());
}

/// The exit code is the verdict on a dry run and a real one alike: a drifted
/// path refuses either way, and names itself.
#[test]
#[serial]
fn a_drifted_path_refuses_on_a_dry_run_and_a_real_one_alike() {
    let (dir, dest, deploy) = tour();
    harness(&dir)
        .run(
            &app(),
            cli::command(),
            write_argv(&[deploy.as_str()], &dest),
        )
        .assert_success();
    std::fs::write(dest.join("bin/tool"), b"#!/bin/sh\necho edited\n").expect("a local edit");

    for extra in [vec![], vec!["--dry-run"]] {
        let mut source = vec![deploy.as_str()];
        source.extend_from_slice(&extra);
        let verdict = exit::Verdict::default();

        let result = harness(&dir).run(&over(&verdict), cli::command(), write_argv(&source, &dest));

        assert_eq!(
            leaving(&result, &verdict),
            exit::REFUSAL,
            "{extra:?}: {}",
            result.error().unwrap_or_default()
        );
        if extra.is_empty() {
            assert!(
                result.error().unwrap_or_default().contains("bin/tool"),
                "{extra:?}: {}",
                result.error().unwrap_or_default()
            );
        } else {
            result.assert_stdout_contains("would refuse     bin/tool              (drifted)");
        }
    }
    assert_eq!(
        std::fs::read(dest.join("bin/tool")).expect("the edited file"),
        b"#!/bin/sh\necho edited\n"
    );
}

/// `--force` lifts the drift refusal, one policy at a time and always from
/// the invocation.
#[test]
#[serial]
fn force_overwrites_a_drifted_path() {
    let (dir, dest, deploy) = tour();
    harness(&dir)
        .run(
            &app(),
            cli::command(),
            write_argv(&[deploy.as_str()], &dest),
        )
        .assert_success();
    std::fs::write(dest.join("bin/tool"), b"#!/bin/sh\necho edited\n").expect("a local edit");

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&[deploy.as_str(), "--force"], &dest),
    );

    result.assert_success();
    result.assert_stdout_contains("overwrote  bin/tool");
    result.assert_stdout_contains("1 written, 2 skipped");
    assert_eq!(
        std::fs::read(dest.join("bin/tool")).expect("the overwritten file"),
        b"#!/bin/sh\necho hi\n"
    );
}

/// A path two owners hold, dropped by one of them: the row reads released,
/// the file stays on disk, and the summary counts it apart from a removal.
#[test]
#[serial]
fn dropping_a_path_another_owner_holds_releases_it_rather_than_removing_it() {
    let (dir, dest, _) = tour();
    let root = utf8(&dir);
    let shared = root.join("shared.toml");
    std::fs::write(
        shared.as_std_path(),
        b"version = 1\n\n[files.\"conf\"]\ncontents = \"shared\\n\"\n",
    )
    .expect("a mapping two owners project");
    let apart = root.join("apart.toml");
    std::fs::write(
        apart.as_std_path(),
        b"version = 1\n\n[files.\"apart\"]\ncontents = \"apart\\n\"\n",
    )
    .expect("a mapping naming another path");
    for owner in ["one", "two"] {
        harness(&dir)
            .run(
                &app(),
                cli::command(),
                write_argv(&[shared.as_str(), "--owner", owner], &dest),
            )
            .assert_success();
    }

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&[apart.as_str(), "--owner", "one"], &dest),
    );

    result.assert_success();
    assert_eq!(
        result.stdout(),
        "wrote      apart\n\
         released   conf\n\
         1 written, 0 skipped, 1 released\n"
    );
    assert_eq!(
        std::fs::read(dest.join("conf")).expect("the path the other owner still holds"),
        b"shared\n"
    );
}

/// A released row states the owners the manifest records, on a dry run as on
/// the real one: planning reads them off the manifest it decided against, so
/// the two reports agree on who holds the path.
#[test]
#[serial]
fn a_dry_run_release_row_carries_the_owners_the_real_run_reports() {
    let (dir, dest, _) = tour();
    let root = utf8(&dir);
    let shared = root.join("shared.toml");
    std::fs::write(
        shared.as_std_path(),
        b"version = 1\n\n[files.\"conf\"]\ncontents = \"shared\\n\"\n",
    )
    .expect("a mapping two owners project");
    let apart = root.join("apart.toml");
    std::fs::write(
        apart.as_std_path(),
        b"version = 1\n\n[files.\"apart\"]\ncontents = \"apart\\n\"\n",
    )
    .expect("a mapping naming another path");
    for owner in ["one", "two"] {
        harness(&dir)
            .run(
                &app(),
                cli::command(),
                write_argv(&[shared.as_str(), "--owner", owner], &dest),
            )
            .assert_success();
    }

    let dry = harness(&dir).output_mode(OutputMode::Json).run(
        &app(),
        cli::command(),
        write_argv(&[apart.as_str(), "--owner", "one", "--dry-run"], &dest),
    );
    let applied = harness(&dir).output_mode(OutputMode::Json).run(
        &app(),
        cli::command(),
        write_argv(&[apart.as_str(), "--owner", "one"], &dest),
    );

    dry.assert_success();
    applied.assert_success();
    let planned: JsonValue = serde_json::from_str(dry.stdout()).expect("a JSON document");
    let real: JsonValue = serde_json::from_str(applied.stdout()).expect("a JSON document");
    let planned_conf = stated(&planned["rows"], "conf");
    let real_conf = stated(&real["report"]["rows"], "conf");
    assert_eq!(planned_conf["verdict"], "Release");
    assert_eq!(real_conf["verdict"], "Released");
    assert_eq!(
        planned_conf["facts"]["owners"],
        serde_json::json!(["one", "two"])
    );
    assert_eq!(planned_conf["facts"], real_conf["facts"]);
}

/// A symlink out of the destination refuses until the invocation permits it.
#[test]
#[serial]
fn an_external_symlink_target_refuses_until_the_invocation_allows_it() {
    let (dir, dest, _) = tour();
    let external = utf8(&dir).join("external.toml");
    std::fs::write(
        external.as_std_path(),
        b"version = 1\n\n[links.\"toolchain\"]\ntarget = \"/opt/toolchains/rust-1.80\"\n",
    )
    .expect("a mapping naming an external target");

    let refused = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&[external.as_str()], &dest),
    );
    assert_eq!(exit::status(refused.outcome()), exit::REFUSAL);
    assert!(!dest.join("toolchain").exists());

    let allowed = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&[external.as_str(), "--allow-external-targets"], &dest),
    );

    allowed.assert_success();
    allowed.assert_stdout_contains("linked     toolchain  -> /opt/toolchains/rust-1.80");
}

/// Structured output is the library's own plan report: no view model stands
/// between a consumer and the document the library serializes.
#[test]
#[serial]
fn a_dry_run_is_the_librarys_own_plan_report() {
    let (dir, dest, deploy) = tour();

    let result = harness(&dir).output_mode(OutputMode::Json).run(
        &app(),
        cli::command(),
        write_argv(&[deploy.as_str(), "--dry-run"], &dest),
    );

    result.assert_success();
    let value: JsonValue = serde_json::from_str(result.stdout()).expect("a JSON document");
    let projection = Projection::new(&dest, None).expect("a projection");
    let desired = libproiectio::load_mapping(&deploy, libproiectio::Limits::default())
        .expect("a desired tree");
    let planned = projection
        .plan(
            crate::testing::OWNER,
            &desired,
            libproiectio::PlanOptions::default(),
        )
        .expect("a plan");
    assert_eq!(
        value,
        serde_json::to_value(planned.report()).expect("a serialized report")
    );
}

/// And a real run is the library's own apply report, whose manifest reads
/// back as the one on disk.
#[test]
#[serial]
fn a_real_run_is_the_librarys_own_apply_report() {
    let (dir, dest, deploy) = tour();

    let result = harness(&dir).output_mode(OutputMode::Json).run(
        &app(),
        cli::command(),
        write_argv(&[deploy.as_str()], &dest),
    );

    result.assert_success();
    let value: JsonValue = serde_json::from_str(result.stdout()).expect("a JSON document");
    assert_eq!(
        value
            .as_object()
            .expect("an object")
            .keys()
            .collect::<Vec<_>>(),
        vec!["report", "manifest"]
    );
    assert_eq!(
        stated(&value["report"]["rows"], "bin/tool")["verdict"],
        "Written"
    );
    assert_eq!(
        serde_json::from_value::<Manifest>(value["manifest"].clone()).expect("a manifest"),
        manifest_of(&dest)
    );
}

/// One positional is a mapping file whatever it turns out to be: a directory
/// fails as an unreadable mapping rather than becoming a tree.
#[test]
#[serial]
fn a_directory_named_as_a_mapping_fails_rather_than_becoming_a_tree() {
    let (dir, dest, _) = tour();
    let skeleton = skeleton(&utf8(&dir));

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&[skeleton.as_str()], &dest),
    );

    assert_eq!(exit::status(result.outcome()), exit::FAILURE);
    assert!(!dest.join("top").exists());
}

/// A command line that names no desired tree, and one that pairs `--strip`
/// with no archive, are usage errors.
#[test]
#[serial]
fn a_command_line_that_names_no_desired_tree_is_a_usage_error() {
    let (dir, dest, deploy) = tour();

    for source in [
        vec![],
        vec!["--strip", "1"],
        vec![deploy.as_str(), "--strip", "1"],
    ] {
        let result = harness(&dir).run(&app(), cli::command(), write_argv(&source, &dest));

        assert_eq!(
            exit::status(result.outcome()),
            exit::FAILURE,
            "{source:?}: {}",
            result.error().unwrap_or_default()
        );
    }
}

#[test]
#[serial]
fn write_names_only_styles_the_stylesheet_declares_and_the_theme_resolves() {
    let (dir, dest, deploy) = tour();
    let argv = write_argv(&[deploy.as_str(), "--dry-run"], &dest);

    let debug =
        harness(&dir)
            .output_mode(OutputMode::TermDebug)
            .run(&app(), cli::command(), argv.clone());
    debug.assert_success();
    assert_tags_declared("write", debug.stdout());

    let term = harness(&dir).output_mode(OutputMode::Term).run(
        &app(),
        cli::command(),
        write_argv(&[deploy.as_str()], &dest),
    );
    term.assert_success();
    assert_styles_resolved("write", term.stdout());
}

/// The path column is measured in terminal columns, so an escaped bracket and
/// a wide character leave every note at the same offset.
#[test]
#[serial]
fn rows_align_on_display_width_rather_than_byte_length() {
    let (dir, dest, _) = tour();
    let links = utf8(&dir).join("links.toml");
    std::fs::write(
        links.as_std_path(),
        "version = 1\n\n[links.\"[a]\"]\ntarget = \"x\"\n\n[links.\"\u{65e5}\u{672c}\u{8a9e}\"]\ntarget = \"y\"\n",
    )
    .expect("a mapping of two links");

    let result = harness(&dir).run(&app(), cli::command(), write_argv(&[links.as_str()], &dest));

    result.assert_success();
    assert_eq!(
        result.stdout(),
        "linked     [a]     -> x\n\
         linked     \u{65e5}\u{672c}\u{8a9e}  -> y\n\
         2 written, 0 skipped\n"
    );
}

/// A row keeps the style its verdict earned: only a link the run writes reads
/// as one.
#[test]
#[serial]
fn a_symlink_row_carries_the_style_of_its_verdict() {
    let (dir, dest, deploy) = tour();
    let argv = write_argv(&[deploy.as_str()], &dest);

    let written =
        harness(&dir)
            .output_mode(OutputMode::TermDebug)
            .run(&app(), cli::command(), argv.clone());
    written.assert_success();
    assert!(
        written.stdout().contains("[linked]linked[/linked]"),
        "a written link reads as one:\n{}",
        written.stdout()
    );

    let again = harness(&dir)
        .output_mode(OutputMode::TermDebug)
        .run(&app(), cli::command(), argv);
    again.assert_success();
    assert!(
        again.stdout().contains("[skipped]skipped[/skipped]"),
        "a skipped link keeps the skip style:\n{}",
        again.stdout()
    );
    assert!(
        !again.stdout().contains("[linked]"),
        "nothing was linked on the second run:\n{}",
        again.stdout()
    );
}

/// A projected path and a link target are data, not markup: both reach the
/// terminal as the characters they are.
#[test]
#[serial]
fn a_path_and_a_target_spelled_like_style_tags_render_as_themselves() {
    const PATH: &str = "[wrote]";
    const TARGET: &str = "[linked]/x";

    let (dir, dest, _) = tour();
    let spelled = utf8(&dir).join("spelled.toml");
    std::fs::write(
        spelled.as_std_path(),
        format!(
            "version = 1\n\n[files.\"{PATH}\"]\ncontents = \"x\\n\"\n\n\
             [links.\"link\"]\ntarget = \"{TARGET}\"\n"
        ),
    )
    .expect("a mapping spelled like markup");

    let text = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&[spelled.as_str()], &dest),
    );
    text.assert_success();
    text.assert_stdout_contains(PATH);
    text.assert_stdout_contains(TARGET);

    let term = harness(&dir).output_mode(OutputMode::Term).run(
        &app(),
        cli::command(),
        write_argv(&[spelled.as_str()], &dest),
    );
    term.assert_success();
    assert_styles_resolved("a write spelled like a tag", term.stdout());
}

// ---------------------------------------------------------------------------
// The size bound on untrusted input
// ---------------------------------------------------------------------------

/// A bound small enough that the tour's own mapping runs past it, so a test
/// need not build a large source to reach the error.
const TIGHT: &str = "64";

/// The bound is the run's, not the archive loader's alone: a mapping whose
/// text and source files outweigh it fails the run, and the diagnostic names
/// both the file being read and the number the run was held to.
#[test]
#[serial]
fn a_write_past_the_size_bound_fails_naming_the_limit() {
    let (dir, dest, deploy) = tour();
    let verdict = exit::Verdict::default();

    let result = harness(&dir).run(
        &over(&verdict),
        cli::command(),
        write_argv(&[deploy.as_str(), "--max-source-size", TIGHT], &dest),
    );

    assert_eq!(leaving(&result, &verdict), exit::FAILURE);
    assert_eq!(result.stdout(), "");
    let error = result.error().unwrap_or_default();
    assert!(
        error.contains(deploy.as_str())
            && error.contains("reads past the 64 bytes one load may hold in memory"),
        "{error}"
    );
}

/// The same bound written to the configuration rather than named on the
/// command line, which is the layer under the flag.
#[test]
#[serial]
fn the_configured_size_bound_holds_a_write_with_no_flag() {
    let (dir, dest, deploy) = tour();
    set_size_bound(&dir, TIGHT);
    let verdict = exit::Verdict::default();

    let result = harness(&dir).run(
        &over(&verdict),
        cli::command(),
        write_argv(&[deploy.as_str()], &dest),
    );

    assert_eq!(leaving(&result, &verdict), exit::FAILURE);
    assert!(
        result
            .error()
            .unwrap_or_default()
            .contains("one load may hold in memory"),
        "{}",
        result.error().unwrap_or_default()
    );
}

/// The three layers in order: the flag wins over the configured key, which
/// wins over the compiled default. The configured bound here would refuse the
/// run, and the flag naming a wider one lets it through.
#[test]
#[serial]
fn the_flag_wins_over_the_configured_size_bound() {
    let (dir, dest, deploy) = tour();
    set_size_bound(&dir, TIGHT);

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&[deploy.as_str(), "--max-source-size", "1048576"], &dest),
    );

    result.assert_success();
    assert!(dest.join("config/settings.toml").is_file());
}

/// With nothing configured and no flag, the compiled default applies, which
/// is wide enough for a mapping a tight bound refuses.
#[test]
#[serial]
fn the_compiled_default_applies_when_nothing_names_a_bound() {
    let (dir, dest, deploy) = tour();

    let result = harness(&dir).run(
        &app(),
        cli::command(),
        write_argv(&[deploy.as_str()], &dest),
    );

    result.assert_success();
    assert!(dest.join("config/settings.toml").is_file());
}

/// `config set` and `config get` carry the key like any other, and the value
/// reads back as the bare integer a TOML document spells rather than a
/// quoted string.
#[test]
#[serial]
fn the_size_bound_round_trips_through_set_and_get() {
    let dir = TempDir::new().expect("a temporary directory");
    let set = harness(&dir).run(
        &app(),
        cli::command(),
        ["proiectio", "conf", "set", "max_source_size", "4096"],
    );
    set.assert_success();

    let got = harness(&dir).run(
        &app(),
        cli::command(),
        ["proiectio", "conf", "get", "max_source_size"],
    );

    got.assert_success();
    assert!(
        got.stdout().ends_with("max_source_size = 4096\n"),
        "{}",
        got.stdout()
    );
}

/// Writes the size bound into the configuration the way an operator would,
/// through the CLI's own `config set`.
fn set_size_bound(dir: &TempDir, bytes: &str) {
    harness(dir)
        .run(
            &app(),
            cli::command(),
            ["proiectio", "conf", "set", "max_source_size", bytes],
        )
        .assert_success();
}
