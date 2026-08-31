Implementation Guidelines

    Proiectio is *entirely* about side effects. Unmanaged, that makes
    a codebase untestable, hence unreliable, hence bug-ridden as time
    goes on. The points below are how we manage it.

1. Three Stages: Observe, Decide, Act

    Separate the logic from the file system — but honestly: planning
    is not pure by itself, because classification needs disk reads,
    hashing what is on disk to detect drift. So the engine is three
    stages, not two:

    - observe: read-only I/O. Walk the destination, hash every file
      it can name — it never sees the desired tree, so it cannot
      know which paths decide will compare — snapshot into plain
      data.
    - decide: pure. (desired, manifest, observations) -> Plan,
      deterministic, no file system. All the interesting logic —
      classification, drift, containment, orphans — lives here.
    - act: write I/O. Execute the plan verbatim.

    Dry runs are observe + decide with act skipped; removal is decide
    against an empty desired tree, over everything the owner holds or
    over the recorded paths a caller names ([./design.lex] sections 2
    and 3). The engine enforces the split: act takes only a Plan,
    never the mapping or the disk directly.

    One deliberate exception to "verbatim": before each destructive
    action — overwrite or removal — act re-checks the target against
    the signature the plan expects — kind, hash, executable bit — and
    refuses if it changed since observation. A link's target grading
    is re-checked the same way, against the disk rather than the
    snapshot it was decided from ([./security.lex] section 3). plan
    and apply are separate calls in the library, so the gap between
    them is real even where the CLI closes it to milliseconds.

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

    Every untrusted path enters through one gateway:

    contained_normalize(rel) -> Option<Utf8PathBuf>

    :: rust ::

    which normalizes lexically and answers None for a spelling the
    contract refuses — decide admits desired keys through it, since a
    Plan keys actions relative to the destination.
    [./security.lex] section 2 owns the refusal list. Two constraints
    bound the implementation:

    - Containment wants *lexical* normalization — never
      std::fs::canonicalize, which follows symlinks and requires
      paths to exist. clippy.toml bans it crate-wide.
    - The check that matters most — no symlinked ancestor at write
      time — is ours to enforce regardless of crate.

    Spiked and decided: adopt cap-std. Observe and act do their I/O
    through a cap_std::fs_utf8::Dir rooted at dest, and every open
    refuses a path whose resolution escapes that root — an escaping
    "..", an absolute path, a symlink chain leaving the Dir. An
    in-dest ".." still resolves: the refusal is about where a path
    lands, not how it is spelled. Escapes fail whenever a hostile
    link appears, so the plan-to-apply race is closed structurally
    rather than by string checks.

    cap-std does not close the other half — an in-dest symlinked
    ancestor is followed silently — so act walks a write's ancestors
    itself. Each component is opened with cap-primitives'
    open_dir_nofollow from the previously verified handle, and the
    final mutation happens relative to that parent, so a component
    swapped for a symlink after the check cannot redirect a write.
    When a no-follow open reports a symlink, act matches it against
    the manifest:

    - not recorded — the Containment refusal;
    - recorded, but the on-disk target no longer hashes to the
      recorded string — Drift, like any stale plan (section 1);
    - recorded and matching, but graded external — refused; an
      external target is never written through;
    - recorded, matching, and in-dest — act reads the target through
      the parent handle, resolves it with contained_target, and
      restarts the walk from the dest root along the resolved path.
      Restarts carry a visited set, and meeting any link twice
      refuses rather than resolving further: a chain that walks
      one link twice ends outside, as a loop does.

    What a restart earns depends on the action, and the three answers
    differ:

    - A write — a file, a symlink, or a block's container — goes
      down at its action key or nowhere. The key is the path the
      manifest records, so a write landing elsewhere puts bytes at
      one path and the record at another. No later run heals that:
      observation never descends a link, so the key reads Missing,
      the write is planned again, and deciding refuses it under the
      no-alias rule. A walk that relocated a write is therefore the
      Containment refusal.
    - A removal follows the link. Where the manifest records the
      location the walk comes out at, the removal refuses as
      RecordedLanding, naming the link, the landing and its owners;
      deciding and applying both grade this, each against the
      manifest as it loaded. A removal expecting nothing drops its
      record without grading the landing. Otherwise the removal
      unlinks through the resolved location and reports it, so
      pruning judges the directory that actually lost a child.
    - A release walks nothing and reads no disk. It drops one owner
      from a manifest entry, and deciding plans it over a shared
      path clean, drifted or missing alike, so a disk check would
      refuse an owner's departure over a node it is not touching.

    One boundary the handles do not close: a directory handle follows
    its object, so a process renaming a verified ancestor out of dest
    carries the handle with it. That actor holds invoker-level write
    access and is trusted by the [./security.lex] split; the claim
    here is about hostile content, not about a concurrently mutating
    privileged process, which section 7's lock rules out for
    proiectio itself.

    Three more things the handles settle:

    - Non-UTF-8 names cannot collide with the projection. Desired
      and manifest paths are Utf8PathBuf by construction, so a
      differently-spelled on-disk name is a different entry.
      fs_utf8's file_name() fails per entry and the walk skips what
      it cannot name; prune treats ENOTEMPTY as kept, not an error.
    - --allow-external-targets coexists. symlink() refuses absolute
      targets, but symlink_contents() writes any target string
      verbatim, and reads through such a link still fail — an
      external target is a pointer, never written through.
      symlink_contents() is Unix-only: a Windows symlink needs a
      kind the desired entry does not carry.
    - Tempfile-persist works inside a Dir. cap-tempfile's TempFile
      plus replace(name) renames over the path atomically and cleans
      up on drop. Permissions, the exec bit included, go on the open
      handle before replace, so bytes and mode publish together in
      the one rename.

    Two Dirs, because a capability follows the tree it guards: the
    dest Dir covers the destination, while the manifest and the
    single-writer lock (section 7) live in the caller-chosen state
    dir of [./design.lex], which need not be inside dest.

    contained_target_chain resolves a target from the link's parent,
    following the links dest holds ([./security.lex] section 3). One
    rule, three callers: decide grades against the destination the
    run leaves, act re-grades against the live disk before publishing
    a link, and act's no-follow walk grades a recorded ancestor link
    one hop at a time. A target one caller reads as in-dest is one
    the others may follow.

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
    ([../../.agents/skills/standout/SKILL.md]) before writing that
    layer.

    To make the adapter layer trivial, the library's error type is a
    structured enum (thiserror) with one refusal variant — carrying
    the refused paths, each with its reason (drift, foreign,
    containment, external target, ...) and the source that named it
    — distinct from I/O errors. The CLI's 0/1/2 exit mapping
    ([./cli-tour.lex]) then falls out of a single match, and messages
    list paths instead of formatting strings deep in the core.

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
      persist removes its tempfile (cap-tempfile's TempFile does
      this on drop), so dest is never littered with .tmp files.

