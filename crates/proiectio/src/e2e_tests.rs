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
    assert_styles_resolved, assert_tags_declared, harness, manifest_of, modified, stated, tour,
    utf8,
};

/// One invocation through the whole argv-to-output pipeline, under the
/// rendered format the tour's transcripts are written in.
fn run(dir: &TempDir, args: &[&str]) -> TestResult {
    run_as(dir, OutputMode::Text, args)
}

fn run_as(dir: &TempDir, mode: OutputMode, args: &[&str]) -> TestResult {
    run_over(dir, mode, &exit::Verdict::default(), args)
}

/// The same, over a verdict the caller keeps: a refused run renders its plan
/// and leaves the refusal there rather than in the run's result.
fn run_over(dir: &TempDir, mode: OutputMode, verdict: &exit::Verdict, args: &[&str]) -> TestResult {
    let mut argv = vec!["proiectio"];
    argv.extend_from_slice(args);
    harness(dir).output_mode(mode).run(
        &app::build(verdict.clone()).expect("an app"),
        cli::command(),
        argv,
    )
}

/// The status the process leaves with: what emitting the run reports, over
/// what the run recorded.
fn leaving(result: &TestResult, verdict: &exit::Verdict) -> u8 {
    verdict.over(exit::status(result.outcome()))
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

/// The plan once `bin/tool` is edited underneath it and `current` removed,
/// as `docs/cli-tour.lex` section 2 prints it.
fn refused_plan(deploy: &Utf8Path) -> String {
    format!(
        "would refuse     bin/tool              (drifted) (from mapping {deploy})\n\
         would skip       config/settings.toml\n\
         would link       current               -> releases/1.2.3\n\
         pass --force to touch them anyway, where the projection can still \
         tell what it would replace\n"
    )
}

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

    let verdict = exit::Verdict::default();
    let mut planned = source.to_vec();
    planned.push("--dry-run");
    let dry = run_over(&dir, OutputMode::Text, &verdict, &planned);
    assert_eq!(leaving(&dry, &verdict), exit::REFUSAL);
    assert_eq!(dry.stdout(), refused_plan(&deploy));
    assert_eq!(dry.error(), None);

    let real = exit::Verdict::default();
    let refused = run_over(&dir, OutputMode::Text, &real, &source);
    assert_eq!(leaving(&refused, &real), exit::REFUSAL);
    assert_eq!(refused.stdout(), refused_plan(&deploy));
    assert_eq!(refused.error(), None);

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

/// The transcript `docs/cli-tour.lex` section 4 prints for a whole owner.
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

/// The transcript `docs/cli-tour.lex` section 4 prints for a subset.
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

#[test]
#[serial]
fn rm_of_a_drifted_path_refuses_until_force_lifts_the_policy() {
    let (dir, dest, deploy) = tour();
    run(&dir, &["write", deploy.as_str(), "--dest", dest.as_str()]).assert_success();
    std::fs::write(dest.join("bin/tool").as_std_path(), EDITED).expect("a local edit");

    let verdict = exit::Verdict::default();
    let refused = run_over(
        &dir,
        OutputMode::Text,
        &verdict,
        &["rm", "bin/tool", "--dest", dest.as_str()],
    );

    assert_eq!(leaving(&refused, &verdict), exit::REFUSAL);
    assert_eq!(
        refused.stdout(),
        "would refuse     bin/tool  (drifted)\npass --force to touch them anyway, where the projection can still \
         tell what it would replace\n"
    );
    assert_eq!(refused.error(), None);
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

#[test]
#[serial]
fn rm_refuses_a_path_that_leaves_the_destination() {
    let (dir, dest, deploy) = tour();
    run(&dir, &["write", deploy.as_str(), "--dest", dest.as_str()]).assert_success();

    let verdict = exit::Verdict::default();
    let result = run_over(
        &dir,
        OutputMode::Text,
        &verdict,
        &["rm", "../outside", "--dest", dest.as_str()],
    );

    assert_eq!(leaving(&result, &verdict), exit::REFUSAL);
    assert_eq!(
        result.stdout(),
        "would refuse     ../outside  (containment)\n"
    );
    assert_eq!(result.error(), None);
}

#[test]
#[serial]
fn a_dry_run_of_rm_reports_the_plan_and_removes_nothing() {
    let (dir, dest, deploy) = tour();
    run(&dir, &["write", deploy.as_str(), "--dest", dest.as_str()]).assert_success();

    let verdict = exit::Verdict::default();
    let result = run_over(
        &dir,
        OutputMode::Text,
        &verdict,
        &["rm", "--dest", dest.as_str(), "--dry-run"],
    );

    result.assert_success();
    assert_eq!(leaving(&result, &verdict), exit::OK);
    assert_eq!(
        result.stdout(),
        "would remove     bin/tool              (exec)\n\
         would remove     config/settings.toml\n\
         would remove     current\n"
    );
    assert!(dest.join("bin/tool").exists());
}

/// Forges the manifest so it records `paths` under the write's owner. Only a
/// hand-edited manifest holds keys like these; the library never writes one.
fn forge_manifest_keys(dest: &Utf8Path, borrowed: &str, paths: &[&str]) {
    let mut manifest = manifest_of(dest);
    let entry = manifest.entries[Utf8Path::new(borrowed)].clone();
    for path in paths {
        manifest
            .entries
            .insert(Utf8PathBuf::from(*path), entry.clone());
    }
    std::fs::write(
        dest.join(".proiectio/manifest.json").as_std_path(),
        serde_json::to_vec_pretty(&manifest).expect("a serialized manifest"),
    )
    .expect("a forged manifest");
}

/// The keys a forged manifest escapes the destination with.
const ESCAPING: [&str; 2] = ["../ESCAPE/x", "/etc/passwd"];

/// A manifest key outside the destination is a containment refusal in both
/// tenses — not a preview of a removal there.
#[test]
#[serial]
fn a_dry_run_of_rm_refuses_the_escaping_keys_the_real_run_refuses() {
    let (dir, dest, deploy) = tour();
    run(&dir, &["write", deploy.as_str(), "--dest", dest.as_str()]).assert_success();
    forge_manifest_keys(&dest, "bin/tool", &ESCAPING);

    let verdict = exit::Verdict::default();
    let dry = run_over(
        &dir,
        OutputMode::Text,
        &verdict,
        &["rm", "--dest", dest.as_str(), "--dry-run"],
    );

    assert_eq!(leaving(&dry, &verdict), exit::REFUSAL);
    assert_eq!(
        dry.stdout(),
        "would refuse     /etc/passwd           (containment)\n\
         would refuse     ../ESCAPE/x           (containment)\n\
         would remove     bin/tool              (exec)\n\
         would remove     config/settings.toml\n\
         would remove     current\n"
    );

    let real_verdict = exit::Verdict::default();
    let real = run_over(
        &dir,
        OutputMode::Text,
        &real_verdict,
        &["rm", "--dest", dest.as_str()],
    );

    assert_eq!(leaving(&real, &real_verdict), exit::REFUSAL);
    assert_eq!(real.stdout(), dry.stdout());
    assert_eq!(real.error(), None);
    // Neither run touched the destination: the real one refuses whole.
    assert!(dest.join("bin/tool").exists());
}

/// A hand-made symlink standing where recorded ancestry was puts the recorded
/// path out of reach; only grading the spelled ancestry reaches the refusal.
#[test]
#[serial]
fn rm_beneath_a_hand_made_link_refuses_on_the_dry_run_and_the_real_one() {
    let dir = TempDir::new().expect("a temporary directory");
    let root = utf8(&dir);
    let dest = root.join("dest");
    std::fs::create_dir(&dest).expect("a destination");
    let mapping = root.join("logs.toml");
    std::fs::write(
        mapping.as_std_path(),
        b"version = 1\n\n[files.\"logs/deep/file.txt\"]\ncontents = \"kept\\n\"\n",
    )
    .expect("a mapping projecting one nested file");
    run(&dir, &["write", mapping.as_str(), "--dest", dest.as_str()]).assert_success();
    std::fs::remove_dir_all(dest.join("logs").as_std_path()).expect("the recorded directory goes");
    std::os::unix::fs::symlink("real/missing", dest.join("logs").as_std_path())
        .expect("a hand-made link stands where it was");

    let dry_verdict = exit::Verdict::default();
    let dry = run_over(
        &dir,
        OutputMode::Text,
        &dry_verdict,
        &["rm", "--dest", dest.as_str(), "--dry-run"],
    );
    let real_verdict = exit::Verdict::default();
    let real = run_over(
        &dir,
        OutputMode::Text,
        &real_verdict,
        &["rm", "--dest", dest.as_str()],
    );

    assert_eq!(leaving(&dry, &dry_verdict), exit::REFUSAL);
    assert_eq!(leaving(&real, &real_verdict), exit::REFUSAL);
    assert_eq!(
        dry.stdout(),
        "would refuse     logs/deep/file.txt  (containment) (below the symlink logs)\n"
    );
    assert_eq!(real.stdout(), dry.stdout());
    assert_eq!(real.error(), None);
    assert!(dest.join("logs").symlink_metadata().is_ok());
}

#[test]
#[serial]
fn rm_of_a_hand_deleted_path_prunes_its_dirs_and_says_it_forgot_it() {
    let dir = TempDir::new().expect("a temporary directory");
    let root = utf8(&dir);
    let dest = root.join("dest");
    std::fs::create_dir(&dest).expect("a destination");
    let mapping = root.join("deep.toml");
    std::fs::write(
        mapping.as_std_path(),
        b"version = 1\n\n[files.\"only/deep/file.txt\"]\ncontents = \"x\\n\"\n",
    )
    .expect("a mapping projecting one nested file");

    run(&dir, &["write", mapping.as_str(), "--dest", dest.as_str()]).assert_success();
    std::fs::remove_file(dest.join("only/deep/file.txt").as_std_path())
        .expect("a hand-deleted file");

    let result = run(&dir, &["rm", "--dest", dest.as_str()]);

    result.assert_success();
    assert_eq!(
        result.stdout(),
        "forgot     only/deep/file.txt\n1 forgotten\n"
    );
    assert_eq!(
        entries(&dest),
        vec![".proiectio".to_owned()],
        "the directories the deleted path held open survived the removal"
    );
    assert!(manifest_of(&dest).entries.is_empty());
}

/// A named path the owner never recorded is reported as such rather than
/// disappearing into `nothing to do`; the run promises nothing about the path.
#[test]
#[serial]
fn rm_of_a_path_the_owner_never_recorded_says_so_and_still_succeeds() {
    let (dir, dest, deploy) = tour();
    run(&dir, &["write", deploy.as_str(), "--dest", dest.as_str()]).assert_success();

    let result = run(
        &dir,
        &[
            "rm",
            "typo.txt",
            "config/settings.toml",
            "--dest",
            dest.as_str(),
        ],
    );

    result.assert_success();
    assert_eq!(
        result.stdout(),
        "removed    config/settings.toml\n\
         no record  typo.txt\n\
         1 removed, 1 not recorded\n"
    );
    assert!(dest.join("bin/tool").exists());

    let json = run_as(
        &dir,
        OutputMode::Json,
        &["rm", "typo.txt", "--dest", dest.as_str()],
    );
    json.assert_success();
    let value: JsonValue = serde_json::from_str(json.stdout()).expect("a JSON document");
    assert_eq!(stated(&value["rows"], "typo.txt")["verdict"], "NotRecorded");
}

#[test]
#[serial]
fn rm_of_a_path_another_owner_holds_reports_it_and_leaves_it_alone() {
    let (dir, dest, deploy) = tour();
    run(
        &dir,
        &[
            "write",
            deploy.as_str(),
            "--dest",
            dest.as_str(),
            "--owner",
            "them",
        ],
    )
    .assert_success();

    let result = run(
        &dir,
        &[
            "rm",
            "config/settings.toml",
            "--dest",
            dest.as_str(),
            "--owner",
            "me",
        ],
    );

    result.assert_success();
    assert_eq!(
        result.stdout(),
        "no record  config/settings.toml\n1 not recorded\n"
    );
    assert!(dest.join("config/settings.toml").exists());
    assert_eq!(
        manifest_of(&dest).entries[Utf8Path::new("config/settings.toml")].owners,
        std::collections::BTreeSet::from(["them".to_owned()])
    );

    let json = run_as(
        &dir,
        OutputMode::Json,
        &[
            "rm",
            "config/settings.toml",
            "--dest",
            dest.as_str(),
            "--owner",
            "me",
        ],
    );
    json.assert_success();
    let value: JsonValue = serde_json::from_str(json.stdout()).expect("a JSON document");
    let row = stated(&value["rows"], "config/settings.toml");
    assert_eq!(row["verdict"], "NotRecorded");
    assert_eq!(row["facts"]["owners"], serde_json::json!(["them"]));
}

#[test]
#[serial]
fn a_refused_dry_run_of_rm_renders_the_whole_plan() {
    let (dir, dest, deploy) = tour();
    run(&dir, &["write", deploy.as_str(), "--dest", dest.as_str()]).assert_success();
    std::fs::write(dest.join("bin/tool").as_std_path(), EDITED).expect("a local edit");
    let argv = ["rm", "--dest", dest.as_str(), "--dry-run"];

    let verdict = exit::Verdict::default();
    let result = run_over(&dir, OutputMode::Text, &verdict, &argv);

    assert_eq!(leaving(&result, &verdict), exit::REFUSAL);
    assert_eq!(
        result.stdout(),
        "would refuse     bin/tool              (drifted)\n\
         would remove     config/settings.toml\n\
         would remove     current\n\
         pass --force to touch them anyway, where the projection can still \
         tell what it would replace\n"
    );
    assert_eq!(result.error(), None);
    assert!(dest.join("bin/tool").exists());

    let structured = exit::Verdict::default();
    let json = run_over(&dir, OutputMode::Json, &structured, &argv);

    assert_eq!(leaving(&json, &structured), exit::REFUSAL);
    let value: JsonValue = serde_json::from_str(json.stdout()).expect("a JSON document");
    assert_eq!(
        stated(&value["rows"], "bin/tool")["verdict"]["Refuse"]["refusal"],
        "Drift"
    );
    assert_eq!(
        stated(&value["rows"], "config/settings.toml")["verdict"],
        "Remove"
    );
}

#[test]
#[serial]
fn rm_of_an_owner_holding_nothing_succeeds() {
    let dir = TempDir::new().expect("a temporary directory");
    let dest = utf8(&dir);

    let result = run(&dir, &["rm", "--dest", dest.as_str()]);

    result.assert_success();
    assert_eq!(result.stdout(), "nothing to do\n");
}

#[test]
#[serial]
fn rm_publishes_the_librarys_own_rows() {
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
    assert_eq!(planned["phase"], "planned");
    assert_eq!(real["phase"], "applied");
    assert_eq!(stated(&planned["rows"], "bin/tool")["verdict"], "Remove");
    assert_eq!(stated(&real["rows"], "bin/tool")["verdict"], "Removed");
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

/// What both the dry and the real run print for the refusal: a run refused
/// before it acted renders the plan it declined, in the plan's own tense.
const REFUSED_LANDING: &str = "would refuse     a/x.txt  (recorded landing) \
                               (through the symlink a, onto real/x.txt, held by p)";

/// The #137 transcript: two owners, one hand deletion, no manifest editing.
#[test]
#[serial]
fn rm_refuses_where_a_recorded_link_resolves_onto_another_owners_node() {
    let dir = TempDir::new().expect("a temporary directory");
    let root = utf8(&dir);
    let dest = root.join("dest");
    std::fs::create_dir(&dest).expect("a destination");

    let held = root.join("o.toml");
    std::fs::write(
        &held,
        b"version = 1\n\n[files.\"a/x.txt\"]\ncontents = \"kept\\n\"\n",
    )
    .expect("the mapping the first owner projects");
    let theirs = root.join("p.toml");
    std::fs::write(
        &theirs,
        b"version = 1\n\n[files.\"real/x.txt\"]\ncontents = \"kept\\n\"\n",
    )
    .expect("the mapping the second owner projects");
    let pivoted = root.join("p-linked.toml");
    std::fs::write(
        &pivoted,
        b"version = 1\n\n\
          [files.\"real/x.txt\"]\ncontents = \"kept\\n\"\n\n\
          [links.\"a\"]\ntarget = \"real\"\n",
    )
    .expect("the mapping that plants the link");

    for (mapping, owner) in [(&held, "o"), (&theirs, "p")] {
        run(
            &dir,
            &[
                "write",
                mapping.as_str(),
                "--dest",
                dest.as_str(),
                "--owner",
                owner,
            ],
        )
        .assert_success();
    }
    std::fs::remove_dir_all(dest.join("a").as_std_path()).expect("the hand deletion");
    run(
        &dir,
        &[
            "write",
            pivoted.as_str(),
            "--dest",
            dest.as_str(),
            "--owner",
            "p",
        ],
    )
    .assert_success();

    for extra in [vec!["--dry-run"], vec![]] {
        let mut argv = vec!["rm", "--dest", dest.as_str(), "--owner", "o"];
        argv.extend(extra);
        let verdict = exit::Verdict::default();
        let result = run_over(&dir, OutputMode::Text, &verdict, &argv);

        assert_eq!(leaving(&result, &verdict), exit::REFUSAL);
        assert_eq!(result.stdout(), format!("{REFUSED_LANDING}\n"), "{argv:?}");
        assert_eq!(
            std::fs::read(dest.join("real/x.txt").as_std_path()).expect("the node p records"),
            b"kept\n"
        );
        assert!(dest.join("real").is_dir(), "pruning took p's directory");
    }

    assert_eq!(
        manifest_of(&dest).entries[Utf8Path::new("real/x.txt")].owners,
        std::collections::BTreeSet::from(["p".to_owned()])
    );
    assert_eq!(
        manifest_of(&dest).entries[Utf8Path::new("a/x.txt")].owners,
        std::collections::BTreeSet::from(["o".to_owned()])
    );
}
