// The dry/real parity harness. `docs/implementation.lex` section 1 lets
// applying refuse only what the disk did after the observation; every case
// here builds a destination, decides against it, touches nothing, and applies,
// so any refusal apply raises that the plan did not is this file's failure.
//
// The table covers the refusal families act owns — `validate`'s whole-plan
// check, `verified_parent`'s no-follow ancestor walk, and `check_expected`'s
// signature re-check — with the scenario of issue #116 as its first row.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};

use crate::test_support::Tree;
use crate::{
    ApplyOutcome, Desired, DriftPolicy, Entry, Error, Placement, PlanOptions, PlannedAction,
    Projection, Refusal, RemovalScope,
};

// The owner every case projects under.
const OWNER: &str = "site";

// The marker the block cases splice under.
const MARKER: &str = "# proiectio";

// A desired tree of one block region at `path`, which no `Tree` declares.
fn block_at(path: &'static str, body: &'static str) -> BTreeMap<Utf8PathBuf, Entry> {
    BTreeMap::from([(
        Utf8PathBuf::from(path),
        Entry::Block {
            body: body.as_bytes().to_vec(),
            marker: MARKER.to_owned(),
            placement: Placement::Append,
        },
    )])
}

// The run whose dry verdicts and real outcomes have to agree.
enum Op {
    // Project this tree under `OWNER`.
    Write(Tree),
    // Remove everything `OWNER` holds.
    Remove,
    // Remove the recorded paths this list names.
    RemovePaths(&'static [&'static str]),
}

// One step of the setup a case records before the run under test.
enum Setup {
    // One projection pass, which is what puts the paths in the manifest.
    Record(&'static str, Tree),
    // The same over entries a `Tree` cannot declare, which is the blocks.
    RecordEntries(&'static str, BTreeMap<Utf8PathBuf, Entry>),
    // A hand edit between two passes, for a manifest no sequence of passes
    // reaches on its own.
    Hand(fn(&Utf8Path)),
}

// One parity case.
struct Case {
    // What the case is about, named by every assertion it fails.
    what: &'static str,
    // Run before the run under test, in order.
    recorded: Vec<Setup>,
    // Run against the destination after those steps: the hand edit whose
    // effect on the two verdicts the case is about.
    by_hand: fn(&Utf8Path),
    // Whether the state directory lies inside the destination, which is what
    // gives the projection a state prefix to refuse paths against.
    state_in_dest: bool,
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
            state_in_dest: false,
            op,
            drift: DriftPolicy::Refuse,
            plans: Vec::new(),
        }
    }

    fn recording(mut self, owner: &'static str, tree: Tree) -> Case {
        self.recorded.push(Setup::Record(owner, tree));
        self
    }

    fn recording_entries(
        mut self,
        owner: &'static str,
        entries: BTreeMap<Utf8PathBuf, Entry>,
    ) -> Case {
        self.recorded.push(Setup::RecordEntries(owner, entries));
        self
    }

    fn then_by_hand(mut self, edit: fn(&Utf8Path)) -> Case {
        self.recorded.push(Setup::Hand(edit));
        self
    }

