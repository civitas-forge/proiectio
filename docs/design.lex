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
    Apply's walk still follows an owned in-dest link, so a plan that
    names a path recorded beneath one reaches it; planning cannot
    write such a plan, because observation never descends the link
    and the path classifies Missing, whose removal expects nothing
    and refuses as drift when apply finds a node there. Nothing the
    projection writes lands beneath a link under this rule, so that
    shape survives only in a manifest predating it — and clearing one
    means removing the link first.

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

    pub struct Projection { target: Utf8PathBuf, state_dir: Utf8PathBuf }
    impl Projection {
    /// Pure: every write, overwrite, removal, and refusal
    /// apply would perform. An empty tree plans a removal.
    pub fn plan(&self, owner: &str, tree: &BTreeMap<Utf8PathBuf, Entry>,
    options: PlanOptions) -> Result<Plan>;
    /// The removal on its own terms: everything this owner
    /// holds, or the recorded paths named. Clearing the owner
    /// and naming no path are separate spellings, never an
    /// empty list.
    pub fn plan_removal(&self, owner: &str, scope: RemovalScope<'_>,
    options: PlanOptions) -> Result<Plan>;
    pub fn apply(&self, plan: &Plan) -> Result<ApplyReport>;
    pub fn status(&self) -> Result<Status>;
    }

    :: rust ::

    Those four methods are the shape the stages take once one value
    holds both paths, and none of them exists yet. What the library
    ships is the stages themselves, as free functions over directory
    handles the caller opened — decide, decide_removal, apply,
    status — because nothing in the crate opens an ambient path
    itself ([./implementation.lex] section 3). Where Projection would
    answer from its two paths, status takes a StateDir naming the
    state handle together with where that directory sits relative to
    the destination — the two travel as one value, since a handle and
    a path that disagree would produce a confident, wrong report and
    nothing can check one against the other. The methods land over
    those functions when the CLI has a caller for them.

    Dependencies: serde, serde_json, thiserror, camino now; sha2
    and cap-std arrive with observe, cap-tempfile and
    cap-primitives with apply ([./implementation.lex] section 3); tar,
    flate2, zstd and zip with archive expansion, which decodes an
    archive into a desired tree and never extracts one to disk
    ([./security.lex] section 4).
    The whole crate
    is a few hundred lines plus tests; the tests run against real
    temp directories, since atomic rename is the behavior under test.
