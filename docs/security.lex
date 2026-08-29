Security Model

    Proiectio writes files computed from content it did not author —
    mappings, source trees, archives — into a directory somebody
    cares about. The model below is what makes that safe to repeat.
    One sentence version: the invoker is trusted, the tree is not.

1. The Trust Split

    The CLI is a tool you run, not a sandbox you run in. Its user
    already holds every permission the process holds, so restricting
    the *invocation* protects nothing: --dest may point anywhere the
    invoker can write, and sources — mapping files, referenced
    content, source trees, archives — may live anywhere the invoker
    can read. Read from anywhere, write only inside dest.

    The untrusted party is the content. A mapping or archive may come
    from a third party, and everything it computes — projected paths,
    symlink targets, archive member names — is treated as hostile
    input. Every permission that widens what content may do is a flag
    on the invocation, never a key in the mapping: an untrusted
    mapping granting itself permission is no permission at all.

    An embedder gets the same split, spelled in the library's own
    terms rather than the CLI's. The invoker is whoever constructs the
    Projection and hands over a mapping path, a source tree or an
    archive: those are absolute paths the embedder chose, and the
    crate opens them against ambient authority, as it opens the
    destination and the state directory. What it never does is
    narrower:

    No path computed from content the crate did not author is ever
    resolved against ambient authority, and nothing resolves against
    the process's current directory. Desired-tree keys, symlink
    targets, archive member names and mapping keys reach the
    filesystem only as relative paths, through a directory handle
    whose root the invoker named, after passing the lexical
    containment gateway of section 2.

    So an embedder chooses where the projection may read and write by
    choosing those paths, and chooses what content may do by passing
    DriftPolicy and ExternalTargetPolicy — the library-side spellings
    of --force and --allow-external-targets. It cannot hand out the
    permission by other means: nothing public takes or returns a
    directory handle, and the stages that would accept one are
    crate-internal ([./design.lex] section 3).

2. Containment

    Every path in the desired tree must be relative, and must still
    lie inside the destination after normalization. Refused
    outright: absolute paths, paths that climb out via "..", empty
    or "." components, any backslash, any NUL — which terminates a
    pathname rather than appearing in one, so a path carrying one
    names nothing that could be written — and — in any component —
    shapes Windows resolves somewhere other than an ordinary file
    under the destination: drive and UNC forms (C:..., \\server),
    colons (NTFS alternate data streams), trailing dots or spaces
    (Windows strips them before resolving, so ".. " climbs), and
    reserved device names — CON, PRN, AUX, NUL, CONIN$, CONOUT$,
    and COM/LPT followed by a digit 1-9 or a superscript 1-3,
    case-insensitive, extension or not. All judged lexically, so a
    tree gets the same verdict on every host.

    Normalization alone does not close the hole. A projected symlink
    "logs -> /etc" followed by a projected file "logs/x" is a write
    to /etc/x — the zip-slip pattern, with the traversal smuggled
    through a pointer instead of a "..". So during apply no ancestor
    component of a write may be a symlink. The refusal is structural:
    it does not matter where the link came from or when it appeared.

    A link proiectio itself owns whose target resolves inside dest is
    the one the walk still follows, and following is not writing: a
    removal travels through such a link, so that pruning judges the
    directory that actually lost a child, while a write the link
    relocated is refused — the bytes would land at one path with the
    manifest recording another. [./implementation.lex] section 3
    states the answer for each of write, removal and release.

