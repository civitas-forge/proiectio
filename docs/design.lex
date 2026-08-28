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

    Each path in the union of the desired tree, the manifest, and the
    directory gets one state:

    | State   | Meaning                                           |
    | Clean   | disk matches the recorded hash                    |
    | Drifted | disk differs from the recorded hash — a user edit |
    | Missing | recorded, but gone from disk                      |
    | Foreign | on disk, absent from the manifest                 |

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
    - Recorded under this owner but absent from the desired tree: an
      orphan, removed when disk still matches the recorded hash and
      refused as drifted otherwise; directories emptied by removal
      are pruned.

    Block entries carry a delimited managed region inside a file the
    caller does not own whole: apply locates proiectio's delimiter
    lines, replaces only the body between them, and hashes that body
    alone, so an edit elsewhere in the file never reads as drift.
    Removal strips the block, or deletes the file where the manifest
    owns it whole.

    plan and apply are separate calls, so before each overwrite or
    removal apply re-hashes the target and refuses if the disk
    changed since the plan.

    Removal is a plan against an empty desired tree — same
    classification, same drift refusals — and status runs the
    classification and writes nothing. Proiectio has no notion of
    git: a caller that wants owned paths excluded from version
    control reads the owned-path list off the manifest and maintains
    the exclusion itself.

3. API

    pub enum DriftPolicy { Refuse, Overwrite }

    pub struct Projection { target: Utf8PathBuf, state_dir: Utf8PathBuf }
    impl Projection {
    /// Pure: every write, overwrite, removal, and refusal
    /// apply would perform. An empty tree plans a removal.
    pub fn plan(&self, owner: &str, tree: &BTreeMap<Utf8PathBuf, Entry>,
    policy: DriftPolicy) -> Result<Plan>;
    pub fn apply(&self, plan: &Plan) -> Result<Manifest>;
    pub fn status(&self) -> Result<Status>;
    }

    :: rust ::

    Dependencies: serde_json, sha2, tempfile, camino. The whole crate
    is a few hundred lines plus tests; the tests run against real
    temp directories, since atomic rename is the behavior under test.
