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
    writes as Foreign. The one stop that cannot heal is the save
    itself failing: then the state directory records nothing of the
    run, Stopped says so, and the next run meets those writes as
    Foreign.

    The manifest and its entries:

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

    Owners are strings proiectio never interprets, beyond requiring
    that a name is there at all — an empty or blank owner refuses at
    every planning entry point; a caller writes whatever names the
    thing that produced the tree. Two owners may
    hold one path only while writing identical bytes — the hash check
    enforces it.
2. Classification and Apply

    Each path in the union of the manifest and the directory gets one
    state; status reports them all except the unrecorded directories.
    The desired tree enters only when plan compares this classification
    against it to choose actions. A non-UTF-8 entry on disk can never
    match a desired or a recorded path, so it stays outside the table —
    never overwritten, never removed, and a
    directory holding one is never pruned.

    One state per path:

        | State   | Meaning                                            |
        | Clean   | disk matches the recorded entry: bytes, kind, mode |
        | Drifted | disk differs from the recorded entry — a user edit |
        | Missing | recorded, but gone from disk                       |
        | Foreign | on disk, absent from the manifest                  |

    :: table header=0 ::

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
      path after normalization, or one lying beneath another: both
      refused as a tree conflict, since there is no deterministic
      entry to prefer.
    - A desired symlink whose target leaves the destination: refused
      and named with its target, unless the caller permits external
      targets (the CLI's --allow-external-targets). [./security.lex]
      section 3 grades targets and owns that rule.
    - Recorded under this owner but absent from the desired tree: an
      orphan, removed when disk still matches the recorded hash and
      refused as drifted otherwise; directories emptied by removal
      are pruned. When another owner still holds the path, the
      departing owner is released from the entry and the disk is
      left alone.

    A projected path never resolves through a symlink. A plan's key is
    the location on disk, so what the manifest records at a path is
    what the next run observes there — and the walk that observes the
    destination never descends a link, so anything written beneath one
    would read as gone on every later run. A desired path beneath a
    link this plan leaves standing is refused as a containment
    violation; [./implementation.lex] section 3 owns that rule and
    says what apply does when it meets the shape anyway.

    For a Block entry, every rule above about "the node at a path"
    means the managed region, not the container file the caller does
    not own whole. The manifest hashes the region's body alone, so an
    edit elsewhere in the container is invisible to every comparison,
    and a container the marker line is gone from reads as Missing
    exactly as a deleted file does.

    Two consequences a reader otherwise gets wrong. An unrecorded
    container does not make the path Foreign — a desired block over a
    file the manifest has never seen plans a write, because writing
    into a file it does not own whole is what a block is for; an
    unrecorded region carrying other bytes is Foreign as any other
    unrecorded node is. And removal strips the region and leaves the
    container standing, even when the strip empties it: a block never
    creates a container, so the manifest never owns one whole.

    The marker rules, the region's byte layout, and what an author
    writing past the region's outer edge costs are EntryKind::Block's
    rustdoc, not restated here.

    plan and apply are separate calls, so before each overwrite or
    removal apply re-checks the target against the signature the plan
    expects — kind, hash, executable bit — and refuses if the disk
    changed since the plan. A symlink's target is re-graded the same
    way before the link is published.

    Removal is a plan against an empty desired tree — same
    classification, same drift refusals — over everything the owner
    holds, or over the recorded paths a caller names instead; a named
    path the owner does not hold plans a NotRecorded row that touches
    nothing, so the caller learns the path was never held and a
    re-run still changes no disk. Status runs the classification and writes nothing, and a
    destination nothing was ever projected into reports rather than
    failing. Proiectio has no notion of git: a caller that wants owned
    paths excluded from version control reads the owned-path list off
    the manifest and maintains the exclusion itself.

3. API

    The exported surface:

        pub enum DriftPolicy { Refuse, Overwrite }
        pub enum ExternalTargetPolicy { Refuse, Allow }
        pub struct PlanOptions { drift, external_targets }
        pub enum RemovalScope<'a> { Everything, Paths(&'a BTreeSet<Utf8PathBuf>) }
        pub enum Origin { Caller, Mapping, Tree, Archive, Files }

        pub struct Projection { target: Utf8PathBuf, state_dir: Utf8PathBuf }
        impl Projection {
        // Reads: no lock, nothing written.
        pub fn status(&self) -> Result<Status>;
        pub fn manifest(&self) -> Result<Manifest>;
        pub fn plan(&self, owner: &str, desired: &Desired,
        origin: Origin, options: PlanOptions) -> Result<Planned>;
        pub fn plan_removal(&self, owner: &str, scope: RemovalScope<'_>,
        drift: DriftPolicy) -> Result<Planned>;
        // The write pass.
        pub fn begin(&self) -> Result<Run>;
        }
        impl Run {
        pub fn plan(&mut self, ...) -> Result<&Plan>;
        pub fn plan_removal(&mut self, ...) -> Result<&Plan>;
        pub fn apply(self) -> Result<ApplyReport, Box<Aborted>>;
        }

    :: rust ::

    Desired is the tree the caller computes or loads with
    load_mapping, load_tree or load_archive: a
    BTreeMap<Utf8PathBuf, Entry>, beside the Dropped records for
    archive members a strip consumed (an expansion the strip would
    consume whole fails the load instead). The records ride the Plan
    and the ApplyReport so a run's caller sees what its archives shed.
    Everything else the crate exports is data: Plan, Status, Manifest,
    ApplyReport, Aborted, Stopped, Dropped, Entry, Error. An apply
    that stops early answers with Aborted — the rows it applied
    beside a Stopped naming whether the state directory records
    them. The three stages are crate-internal.

    Reads take no lock. The Plan they return says what applying would
    do; it is not a reservation, which is why nothing applies one.
    Writing is a Run, which decides and executes under one guard, and
    Run::apply takes no plan argument — so the only plan a Run can
    execute is one it decided itself. That is a compile error rather
    than a documented hazard, and the hazard it forecloses is a
    deletion: a plan decided unlocked can say Remove for a path a
    concurrent run has since given a second owner.
    [./implementation.lex] section 7 owns the lock's critical section.

    The rustdoc owns the contract: what each call refuses, and on what
    terms. [./cli-tour.lex] owns the 0/1/2 exit contract that
    Error::is_refusal feeds.

    Dependencies: serde, serde_json, thiserror, camino, sha2, cap-std,
    cap-primitives and cap-tempfile for the stages, rustix for the
    lock, toml for mappings, and tar, flate2, zstd and zip for archive
    expansion, which decodes an archive into a desired tree and never
    extracts one to disk ([./security.lex] section 4). The whole crate
    is a few thousand lines plus tests; the tests run against real temp
    directories, since atomic rename is the behavior under test.