3. Symlinks

    A symlink's *placement* is a path in the tree and is confined
    like any other. Its *target* is graded:

    | in-dest   | relative target resolving inside dest —      |
    |           | always allowed                               |
    | external  | absolute, relative escaping dest, reaching   |
    |           | outside through a link dest will hold, or    |
    |           | one of the two spellings below — refused     |
    |           | unless --allow-external-targets              |
    | not a     | empty, or carrying a NUL — refused under     |
    | path      | either policy                                |

    Grading classifies at plan time and is repeated at apply time
    against the disk, immediately before each link is published; both
    run the same rule. It follows the destination's own links: the
    target string is resolved from the link's parent directory, and a
    component that is itself a symlink is followed to where it
    points, hop by hop. So "pivot/passwd" grades in-dest where
    "pivot" is an ordinary directory and external where dest holds
    "pivot -> /etc". The flag is about whether a pointer reaches
    outside dest, and that is a question about the filesystem: a rule
    answering it by string arithmetic alone answers a different
    question. An ordinary chain is unaffected — "shared -> real"
    under "rc -> shared/rc" lands in dest and needs no flag.
    Following carries a visited set of the links it followed, so a
    cycle ends the resolution rather than looping, and a hop whose
    on-disk target is not UTF-8 ends it too: a hop nobody can follow
    is one nobody can vouch for. Both end it outside — a chain that
    never lands cannot be said to land in dest.

    At plan time the destination a pointer is graded against is the
    one the run *leaves*: the tree's own links are hops, a link the
    run removes is not, and everything the run does not touch is read
    from the observation snapshot. A pointer graded against the
    destination it will live in is what stops two
    links that each land in dest alone from composing into one that
    does not — "b -> ." and "a -> b/../escape" both land in dest read
    separately, and together "a" points at dest's parent, because
    "b/.." pops the directory "b" resolved to. It is also what makes
    the verdict the same on the run that writes a link and on every
    run after it, once that link is on disk to be observed. Reading
    the tree rather than the disk costs nothing in safety: a plan
    holding a single refusal applies nothing, so either every entry
    lands or none does.

    The price is stated rather than hidden: a target's verdict
    depends on what the destination holds, so the same tree may need
    the flag in one destination and not in another. Tree *paths* keep
    the host-independent lexical verdict of section 2. One bound
    still holds: nothing is written *through* any of these links; the
    apply-time walk of section 2 is what enforces that.

    A plan-time verdict is about a destination that can move under
    it, so apply re-grades a link's target against the disk as it
    stands before publishing the link, and refuses it as an external
    target rather than publish a pointer whose pivot was swapped in
    the gap between the two calls — the same shape as the drift
    re-check of section 5. A link the run leaves in place is
    re-graded too, once the run is finished: nothing is published for
    it, but the pivot under it can move like any other. Under the
    flag there is no verdict to re-check: the invoker permitted
    pointers out of dest whatever the destination holds.

    Re-grading against the disk is what makes the order links are
    published in matter, since a run may be putting the pivot its own
    pointer resolves through in place. Links go last, after
    everything else the run writes, and one is published only when
    two things hold at once: it grades in-dest against the disk, and
    the chain that graded it walked through no path the run is still
    going to publish a link at. Otherwise it is held rather than
    refused, until a pass publishes nothing and every link still
    waiting is refused ([./implementation.lex] section 6).

    Grading in-dest at the moment of publishing is not enough on its
    own, which is what the second condition is for: publish "a ->
    b/../escape" against a "b" that still points at a directory, then
    republish "b" at the destination root, and "a" reaches outside
    without either grading ever saying so. Waiting on the paths a
    chain walked means every one of them is final by the time the
    link goes down. So the destination never holds a pointer out of
    dest that this run published — not between two actions, and not
    after a run that failed partway. That is the guarantee of the
    refusing policy, which is the one with a verdict to hold to. Under
    the flag a link is published where sorted order puts it, so a run
    replacing an escaping pivot can hold an escaping pointer partway
    through — pointers out of dest are what the invoker permitted.

    A "." or empty component resolves away as it does on disk, and
    ".." pops what resolution walked — after a followed hop that is
    the hop's own parent, not the directory the target was spelled
    from. Two spellings are graded external outright, on every host,
    for the target as written and for every followed hop alike: a
    backslash anywhere in the target, which is a separator on one
    host and a name on another; and a leading Windows drive
    designator — a letter and a colon, with a slash (C:/escape) or
    without (C:escape) — which Windows resolves against that drive
    rather than against the link's parent. Other colon shapes stay
    names: a target "victim:stream" addresses a sibling's NTFS
    stream, under the destination, not a place outside it. What
    reaches disk is the target string verbatim; proiectio never
    rewrites it. A link projected from a source tree therefore keeps
    working when it stayed relative and in-tree, because the layout
    around it is preserved.

    Placement carries one more rule, and it is the plan-time half of
    section 2's apply-time check rather than the same rule: no
    projected path may lie beneath a symlink at all, the projection's
    own links included ([./design.lex] section 2), where section 2
    still lets apply follow a link proiectio owns whose target
    resolves inside dest. A write through a link would land at a path
    the plan never names, and the classification — which never reads
    through a link — could not see it afterwards.

    One question comes before grading: whether the target is a
    pathname at all. Two strings are not, on any host — the empty
    string, which names nothing, and one carrying a NUL byte, which
    terminates a pathname rather than appearing in one — and both are
    refused outright. No flag lifts that: --allow-external-targets
    permits a pointer to somewhere outside dest, and there is no
    pointer here. It is not a promise that every other target is
    writable; one past the host's length limit is refused by the
    filesystem, which no lexical check foresees.

    An external target writes nothing outside dest — it is only a
    pointer — but a foreign mapping planting pointers into the
    filesystem is a surprise the invoker must opt into, hence the
    flag. Dangling targets are allowed: a symlink is a pointer, and
    the manifest hashes the target string, not what it points at.

4. Archives

    Archive members are the canonical hostile tree — member names
    chosen by whoever built the archive — and get exactly the
    treatment above: each member's path passes containment, each
    symlink member is graded in-dest or external, and extraction
    never writes through a symlinked ancestor. Member kinds are
    restricted to files, directories, and symlinks; hardlinks,
    device nodes, and fifos are refused. Member modes contribute
    only the executable bit.

    Because an archive expands at plan time into ordinary entries,
    nothing after the plan is archive-specific: the same checks, the
    same manifest, the same drift rules.

5. What Was Already Written

    The manifest closes the other half of the loop — protecting the
    destination's own files from the projection:

    - Foreign: a path on disk that the manifest does not record is
      never overwritten and never removed. No flag lifts this.
    - Drifted: a recorded path whose bytes changed on disk is a user
      edit; overwriting or removing it refuses (exit 2) unless the
      invoker passes --force.

    Writes go through a tempfile in the target directory persisted
    over the path, so a crash leaves the old file or the new one,
    never a torn write. Before each overwrite or removal, apply
    re-checks the target against the signature the plan expects —
    kind, hash, executable bit — and refuses if the disk changed
    after the plan; the drift check holds across the gap between the
    two calls, not just at plan time.

    The manifest itself is written atomically, after every other
    write — and on a failed apply still persisted, recording what
    was actually written, so a partial run heals on re-run instead
    of wedging behind the Foreign rule. It lives with the
    destination (<dest>/.proiectio by default) — implicitly a
    proiectio-owned path.
