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
    against an empty desired tree. The engine enforces the split: act
    takes only a Plan, never the mapping or the disk directly.

    One deliberate exception to "verbatim": before each destructive
    action — overwrite or removal — act re-checks the target against
    the signature the plan expects — kind, hash, executable bit — and
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

    Every untrusted path enters through one gateway:

    contained_join(dest, rel) -> Result<Utf8PathBuf>

    :: rust ::

    with a crate-internal normalize-only half, contained_normalize,
    applying the same rules without the join — decide admits desired
    keys through it, since a Plan keys actions relative to the
    destination — the sole lexical gateway enforcing the containment
    rules of
    [./security.lex] section 2 — that section's refusal list, not
    any paraphrase elsewhere, is the contract; act's no-follow walk
    below is the apply-time half of the same rule. Crates are
    implementation details inside it [1], and two constraints bound
    the choice:

    - Containment wants *lexical* normalization — never
      std::fs::canonicalize, which follows symlinks and requires
      paths to exist.
    - The check that matters most — no symlinked ancestor at write
      time — is ours to enforce regardless of crate.

    Spiked and decided (issue #3): adopt cap-std. Observe and act do
    their I/O through a cap_std::fs_utf8::Dir rooted at dest —
    camino-typed, and every open refuses any path whose resolution
    escapes that root: an escaping "..", an absolute path, a
    symlink chain that leaves the Dir [2]. An in-dest ".." still
    resolves — the refusal is about where a path lands, not how it
    is spelled. Escapes fail no matter when a hostile link appears,
    so the plan-to-apply race is closed structurally, not by string
    checks. What the spike verified:

    - A write through an escaping symlinked ancestor — "logs ->
      ../outside", or an absolute target — fails at open time. The
      other half of the [./security.lex] rule stays ours: an in-dest
      symlinked ancestor is followed silently, so refusing links
      proiectio does not own is still our check — and act enforces
      it through the handle, not beside it. Each ancestor component
      is opened with cap-primitives' open_dir_nofollow — public
      there, not on Dir itself, so cap-primitives joins the
      dependency list — from the previously verified handle, and
      the final mutation happens relative to that parent, so a
      component swapped for a symlink after the check cannot
      redirect a write to an in-dest path the plan never named.
      When a no-follow open does report a symlink, act matches it
      against the manifest before anything else: not recorded — the
      structured refusal below; recorded, but the on-disk target no
      longer hashes to the recorded string — the same Drift refusal
      every stale plan gets (section 1); recorded and matching, but
      graded external — refused too, an external target is never
      written through; recorded, matching, and in-dest — act reads
      the target through the parent handle, resolves it with
      contained_join, and restarts the no-follow walk from the dest
      root along the resolved path. Restarts carry a per-walk
      visited set: revisiting a component means an owned-link
      cycle, and refuses rather than loops.
      One boundary the handles do not close: a directory handle
      follows its object, so a process renaming a verified ancestor
      out of dest carries the handle with it. That actor holds
      invoker-level write access and is trusted by the
      [./security.lex] split — the structural claim here is about
      hostile content, paths and pointers, not about a concurrently
      mutating privileged process, which the single-writer lock
      (section 7) already rules out for proiectio itself.
    - Non-UTF-8 names cannot collide with the projection. Desired
      and manifest paths are Utf8PathBuf by construction, and a
      differently-spelled on-disk name is a different entry, so a
      non-UTF-8 entry is invisible to classification — protected
      like Foreign in effect, but never a row in the state table,
      because Status cannot name it. fs_utf8's file_name() fails
      per entry, so the walk skips what it cannot name;
      [./design.lex] records the matching scope on the
      classification contract, and prune treats a not-actually-
      empty directory (ENOTEMPTY — perhaps holding a skipped entry)
      as kept, not as an error.
    - --allow-external-targets coexists: symlink() refuses absolute
      targets, but symlink_contents() writes any target string
      verbatim — act uses it for flag-permitted links — and reads
      through such a link still fail, which is the model exactly: an
      external target is a pointer, never written through.
      read_link_contents() hands observe the string back untouched
      — via the plain-Dir view (as_cap_std()), because the fs_utf8
      wrapper errors on a non-UTF-8 target, and a recorded link
      whose target was edited to such bytes must classify as
      Drifted, not fail observe.
      One platform edge: symlink_contents() is Unix-only — a
      Windows symlink needs a file-or-directory kind the entry does
      not carry, and dangling targets are allowed, so act could not
      infer one. Nothing here targets Windows; if that changes, the
      desired-tree symlink entry grows a kind first.
    - Tempfile-persist works inside a Dir: cap-tempfile's
      TempFile::new(dir.as_cap_std()) — its signature takes the
      plain cap_std::fs::Dir, so the fs_utf8 handle converts at the
      call — plus replace(name) renames over the path atomically,
      replaces existing files, and cleans up on drop. Permissions,
      the exec bit included, go on the open tempfile handle before
      replace, so bytes and mode publish together in the one rename
      — never a visible file with a wrong mode.

    Two Dirs, because a capability follows the tree it guards: the
    dest Dir covers the destination tree, while the manifest and
    the single-writer lock (section 7) live in the caller-chosen
    state dir of [./design.lex] — which need not be inside dest —
    under a second Dir rooted there.

    contained_join stays the plan-time gateway regardless: cap-std
    tolerates in-dest "..", and at link creation refuses only an
    absolute target — an escaping relative target writes fine and
    fails only when traversed — so grading targets in-dest or
    external stays contained_join's job. The structured Containment
    refusal, with paths, names its producers: decide — the lexical
    rules, and paths entering the projection's own state directory
    — and act's no-follow walk, on an ancestor symlink that is
    unowned, cyclic, or graded external; one refusal variant,
    because [./security.lex] states one rule. (A swapped owned
    target is the walk's Drift refusal, not Containment.) A Dir
    escape refusal past that walk means a bug in the walk itself —
    defense in depth — and surfaces as the I/O error it is.

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
      persist removes its tempfile (cap-tempfile's TempFile does
      this on drop), so dest is never littered with .tmp files.

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

Notes

    :: notes ::

    1. Candidates for the lexical side of contained_join — the one
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
