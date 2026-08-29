Proiectio Design

    The file-projection crate: it maps a computed tree of paths and
    contents onto a target directory, records what it wrote in a
    manifest, and classifies every path before touching it. It
    compiles with no reference to any consumer's domain; a caller
    binds it to whatever computes the tree and to the owner names it
    chooses.

1. The Model

    Three trees, compared pairwise — chezmoi's model: the desired
    tree the caller passes, the recorded state in the manifest, and
    the files on disk. The manifest stores, per path, the SHA-256 of
    the bytes last written — a hash rather than the bytes, because
    the caller can always recompute desired content for a diff, and
    a projected secret is never copied into state.

    The manifest, one JSON file in a caller-chosen directory, written
    atomically and after every other write — and on a failed apply
    still persisted, recording the entries actually applied, so a
    partial run heals on re-run instead of classifying its own
    writes as Foreign:

    pub struct Manifest {
    pub version: u32,
    pub entries: BTreeMap<Utf8PathBuf, ManifestEntry>,
    }
    pub struct ManifestEntry {
    pub kind: EntryKind,          // File | Symlink | Block
    pub hash: String,             // sha256 of written bytes;
      //   Block: of the body
    pub executable: bool,
    pub owners: BTreeSet<String>, // opaque strings
    }

    :: rust ::

    Owners are strings proiectio never interprets; a caller writes
    whatever names the thing that produced the tree. Two owners may
    hold one path only while writing identical bytes — the hash check
    enforces it.

