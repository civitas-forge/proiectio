// The dry/real parity harness. `docs/implementation.lex` section 1 lets
// applying refuse only what the disk did after the observation; every case
// here builds a destination, decides against it, touches nothing, and applies,
// so any refusal apply raises that the plan did not is this file's failure.
//
// The table covers the refusal families act owns — `validate`'s whole-plan
// check, `verified_parent`'s no-follow ancestor walk, and `check_expected`'s
// signature re-check — with the scenario of issue #116 as its first row.

use std::collections::BTreeMap;
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};

use crate::test_support::Tree;
use crate::{
    ApplyOutcome, Desired, DriftPolicy, Error, PlanOptions, PlannedAction, Projection, Refusal,
    RemovalScope,
};

// The owner every case projects under.
const OWNER: &str = "site";

// The run whose dry verdicts and real outcomes have to agree.
enum Op {
    // Project this tree under `OWNER`.
    Write(Tree),
    // Remove everything `OWNER` holds.
    Remove,
    // Remove the recorded paths this list names.
    RemovePaths(&'static [&'static str]),
}

// One parity case.
struct Case {
    // What the case is about, named by every assertion it fails.
    what: &'static str,
    // Projected before the run under test, one pass per owner, which is what
    // puts the paths in the manifest.
    recorded: Vec<(&'static str, Tree)>,
    // Run against the destination after those passes: the hand edit whose
    // effect on the two verdicts the case is about.
    by_hand: fn(&Utf8Path),
    op: Op,
    drift: DriftPolicy,
    // Every path the plan states, with the verdict it reaches. The apply
    // outcome each verdict predicts is `outcome_of`'s.
    plans: Vec<(&'static str, PlannedAction)>,
}

impl Case {
    fn new(what: &'static str, op: Op) -> Case {
        Case {
            what,
            recorded: Vec::new(),
            by_hand: |_| {},
            op,
            drift: DriftPolicy::Refuse,
            plans: Vec::new(),
        }
    }

    fn recording(mut self, owner: &'static str, tree: Tree) -> Case {
        self.recorded.push((owner, tree));
        self
    }

    fn by_hand(mut self, by_hand: fn(&Utf8Path)) -> Case {
        self.by_hand = by_hand;
        self
    }

    fn forcing(mut self) -> Case {
        self.drift = DriftPolicy::Overwrite;
        self
    }

    fn plans(mut self, path: &'static str, verdict: PlannedAction) -> Case {
        self.plans.push((path, verdict));
        self
    }

    fn refuses(self, path: &'static str, refusal: Refusal) -> Case {
        self.plans(path, PlannedAction::Refuse { refusal })
    }
}

// The outcome one planned verdict predicts of applying.
fn outcome_of(verdict: &PlannedAction) -> ApplyOutcome {
    match verdict {
        PlannedAction::Write => ApplyOutcome::Written,
        PlannedAction::Overwrite { .. } => ApplyOutcome::Overwritten,
        PlannedAction::Skip => ApplyOutcome::Skipped,
        PlannedAction::Remove => ApplyOutcome::Removed,
        PlannedAction::Forget => ApplyOutcome::Forgot,
        PlannedAction::Release => ApplyOutcome::Released,
        PlannedAction::NotRecorded => ApplyOutcome::NotRecorded,
        PlannedAction::Refuse { .. } => {
            unreachable!("a plan carrying a refusal never reaches an outcome")
        }
    }
}

fn containment(link: &str) -> Refusal {
    Refusal::Containment {
        through: Some(Utf8PathBuf::from(link)),
    }
}

// Runs one case: the recording passes, the hand edit, then one decide and one
// apply with nothing in between.
fn parity(case: &Case) {
    let what = case.what;
    let dest = Tree::new().materialize();
    let state = Tree::new().materialize();
    let projection =
        Projection::new(dest.root(), Some(state.root())).expect("a projection over the fixtures");

    for (owner, tree) in &case.recorded {
        let mut run = projection.begin().expect("begin a recording pass");
        run.plan(
            owner,
            &Desired::from_caller(tree.entries()),
            PlanOptions::default(),
        )
        .unwrap_or_else(|error| panic!("{what}: deciding the pass for {owner}: {error}"));
        run.apply()
            .unwrap_or_else(|aborted| panic!("{what}: the pass for {owner}: {aborted}"));
    }

    (case.by_hand)(dest.root());

    let mut run = projection.begin().expect("begin the run under test");
    let options = PlanOptions {
        drift: case.drift,
        ..PlanOptions::default()
    };
    let requested;
    let plan = match &case.op {
        Op::Write(tree) => run.plan(OWNER, &Desired::from_caller(tree.entries()), options),
        Op::Remove => run.plan_removal(OWNER, RemovalScope::Everything, case.drift),
        Op::RemovePaths(paths) => {
            requested = paths.iter().map(Utf8PathBuf::from).collect();
            run.plan_removal(OWNER, RemovalScope::Paths(&requested), case.drift)
        }
    }
    .unwrap_or_else(|error| panic!("{what}: deciding: {error}"))
    .clone();
    let manifest = run.manifest().clone();

    let planned: BTreeMap<Utf8PathBuf, PlannedAction> = plan
        .report(&manifest)
        .rows
        .into_iter()
        .map(|(path, row)| (path, row.verdict))
        .collect();
    let stated: BTreeMap<Utf8PathBuf, PlannedAction> = case
        .plans
        .iter()
        .map(|(path, verdict)| (Utf8PathBuf::from(*path), verdict.clone()))
        .collect();
    assert_eq!(planned, stated, "{what}: what the dry run says");

    match run.apply() {
        Ok(report) => {
            let predicted = plan.refused();
            assert!(
                predicted.is_none(),
                "{what}: applying carried out a plan that refused {}",
                predicted.expect("just checked"),
            );
            let applied: BTreeMap<Utf8PathBuf, ApplyOutcome> = report
                .report
                .rows
                .into_iter()
                .map(|(path, row)| (path, row.verdict))
                .collect();
            let expected: BTreeMap<Utf8PathBuf, ApplyOutcome> = planned
                .iter()
                .map(|(path, verdict)| (path.clone(), outcome_of(verdict)))
                .collect();
            assert_eq!(applied, expected, "{what}: what the real run did");
        }
        Err(aborted) => {
            let Error::Refused(refused) = aborted.stopped.error() else {
                panic!(
                    "{what}: applying failed rather than refused: {}",
                    aborted.stopped.error()
                );
            };
            let predicted = plan.refused().unwrap_or_else(|| {
                panic!(
                    "{what}: applying refused what the dry run did not predict — nothing touched \
                     the disk between them, so this refusal was the snapshot's to make: {refused}"
                )
            });
            assert_eq!(
                refused.paths(),
                predicted.paths(),
                "{what}: the refusal the dry run predicted"
            );
            assert!(
                !aborted.applied_anything(),
                "{what}: a run that refused whole still touched the destination"
            );
        }
    }
}

#[test]
fn dry_and_real_runs_reach_the_same_verdict() {
    for case in cases() {
        parity(&case);
    }
}

fn cases() -> Vec<Case> {
    vec![
        // --- act's no-follow ancestor walk (`verified_parent`) ---
        //
        // Issue #116: observation never descends the hand-made link, so the
        // recorded path beneath it reads absent. Grading the ancestry the
        // path is spelled of is what lets the dry run refuse where the real
        // run does.
        Case::new("a removal beneath a hand-made link", Op::Remove)
            .recording(OWNER, Tree::new().file("logs/deep/file.txt", "kept\n"))
            .by_hand(|root| {
                fs::remove_dir_all(root.join("logs")).expect("remove the recorded directory");
                std::os::unix::fs::symlink("real/missing", root.join("logs"))
                    .expect("plant the hand-made link");
            })
            .refuses("logs/deep/file.txt", containment("logs")),
        Case::new(
            "a write beneath a hand-made link",
            Op::Write(Tree::new().file("logs/deep/file.txt", "kept\n")),
        )
        .recording(OWNER, Tree::new().file("logs/deep/file.txt", "kept\n"))
        .by_hand(|root| {
            fs::remove_dir_all(root.join("logs")).expect("remove the recorded directory");
            std::os::unix::fs::symlink("real/missing", root.join("logs"))
                .expect("plant the hand-made link");
        })
        .refuses("logs/deep/file.txt", containment("logs")),
        // A recorded link the walk would follow, whose target was edited: the
        // walk refuses drift at the link rather than resolving through it.
        Case::new(
            "a write beneath a recorded link whose target moved",
            Op::Write(
                Tree::new()
                    .file("real/keep.txt", "kept\n")
                    .file("pivot/new.txt", "fresh\n"),
            ),
        )
        .recording("other", Tree::new().symlink("pivot", "real"))
        .recording(OWNER, Tree::new().file("real/keep.txt", "kept\n"))
        .by_hand(|root| {
            fs::remove_file(root.join("pivot")).expect("remove the recorded link");
            fs::create_dir(root.join("other")).expect("the directory the link is moved to");
            std::os::unix::fs::symlink("other", root.join("pivot")).expect("re-plant the link");
        })
        .plans("real/keep.txt", PlannedAction::Skip)
        .refuses("pivot/new.txt", Refusal::Drift),
        // The same, forced. Nothing in the ancestor walk consults the drift
        // policy, so the two runs agree under `--force` as they do without it.
        Case::new(
            "a forced write beneath a recorded link whose target moved",
            Op::Write(
                Tree::new()
                    .file("real/keep.txt", "kept\n")
                    .file("pivot/new.txt", "fresh\n"),
            ),
        )
        .recording("other", Tree::new().symlink("pivot", "real"))
        .recording(OWNER, Tree::new().file("real/keep.txt", "kept\n"))
        .by_hand(|root| {
            fs::remove_file(root.join("pivot")).expect("remove the recorded link");
            fs::create_dir(root.join("other")).expect("the directory the link is moved to");
            std::os::unix::fs::symlink("other", root.join("pivot")).expect("re-plant the link");
        })
        .forcing()
        .plans("real/keep.txt", PlannedAction::Skip)
        .refuses("pivot/new.txt", Refusal::Drift),
        // An ancestor that is a node and not a directory: the walk refuses it
        // as what the manifest says it is.
        Case::new(
            "a write beneath a hand-made file",
            Op::Write(Tree::new().file("conf/rc", "settings\n")),
        )
        .by_hand(|root| {
            Tree::new()
                .file("conf", "somebody else's\n")
                .write_under(root);
        })
        .refuses("conf/rc", Refusal::Foreign),
        Case::new(
            "a write beneath another owner's recorded file",
            Op::Write(Tree::new().file("conf/rc", "settings\n")),
        )
        .recording("other", Tree::new().file("conf", "theirs\n"))
        .refuses("conf/rc", Refusal::Drift),
        // The same shape one owner projects whole: the run's own removal
        // clears the file out of the way before the write walks through it.
        Case::new(
            "a write beneath a file the same run removes",
            Op::Write(Tree::new().file("conf/rc", "settings\n")),
        )
        .recording(OWNER, Tree::new().file("conf", "mine\n"))
        .plans("conf", PlannedAction::Remove)
        .plans("conf/rc", PlannedAction::Write),
        // --- act's signature re-check (`check_expected`) ---
        //
        // Nothing moved between the plan and the apply, so every re-check
        // passes and each verdict comes out as the outcome it predicted.
        Case::new(
            "a projection over paths it already holds",
            Op::Write(
                Tree::new()
                    .file("a.txt", "two\n")
                    .executable("bin/tool", "#!/bin/sh\n")
                    .symlink("rc", "a.txt"),
            ),
        )
        .recording(
            OWNER,
            Tree::new()
                .file("a.txt", "one\n")
                .executable("bin/tool", "#!/bin/sh\n")
                .symlink("rc", "a.txt"),
        )
        .plans(
            "a.txt",
            PlannedAction::Overwrite {
                reason: crate::OverwriteReason::ContentChanged,
            },
        )
        .plans("bin/tool", PlannedAction::Skip)
        .plans("rc", PlannedAction::Skip),
        Case::new("a removal over a path a hand already deleted", Op::Remove)
            .recording(
                OWNER,
                Tree::new()
                    .file("a.txt", "one\n")
                    .file("deep/b.txt", "two\n"),
            )
            .by_hand(|root| {
                fs::remove_file(root.join("deep/b.txt")).expect("delete the recorded path");
            })
            .plans("a.txt", PlannedAction::Remove)
            .plans("deep/b.txt", PlannedAction::Forget),
        Case::new(
            "a forced overwrite over an edit the plan measured",
            Op::Write(Tree::new().file("a.txt", "two\n")),
        )
        .recording(OWNER, Tree::new().file("a.txt", "one\n"))
        .by_hand(|root| {
            Tree::new().file("a.txt", "edited\n").write_under(root);
        })
        .forcing()
        .plans(
            "a.txt",
            PlannedAction::Overwrite {
                reason: crate::OverwriteReason::ForcedDrift,
            },
        ),
        // --- act's whole-plan check (`validate`) ---
        //
        // A plan carrying a refusal is declined whole and writes nothing, so
        // the keys the dry run named are the keys the real run names.
        Case::new(
            "a projection over an edit no force lifts",
            Op::Write(Tree::new().file("a.txt", "two\n")),
        )
        .recording(OWNER, Tree::new().file("a.txt", "one\n"))
        .by_hand(|root| {
            Tree::new().file("a.txt", "edited\n").write_under(root);
        })
        .refuses("a.txt", Refusal::Drift),
        Case::new(
            "a projection over a path nothing records",
            Op::Write(Tree::new().file("a.txt", "two\n")),
        )
        .by_hand(|root| {
            Tree::new()
                .file("a.txt", "somebody else's\n")
                .write_under(root);
        })
        .refuses("a.txt", Refusal::Foreign),
        // --- the verdicts that touch no node ---
        Case::new("a removal over a path a second owner holds", Op::Remove)
            .recording(OWNER, Tree::new().file("shared.txt", "both\n"))
            .recording("other", Tree::new().file("shared.txt", "both\n"))
            .plans("shared.txt", PlannedAction::Release),
        Case::new(
            "a removal naming a path this owner never held",
            Op::RemovePaths(&["a.txt", "theirs.txt"]),
        )
        .recording(OWNER, Tree::new().file("a.txt", "one\n"))
        .recording("other", Tree::new().file("theirs.txt", "two\n"))
        .plans("a.txt", PlannedAction::Remove)
        .plans("theirs.txt", PlannedAction::NotRecorded),
    ]
}
