//! The whole life of a destination through the shell, and the transcripts
//! `docs/cli-tour.lex` sections 3 and 4 print.

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::Value as JsonValue;
use serial_test::serial;
use standout::OutputMode;
use standout_test::TestResult;
use tempfile::TempDir;

use crate::app;
use crate::cli;
use crate::exit;
use crate::testing::{
    assert_styles_resolved, assert_tags_declared, harness, manifest_of, modified, tour, utf8,
};

/// One invocation through the whole argv-to-output pipeline, under the
/// rendered format the tour's transcripts are written in.
fn run(dir: &TempDir, args: &[&str]) -> TestResult {
    run_as(dir, OutputMode::Text, args)
}

fn run_as(dir: &TempDir, mode: OutputMode, args: &[&str]) -> TestResult {
    let mut argv = vec!["proiectio"];
    argv.extend_from_slice(args);
    harness(dir)
        .output_mode(mode)
        .run(&app::build().expect("an app"), cli::command(), argv)
}

/// What the destination holds, by name, at one level.
fn entries(dest: &Utf8Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dest)
        .expect("a destination")
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

fn mtimes(dest: &Utf8Path) -> Vec<std::time::SystemTime> {
    ["bin/tool", "config/settings.toml"]
        .map(|path| modified(&dest.join(path)))
        .to_vec()
}

const PROJECTED: &str = "wrote      bin/tool              (exec)\n\
                         wrote      config/settings.toml\n\
                         linked     current               -> releases/1.2.3\n\
                         3 written, 0 skipped\n";

const UNCHANGED: &str = "skipped    bin/tool              (exec)\n\
                         skipped    config/settings.toml\n\
                         skipped    current               -> releases/1.2.3\n\
                         3 unchanged\n";

const CLEARED: &str = "removed    bin/tool              (exec)\n\
                       removed    config/settings.toml\n\
                       removed    current\n\
                       3 removed\n";

const EDITED: &[u8] = b"#!/bin/sh\necho edited\n";
const PROJECTED_TOOL: &[u8] = b"#!/bin/sh\necho hi\n";

/// The whole life of a destination in one run of the shell: projected,
/// re-projected as a no-op, edited underneath, refused, forced, and cleared.
#[test]
#[serial]
fn a_destination_is_projected_re_projected_refused_forced_and_cleared() {
    let (dir, dest, deploy) = tour();
    let source = ["write", deploy.as_str(), "--dest", dest.as_str()];

    let projected = run(&dir, &source);
    projected.assert_success();
    assert_eq!(projected.stdout(), PROJECTED);

    let stamps = mtimes(&dest);
    let again = run(&dir, &source);
    again.assert_success();
    assert_eq!(again.stdout(), UNCHANGED);
    assert_eq!(stamps, mtimes(&dest), "a skipped path was rewritten");

    std::fs::write(dest.join("bin/tool").as_std_path(), EDITED).expect("a local edit");
    std::fs::remove_file(dest.join("current").as_std_path()).expect("a local removal");
    let drifted = run(&dir, &["status", "--dest", dest.as_str()]);
    drifted.assert_success();
    assert_eq!(
        drifted.stdout(),
        "drifted  bin/tool\nclean    config/settings.toml\nmissing  current\n"
    );

    let refused = run(&dir, &source);
    assert_eq!(exit::status(refused.outcome()), exit::REFUSAL);
    assert!(
        refused.error().unwrap_or_default().contains("bin/tool"),
        "{}",
        refused.error().unwrap_or_default()
    );

    let forced = run(
        &dir,
        &["write", deploy.as_str(), "--dest", dest.as_str(), "--force"],
    );
    forced.assert_success();
    forced.assert_stdout_contains("overwrote  bin/tool");
    assert_eq!(
        std::fs::read(dest.join("bin/tool")).expect("the overwritten file"),
        PROJECTED_TOOL
    );

    let cleared = run(&dir, &["rm", "--dest", dest.as_str()]);
    cleared.assert_success();
    assert_eq!(cleared.stdout(), CLEARED);
    assert_eq!(
        entries(&dest),
        vec![".proiectio".to_owned()],
        "removal left the destination holding more than its state directory"
    );

    let empty = run(&dir, &["status", "--dest", dest.as_str()]);
    empty.assert_success();
    assert_eq!(empty.stdout(), "");
    assert!(manifest_of(&dest).entries.is_empty());
}

/// The transcript `docs/cli-tour.lex` section 4 prints for a whole owner: the
/// invocation names the destination and the owner, never the manifest.
#[test]
#[serial]
fn rm_clears_everything_the_named_owner_holds() {
    let (dir, dest, deploy) = tour();
    run(
        &dir,
        &[
            "write",
            deploy.as_str(),
            "--dest",
            dest.as_str(),
            "--owner",
            "site",
        ],
    )
    .assert_success();

    let result = run(&dir, &["rm", "--dest", dest.as_str(), "--owner", "site"]);

    result.assert_success();
    assert_eq!(result.stdout(), CLEARED);
    assert_eq!(entries(&dest), vec![".proiectio".to_owned()]);
}