6. Determinism and Ordering

    All collections are BTreeMap/BTreeSet ([./design.lex]); act
    executes in sorted order, parents before children, removals in
    reverse. Plans are diffable, output is stable, failures are
    reproducible.

    Symlinks are the one exception, and grading is why: a link's
    target is graded against the destination as it stands
    ([./security.lex] section 3), so it cannot be published before
    the run has put whatever its target resolves through in place.
    They run after everything else, and one is published only when it
    grades in-dest against the disk *and* the chain that graded it
    walked through no path the run is still going to publish a link
    at — otherwise it is held, the pass repeating over what it held
    until one publishes nothing, which refuses every link still
    waiting. Every path a published link resolves through is
    therefore already final, so no later publication moves where it
    lands, and a run that fails partway leaves no pointer out of dest
    behind. The order stays deterministic: same plan, same
    destination, same sequence.

    All of that is the refusing policy's, since it is the one with a
    plan-time verdict to hold to. Under the external-target permission
    a link neither re-grades nor waits: it is published where sorted
    order puts it, which is the ordinary rule with no exception.

    What the run is still going to publish a link at is named by
    action key, which is a second reason a symlink goes down at its
    key or nowhere (section 3): published anywhere but its key, it
    would be a link no other link's chain waits for, so a landing
    already vouched for could still move. Deciding's no-alias rule
    refuses to plan such a link.

7. Concurrency

    Two processes applying to one destination corrupt the manifest's
    read-modify-write, so a single-writer lock file in the state dir
    excludes the second. The harness use case makes concurrent
    invocations plausible.

    Projection::begin opens the destination, creates and opens the
    state directory, takes the lock, then loads the manifest — in
    that order. The manifest's read-modify-write begins at the load,
    so the load is inside the guard: a run that loaded first would
    persist over whatever a writer finishing in between had recorded.
    The section ends when the Run is dropped, which is after apply
    has persisted the manifest.

    Acquisition is try-lock, so a contended lock is Error::LockHeld
    immediately rather than a wait. The cost is stated rather than
    hidden: a Run holds the guard for its whole life, so a caller
    prompting a human between deciding and applying holds it across
    the prompt, and other runs meet LockHeld. A caller with a plan
    only to show reads Projection::plan instead, which takes no lock.

    The guard is rustix::fs::flock, and the crate requires it. A
    target without flock(2), or without Unix at all, fails to compile
    rather than offering reads it cannot pair with a write.

Notes

    :: notes ::

    1. Candidates for the lexical side, contained_normalize — the one
        side the cap-std adoption above does not cover:
        path-absolutize and normpath (dot-component cleanup without
        canonicalizing); relative-path (portable, strictly relative
        path types); camino (UTF-8 paths, already in the design).
        strict-path drops out on both sides: it resolves against
        the live filesystem, which the lexical side must not, and
        its escape-refusal role is cap-std's at the I/O layer.
    2. openat2 with RESOLVE_BENEATH on Linux 5.6 and newer, openat
        with O_RESOLVE_BENEATH on FreeBSD 13 and newer — a single
        kernel-enforced call — and a per-component walk on macOS
        and Windows: userspace, but every open anchored to the
        directory handle, so still no string checks and no
        dependence on the cwd.