    fn with_state_in_dest(mut self) -> Case {
        self.state_in_dest = true;
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
    let state_dir = (!case.state_in_dest).then(|| state.root());
    let projection =
        Projection::new(dest.root(), state_dir).expect("a projection over the fixtures");

    for step in &case.recorded {
        let (owner, entries) = match step {
            Setup::Record(owner, tree) => (owner, tree.entries()),
            Setup::RecordEntries(owner, entries) => (owner, entries.clone()),
            Setup::Hand(edit) => {
                edit(dest.root());
                continue;
            }
        };
        let mut run = projection.begin().expect("begin a recording pass");
        run.plan(
            owner,
            &Desired::from_caller(entries),
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
        // A recorded link the walk follows out to a node another action of
        // the same plan claims. Applying runs the removal first, so the skip
        // would meet a path the run had just deleted; both keys refuse
        // instead (issue #129).
        Case::new(
            "a removal landing where a desired path stands",
            Op::Write(Tree::new().file("real/x.txt", "kept\n")),
        )
        .recording(
            OWNER,
            Tree::new()
                .file("logs/x.txt", "kept\n")
                .file("real/x.txt", "kept\n"),
        )
        .then_by_hand(|root| {
            fs::remove_dir_all(root.join("logs")).expect("remove the recorded directory");
        })
        .recording("other", Tree::new().symlink("logs", "real"))
        .refuses(
            "logs/x.txt",
            Refusal::TreeConflict {
                paths: BTreeSet::from([Utf8PathBuf::from("real/x.txt")]),
            },
        )
        .refuses(
            "real/x.txt",
            Refusal::TreeConflict {
                paths: BTreeSet::from([Utf8PathBuf::from("logs/x.txt")]),
            },
        ),
        // The landing, not the key, is what the run leaves empty: the write
        // walks through `real/x`, which the removal of `logs/x` unlinks. Keyed
        // by `logs/x` instead, the walk met the old file still standing there
        // and refused it as Foreign, which applying does not do.
        Case::new(
            "a write walking through what a removal vacates elsewhere",
            Op::Write(Tree::new().file("real/x/child.txt", "fresh\n")),
        )
        .recording(OWNER, Tree::new().file("logs/x", "old\n"))
        .then_by_hand(|root| {
            fs::rename(root.join("logs"), root.join("real")).expect("move the directory aside");
        })
        .recording("other", Tree::new().symlink("logs", "real"))
        .plans("logs/x", PlannedAction::Remove)
        .plans("real/x/child.txt", PlannedAction::Write),
        // A removal expecting nothing unlinks nothing, so the location it
        // verifies empty is free for a desired key to write: forgetting the
        // record and writing the path are one run, not two claims on one node.
        Case::new(
            "a write where an absence-only removal lands",
            Op::Write(Tree::new().file("real/x", "fresh\n")),
        )
        .recording(OWNER, Tree::new().file("logs/x", "old\n"))
        .then_by_hand(|root| {
            fs::rename(root.join("logs"), root.join("real")).expect("move the directory aside");
            fs::remove_file(root.join("real/x")).expect("delete the recorded path by hand");
        })
        .recording("other", Tree::new().symlink("logs", "real"))
        .plans("logs/x", PlannedAction::Forget)
        .plans("real/x", PlannedAction::Write),
        // A desired path standing on a directory this run empties from
        // elsewhere: the only node `real` holds is the one the removal keyed
        // `logs/x` unlinks, so pruning takes the directory and the write has
        // the location. Read off the keys, the directory looked held.
        Case::new(
            "a write over a directory a removal empties through a link",
            Op::Write(Tree::new().file("real", "fresh\n")),
        )
        .recording(OWNER, Tree::new().file("logs/x", "old\n"))
        .then_by_hand(|root| {
            fs::rename(root.join("logs"), root.join("real")).expect("move the directory aside");
        })
        .recording("other", Tree::new().symlink("logs", "real"))
        .plans("logs/x", PlannedAction::Remove)
        .plans("real", PlannedAction::Write),
        // A block whose container the walk reaches through a recorded link.
        // Observation parses a region under the manifest key standing at the
        // path it walks, so the container reads as an ordinary file where it
        // actually sits; grading the record against that file made every such
        // removal drift, while applying strips the region and succeeds.
        Case::new(
            "a block removal landing beneath a recorded link",
            Op::Remove,
        )
        .then_by_hand(|root| {
            Tree::new().file("logs/x", "author\n").write_under(root);
        })
        .recording_entries(OWNER, block_at("logs/x", "managed\n"))
        .then_by_hand(|root| {
            fs::rename(root.join("logs"), root.join("real")).expect("move the container aside");
        })
        .recording("other", Tree::new().symlink("logs", "real"))
        .plans("logs/x", PlannedAction::Remove),
        // The same walk, landing inside the state subtree. Before deciding
        // graded the landing, the plan aimed a removal at the state file and
        // applying deleted it: act knows no state prefix, so nothing behind
        // deciding would have stopped it (issue #129).
        Case::new("a removal landing inside the state subtree", Op::Remove)
            .with_state_in_dest()
            .recording(OWNER, Tree::new().file("logs/private-state", "secret\n"))
            .then_by_hand(|root| {
                fs::remove_dir_all(root.join("logs")).expect("remove the recorded directory");
                fs::write(root.join(".proiectio/private-state"), "secret\n")
                    .expect("plant the state file the link points at");
            })
            .recording("other", Tree::new().symlink("logs", ".proiectio"))
            .refuses("logs/private-state", containment("logs")),
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