/// An owner the invocation does not name holds its paths still.
#[test]
#[serial]
fn rm_leaves_the_paths_another_owner_holds() {
    let (dir, dest, deploy) = tour();
    for owner in ["site", "other"] {
        run(
            &dir,
            &[
                "write",
                deploy.as_str(),
                "--dest",
                dest.as_str(),
                "--owner",
                owner,
            ],
        )
        .assert_success();
    }

    let result = run(&dir, &["rm", "--dest", dest.as_str(), "--owner", "site"]);

    result.assert_success();
    assert_eq!(
        result.stdout(),
        "released   bin/tool              (exec)\n\
         released   config/settings.toml\n\
         released   current\n\
         3 released\n"
    );
    assert_eq!(
        std::fs::read(dest.join("config/settings.toml")).expect("the path the other owner holds"),
        b"listen = \":8080\"\n"
    );
    assert_eq!(
        manifest_of(&dest).entries[Utf8Path::new("bin/tool")].owners,
        std::collections::BTreeSet::from(["other".to_owned()])
    );
}

/// The transcript `docs/cli-tour.lex` section 4 prints for a subset: the
/// positionals name recorded paths, and the rest of the owner's tree stays.
#[test]
#[serial]
fn rm_of_a_path_subset_leaves_the_rest() {
    let (dir, dest, deploy) = tour();
    run(&dir, &["write", deploy.as_str(), "--dest", dest.as_str()]).assert_success();

    let result = run(
        &dir,
        &["rm", "config/settings.toml", "--dest", dest.as_str()],
    );

    result.assert_success();
    assert_eq!(
        result.stdout(),
        "removed    config/settings.toml\n1 removed\n"
    );
    let remaining = run(&dir, &["status", "--dest", dest.as_str()]);
    remaining.assert_success();
    assert_eq!(remaining.stdout(), "clean    bin/tool\nclean    current\n");
}

/// A directory the removal emptied is pruned; one still holding a path is not.
#[test]
#[serial]
fn removal_prunes_the_directories_it_empties() {
    let (dir, dest, deploy) = tour();
    run(&dir, &["write", deploy.as_str(), "--dest", dest.as_str()]).assert_success();
    std::fs::write(dest.join("config/kept.toml").as_std_path(), b"kept\n").expect("a stray file");

    run(
        &dir,
        &[
            "rm",
            "bin/tool",
            "config/settings.toml",
            "--dest",
            dest.as_str(),
        ],
    )
    .assert_success();

    assert!(!dest.join("bin").exists(), "an emptied directory survived");
    assert!(
        dest.join("config/kept.toml").exists(),
        "pruning took a directory that still held a path"
    );
}

/// A drifted path refuses, names itself, and stays on disk until the
/// invocation lifts the policy.
#[test]
#[serial]
fn rm_of_a_drifted_path_refuses_until_force_lifts_the_policy() {
    let (dir, dest, deploy) = tour();
    run(&dir, &["write", deploy.as_str(), "--dest", dest.as_str()]).assert_success();
    std::fs::write(dest.join("bin/tool").as_std_path(), EDITED).expect("a local edit");

    let refused = run(&dir, &["rm", "bin/tool", "--dest", dest.as_str()]);

    assert_eq!(exit::status(refused.outcome()), exit::REFUSAL);
    assert!(
        refused.error().unwrap_or_default().contains("bin/tool"),
        "{}",
        refused.error().unwrap_or_default()
    );
    assert_eq!(
        std::fs::read(dest.join("bin/tool")).expect("the edited file"),
        EDITED
    );

    let forced = run(
        &dir,
        &["rm", "bin/tool", "--dest", dest.as_str(), "--force"],
    );

    forced.assert_success();
    forced.assert_stdout_contains("removed    bin/tool");
    assert!(!dest.join("bin/tool").exists());
}

/// A positional that climbs out of the destination is refused before anything
/// is read, and names the path it refused.
#[test]
#[serial]
fn rm_refuses_a_path_that_leaves_the_destination() {
    let (dir, dest, deploy) = tour();
    run(&dir, &["write", deploy.as_str(), "--dest", dest.as_str()]).assert_success();

    let result = run(&dir, &["rm", "../outside", "--dest", dest.as_str()]);

    assert_eq!(exit::status(result.outcome()), exit::REFUSAL);
    assert!(
        result.error().unwrap_or_default().contains("../outside"),
        "{}",
        result.error().unwrap_or_default()
    );
}

