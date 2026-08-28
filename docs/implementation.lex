Implementation Guidelines

    Proiectio is *entirely* about side effects. Unmanaged, that makes
    a codebase untestable, hence unreliable, hence bug-ridden as time
    goes on. The points below are how we manage it.

1. Three Stages: Observe, Decide, Act

    Separate the logic from the file system — but honestly: planning
    is not pure by itself, because classification needs disk reads,
    hashing recorded paths to detect drift. So the engine is three
    stages, not two:

    - observe: read-only I/O. Walk the destination, hash recorded
      paths, snapshot into plain data.
    - decide: pure. (desired, manifest, observations) -> Plan,
      deterministic, no file system. All the interesting logic —
      classification, drift, containment, orphans — lives here.
    - act: write I/O. Execute the plan verbatim.

    Dry runs are observe + decide with act skipped; removal is decide
    against an empty desired tree. The engine enforces the split: act
    takes only a Plan, never the mapping or the disk directly.

    One deliberate exception to "verbatim": before each destructive
    action — overwrite or removal — act re-hashes the target and
    refuses if it changed since observation. plan and apply are
    separate calls in the library, so the gap between them is real
    even where the CLI closes it to milliseconds.

2. Testing

    The split above does most of the work: decide is table-testable
    with no file system — most scenarios are plain data in, Plan
    out. File-system tests target observe and act specifically,
    since atomic rename and symlink refusal are the behavior under
    test.

    For those, tempfile::TempDir gives isolation and teardown by
    RAII — each test owns its directory, drop cleans it up, no
    shared state, no ordering. The one rule that needs enforcing:
    tests never touch the current directory. cargo test runs tests
    in parallel threads and set_current_dir is process-global, so
    everything takes absolute paths from the fixture.

    A small tree-declaration helper — paths, contents, links in one
    expression — keeps scenarios readable; it is a few dozen lines,
    not a dependency. The CLI layer gets its own harness later
    (section 4); the library never waits on it.

3. Centralized Path Resolution

    Every untrusted path enters through one function:

    contained_join(dest, rel) -> Result<Utf8PathBuf>

    :: rust ::

    the sole gateway enforcing the containment rules of
    [./security.lex] section 2 — that section's refusal list, not
    any paraphrase elsewhere, is the contract. Crates are
    implementation details inside it [1], and
    two constraints bound the choice:

    - Containment wants *lexical* normalization — never
      std::fs::canonicalize, which follows symlinks and requires
      paths to exist.
    - The check that matters most — no symlinked ancestor at write
      time — is ours to enforce regardless of crate.

    Worth a spike before act is written: cap-std, whose Dir handle
    rooted at dest makes the OS itself refuse ".." and symlink
    escapes at open time — kernel-enforced containment for observe
    and act, closing the plan-to-apply race structurally rather than
    by string checks. The open question is how it coexists with
    --allow-external-targets, where we deliberately create a link
    pointing out of dest.

4. Standout

    The CLI is built on the Standout framework, which enforces the
    split this project already wants: a CLI-free library — the
    projection engine, pure data types in and out — and a binary
    package that owns Clap, Standout, handlers, view models,
    templates and output. Handlers stay thin: translate CLI input
    into library calls and library results into serializable view
    models; nothing prints or renders from the core.

    Corollary: the library comes first, and the CLI is tackled after,
    as adapters. Read the standout skill
    ([../.agents/skills/standout/SKILL.md]) before writing that
    layer.

    To make the adapter layer trivial, the library's error type is a
    structured enum (thiserror), with refusals — drift, foreign,
    containment, external target, each variant carrying the
    offending paths — distinct from I/O errors. The CLI's 0/1/2 exit
    mapping ([./cli-tour.lex]) then falls out of a single match, and
    messages list paths instead of formatting strings deep in the
    core.

5. Error Handling

    The pure stages validate early, but runtime errors mid-apply
    cannot be prevented — disk full, permissions changed, trees
    deleted underneath us. The rules are about honesty, not
    prevention:

    - Never swallow an error. Friendlier messages are fine; changing
      an error's nature or semantics is not. The underlying OS error
      stays visible in the message; the exit code stays on the 0/1/2
      contract of [./cli-tour.lex].
    - No recovery, no rollback. The first error aborts the run;
      per-file atomicity — a tempfile persisted over the path —
      already guarantees no torn file exists.
    - But the manifest reflects reality, not success. If act dies
      halfway, the entries already applied are persisted to the
      manifest before the error returns — otherwise those files
      classify as Foreign on the next run and the destination is
      wedged forever behind its own safety rule. A failed run must
      leave a destination a re-run can heal.
    - "No cleanup" does not cover our own droppings: a failed
      persist removes its tempfile (the tempfile crate does this on
      drop), so dest is never littered with .tmp files.

6. Determinism and Ordering

    All collections are BTreeMap/BTreeSet ([./design.lex]); act
    executes in sorted order, parents before children, removals in
    reverse. Plans are diffable, output is stable, failures are
    reproducible.

7. Concurrency

    Two processes applying to one destination corrupt the manifest's
    read-modify-write. A single-writer lock file in the state dir
    (fd-lock or equivalent) closes it in a few lines. The harness
    use case makes concurrent invocations plausible enough to decide
    this now rather than discover it later.

Notes:

[1] Candidate crates: path-absolutize and normpath (lexical
    normalization, dot-component cleanup without canonicalizing);
    relative-path (portable, strictly relative path types); camino
    (UTF-8 paths, already in the design); strict-path (validates
    untrusted paths against traversal and symlink escape — young,
    evaluate rather than assume); cap-std (capability-based Dir
    rooted at dest, OS-enforced — see the spike in section 3).