2. Classification and Apply

    Each path in the union of the manifest and the directory gets one
    state; the desired tree enters only when plan compares this
    classification against it to choose actions. Classification
    covers what UTF-8 can name: a non-UTF-8 entry on disk can never
    match a desired or recorded path, so it stays outside the table
    — never overwritten, never removed, and a directory holding one
    is never pruned ([./implementation.lex] section 3):

    | State   | Meaning                                            |
    | Clean   | disk matches the recorded entry: bytes, kind, mode |
    | Drifted | disk differs from the recorded entry — a user edit |
    | Missing | recorded, but gone from disk                       |
    | Foreign | on disk, absent from the manifest                  |

    plan turns states into actions, per path:

    - Disk already equals desired: skip, so re-applying is a no-op
      and mtimes survive.
    - Disk equals recorded and desired differs: overwrite, through a
      tempfile in the target directory persisted over the path — a
      crash leaves the old file or the new one, never a torn write.
    - Drifted: refuse and name the path, unless the caller passes
      DriftPolicy::Overwrite (the CLI's --force).
    - Foreign: refuse always — a projection never overwrites a file
      it did not write.
    - Two desired entries claiming one on-disk location — the same
      path after normalization, or one path beneath another, which no
      non-directory entry permits: both claims refused as a tree
      conflict, since there is no deterministic entry to prefer.
    - A desired symlink whose target grades external
      ([./security.lex] section 3): refused and named with its
      target, unless the caller permits external targets (the CLI's
      --allow-external-targets). Grading is per link and resolves
      through the links the run leaves dest holding — the ones
      already there and the ones the tree projects — so a target
      reaching outside through any of them is external too; what
      apply writes is the target string verbatim. A target that
      is not a pathname on any host — empty, or carrying a NUL — is
      refused before grading and under either policy: it lands
      nowhere to grade, and there is no pointer to permit.
    - Recorded under this owner but absent from the desired tree: an
      orphan, removed when disk still matches the recorded hash and
      refused as drifted otherwise; directories emptied by removal
      are pruned. When another owner still holds the path, the
      departing owner is released from the entry and the disk is
      left alone.

    A projected path never resolves through a symlink: a plan's key
    is the location on disk, so what the manifest records at a path
    is what the next run observes there. A desired path beneath a
    desired link is the tree conflict above; one beneath a link on
    disk that this plan leaves standing — held by another owner, or
    by nobody — is refused as a containment violation. That keeps the
    three stages agreeing, because the walk that observes the
    destination never descends a link: a path beneath one reads as
    gone, so a projection that wrote through the link would plan the
    write again on every run and then refuse its own file as changed.
    A link this plan removes is not in the way — removals run first.
    Apply's walk still follows an owned in-dest link, so a removal
    that names a path recorded beneath one reaches it; planning
    cannot aim that removal, because observation never descends the
    link and the path classifies Missing, whose removal expects
    nothing and refuses as drift when apply finds a node there. A
    write is the other way: the walk may follow the link, but a write
    it relocates off its key is refused at apply too
    ([./implementation.lex] section 3), so nothing the projection
    writes lands beneath a link on either side of the plan — that
    shape survives only in a manifest predating the rule, and
    clearing one means removing the link first.

    For a Block entry, every rule above about "the node at a path"
    means the managed region, not the container file the caller does
    not own whole. Clean, Drifted, Missing, Foreign, the apply-time
    signature re-check and removal all read that way, and the
    machinery above then runs unchanged: the manifest hashes the
    region's body alone, so an edit elsewhere in the container is
    invisible to every comparison, and a container the marker line is
    gone from reads as Missing exactly as a deleted file does.

    Two consequences a reader otherwise gets wrong. An unrecorded
    container does not make the path Foreign — a desired block over a
    file the manifest has never seen plans a write, because writing
    into a file it does not own whole is what a block is for; an
    unrecorded region carrying other bytes is Foreign as any other
    unrecorded node is. And removal strips the region and the marker
    and leaves the container standing, even when the strip empties it:
    a block never creates a container, so the manifest never owns one
    whole, and a file the projection does own whole is a File entry
    whose removal already deletes it.

    The marker rules, the region's byte layout, and the tradeoff
    between prepending and appending are EntryKind::Block's rustdoc,
    not restated here.

    plan and apply are separate calls, so before each overwrite or
    removal apply re-checks the target against the signature the plan
    expects — kind, hash, executable bit — and refuses if the disk
    changed since the plan. A symlink's target is re-graded the same
    way before the link is published, and refuses as an external
    target where the plan-time verdict no longer holds
    ([./security.lex] section 3) — nothing to re-check where the
    caller permitted external targets.

    Removal is a plan against an empty desired tree — same
    classification, same drift refusals — over everything the owner
    holds, or over the recorded paths a caller names instead: a subset
    admits its paths through the same containment gateway, and a named
    path the owner does not hold plans nothing, so a removal re-run
    stays a no-op. Status runs the classification and writes nothing;
    a state directory that does not exist and one holding no manifest
    both read as the empty manifest, so a destination nothing was ever
    projected into reports rather than failing. Proiectio has no
    notion of git: a caller that wants owned paths excluded from
    version control reads the owned-path list off the manifest and
    maintains the exclusion itself.

3. API

    pub enum DriftPolicy { Refuse, Overwrite }
    pub enum ExternalTargetPolicy { Refuse, Allow }
    pub struct PlanOptions {
    pub drift: DriftPolicy,
    pub external_targets: ExternalTargetPolicy,
    }

    pub enum RemovalScope<'a> {
    Everything,
    Paths(&'a BTreeSet<Utf8PathBuf>),
    }

    pub enum Origin {
    Caller,
    Mapping { path: Utf8PathBuf },
    Tree { path: Utf8PathBuf },
    Archive { path: Utf8PathBuf, via: Option<Utf8PathBuf> },
    Files,
    }

    pub struct Projection { target: Utf8PathBuf, state_dir: Utf8PathBuf }
    impl Projection {
    // Reads: no lock, and nothing is written.
    pub fn status(&self) -> Result<Status>;
    pub fn manifest(&self) -> Result<Manifest>;
    pub fn plan(&self, owner: &str, desired: &Desired,
    origin: Origin, options: PlanOptions) -> Result<Plan>;
    pub fn plan_removal(&self, owner: &str, scope: RemovalScope<'_>,
    options: PlanOptions) -> Result<Plan>;
    // The write pass.
    pub fn begin(&self) -> Result<Run>;
    }
    impl Run {
    pub fn plan(&mut self, owner: &str, desired: &Desired,
    origin: Origin, options: PlanOptions) -> Result<&Plan>;
    pub fn plan_removal(&mut self, owner: &str, scope: RemovalScope<'_>,
    options: PlanOptions) -> Result<&Plan>;
    pub fn planned(&self) -> Option<&Plan>;
    pub fn apply(self) -> Result<ApplyReport>;
    }

    :: rust ::

    Desired is BTreeMap<Utf8PathBuf, Entry>, the tree the caller computes
    or loads. Everything else the crate exports is data — Plan, Status,
    Manifest, ApplyReport, Entry, Error — plus the three tree loaders,
    load_mapping, load_tree and load_archive. The stages themselves are
    crate-internal, which is what makes the paragraphs below hold rather
    than merely describe.

    Who opens what. A Projection is a validated pair of absolute paths and
    constructing one touches no filesystem. Every directory handle a call
    needs is opened inside that call and closed when it returns; no public
    item takes or returns one, so the destination handle, the state handle
    and the in-dest state prefix cannot be spelled apart and disagree. The
    destination must already exist — a projection writes where somebody
    chose — and begin creates the state directory where a first run finds
    none.

    So the crate does open invoker-named paths against ambient authority:
    the two paths above, a mapping file and the sources it references, a
    source tree, an archive — all licensed by the trust split
    ([./security.lex] section 1), which reads from anywhere the invoker
    can read. The rule the crate keeps is the narrower one:

    No path computed from content the crate did not author is ever
    resolved against ambient authority, and nothing resolves against the
    process's current directory. Desired-tree keys, symlink targets,
    archive member names and mapping keys reach the filesystem only as
    relative paths, through a directory handle whose root the invoker
    named, after passing the lexical containment gateway.

    Two passes. Reads take no lock and write nothing: status, manifest,
    plan and plan_removal. The Plan they return is a report of what
    applying would do, not a reservation — which is what a dry run wants,
    and why nothing applies one. Writing is a Run, which decides and
    executes under one guard. Run::apply takes no plan argument: the only
    plan a Run can execute is one it decided itself, from the manifest it
    loaded. That is a compile error rather than a documented hazard, and
    the hazard is a deletion — a plan decided unlocked may say Remove for
    a path a concurrent run has since given a second owner, and applying
    it deletes that owner's file where a plan decided under the lock would
    have said Release.

    Projection::plan and Run::plan differ only in whether a guard is held.
    Deliberate: the unlocked one serves dry runs and status, the locked one
    is the only path to apply.

    The lock's critical section. begin opens the destination, creates and
    opens the state directory, takes the single-writer lock, then loads the
    manifest — in that order. The manifest's read-modify-write begins at
    the load, so the load is inside the guard: a run that loaded first
    would persist over whatever a writer finishing in between had recorded.
    The section ends when the Run is dropped, which is after apply has
    persisted the manifest. Acquisition is try-lock, so a contended lock is
    LockHeld immediately rather than a wait. The cost is stated rather than
    hidden: a Run holds the guard for its whole life, so a caller prompting
    a human between deciding and applying holds it across the prompt.

    What a refusal carries. A plan carrying any refusal executes nothing;
    applying it returns the matching variant of Error, aggregating every
    refused path. Refusals — Drift, Foreign, Containment, OwnerConflict,
    ExternalTarget, InvalidTarget, TreeConflict, Block — are distinct from
    I/O and format failures, and Error::is_refusal is the split a CLI's
    0/1/2 exit contract matches on ([./cli-tour.lex]).

    The four whose offending value the desired tree chose — Containment,
    TreeConflict, ExternalTarget, InvalidTarget — also carry the Origin the
    plan was decided with, so a message says which file to go and edit
    rather than only which path was declined. The origin rides on the Plan
    rather than being attached by each loader, so a refusal deciding
    produces names its source as well as one raised while parsing does, and
    Archive carries via because one mapping may name several archives and a
    member path says neither which archive to open nor which line to
    change. Origin::Caller — a caller-computed tree, and every removal —
    renders as nothing, so the no-source case reads as a plain refusal.

    Dependencies: serde, serde_json, thiserror, camino, sha2, cap-std,
    cap-primitives and cap-tempfile for the stages
    ([./implementation.lex] section 3), rustix for the lock, toml for
    mappings, and tar, flate2, zstd and zip for archive expansion, which
    decodes an archive into a desired tree and never extracts one to disk
    ([./security.lex] section 4). The whole crate is a few thousand lines
    plus tests; the tests run against real temp directories, since atomic
    rename is the behavior under test.