/// A dry run reports the plan and removes nothing, on the same exit contract
/// as the real run.
#[test]
#[serial]
fn a_dry_run_of_rm_reports_the_plan_and_removes_nothing() {
    let (dir, dest, deploy) = tour();
    run(&dir, &["write", deploy.as_str(), "--dest", dest.as_str()]).assert_success();

    let result = run(&dir, &["rm", "--dest", dest.as_str(), "--dry-run"]);

    result.assert_success();
    assert_eq!(
        result.stdout(),
        "would remove     bin/tool              (exec)\n\
         would remove     config/settings.toml\n\
         would remove     current\n"
    );
    assert!(dest.join("bin/tool").exists());
}

/// With nothing recorded under the owner there is nothing to remove, and the
/// run still succeeds.
#[test]
#[serial]
fn rm_of_an_owner_holding_nothing_succeeds() {
    let dir = TempDir::new().expect("a temporary directory");
    let dest = utf8(&dir);

    let result = run(&dir, &["rm", "--dest", dest.as_str()]);

    result.assert_success();
    assert_eq!(result.stdout(), "nothing to do\n");
}

/// Structured output is the library's own report, as `write`'s is: no view
/// model stands between a consumer and the document the library serializes.
#[test]
#[serial]
fn rm_is_the_librarys_own_reports() {
    let (dir, dest, deploy) = tour();
    run(&dir, &["write", deploy.as_str(), "--dest", dest.as_str()]).assert_success();

    let dry = run_as(
        &dir,
        OutputMode::Json,
        &["rm", "--dest", dest.as_str(), "--dry-run"],
    );
    let applied = run_as(&dir, OutputMode::Json, &["rm", "--dest", dest.as_str()]);

    dry.assert_success();
    applied.assert_success();
    let planned: JsonValue = serde_json::from_str(dry.stdout()).expect("a JSON document");
    let real: JsonValue = serde_json::from_str(applied.stdout()).expect("a JSON document");
    assert_eq!(planned["rows"]["bin/tool"]["verdict"], "Remove");
    assert_eq!(real["report"]["rows"]["bin/tool"]["verdict"], "Removed");
    assert_eq!(
        real["manifest"],
        serde_json::to_value(manifest_of(&dest)).expect("a serialized manifest")
    );
}

#[test]
#[serial]
fn rm_names_only_styles_the_stylesheet_declares_and_the_theme_resolves() {
    let (dir, dest, deploy) = tour();
    run(&dir, &["write", deploy.as_str(), "--dest", dest.as_str()]).assert_success();
    let argv = ["rm", "--dest", dest.as_str(), "--dry-run"];

    let debug = run_as(&dir, OutputMode::TermDebug, &argv);
    debug.assert_success();
    assert_tags_declared("rm", debug.stdout());

    let term = run_as(&dir, OutputMode::Term, &argv);
    term.assert_success();
    assert_styles_resolved("rm", term.stdout());
}

/// A projected path is data, not markup, on the way out as on the way in.
#[test]
#[serial]
fn a_removed_path_spelled_like_a_style_tag_renders_as_itself() {
    let (dir, dest, _) = tour();
    let mapping = utf8(&dir).join("tagged.toml");
    std::fs::write(
        mapping.as_std_path(),
        b"version = 1\n\n[files.\"[removed]tool[/removed]\"]\ncontents = \"x\\n\"\n",
    )
    .expect("a mapping naming a path spelled like a tag");
    run(&dir, &["write", mapping.as_str(), "--dest", dest.as_str()]).assert_success();

    let result = run(&dir, &["rm", "--dest", dest.as_str()]);

    result.assert_success();
    result.assert_stdout_contains("removed    [removed]tool[/removed]");
}

/// The manifest owner the invocation names wins over the configured one.
#[test]
#[serial]
fn rm_falls_back_to_the_configured_owner() {
    let (dir, dest, deploy) = tour();
    run(&dir, &["conf", "set", "owner", "configured"]).assert_success();
    run(&dir, &["write", deploy.as_str(), "--dest", dest.as_str()]).assert_success();
    assert_eq!(
        manifest_of(&dest).entries[Utf8Path::new("bin/tool")].owners,
        std::collections::BTreeSet::from(["configured".to_owned()])
    );

    run(&dir, &["rm", "--dest", dest.as_str()]).assert_success();

    assert!(manifest_of(&dest).entries.is_empty());
}

/// The state directory is the manifest's home wherever the invocation puts
/// it, and a removal reads and rewrites the one it names.
#[test]
#[serial]
fn rm_reads_the_manifest_the_state_dir_flag_names() {
    let (dir, dest, deploy) = tour();
    let state: Utf8PathBuf = utf8(&dir).join("state");
    let source = [
        "write",
        deploy.as_str(),
        "--dest",
        dest.as_str(),
        "--state-dir",
        state.as_str(),
    ];
    run(&dir, &source).assert_success();

    let result = run(
        &dir,
        &["rm", "--dest", dest.as_str(), "--state-dir", state.as_str()],
    );

    result.assert_success();
    assert_eq!(result.stdout(), CLEARED);
    assert_eq!(entries(&dest), Vec::<String>::new());
}
