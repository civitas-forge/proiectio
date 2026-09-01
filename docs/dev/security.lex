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

    An embedder gets the same split in the library's terms. The
    invoker is whoever constructs the Projection and hands over a
    mapping path, a source tree or an archive; the crate opens those
    against ambient authority, as it opens the destination and the
    state directory. The narrower rule it keeps:

    No path content chose as a place to *write* is ever resolved
    against ambient authority, and nothing resolves against the
    process's current directory. Desired-tree keys, symlink targets,
    archive member names and mapping keys reach the filesystem only
    as relative paths, through a directory handle whose root the
    invoker named, after passing the gateway of section 2.

    Places content chooses to *read* are not covered. A mapping's
    \[files\] and \[archives\] source values are joined onto the mapping
    file's own directory and opened against ambient authority, and a
    rooted source supplants that directory. So a mapping can name any
    file the process can read and project its bytes into the
    destination: read a third-party mapping before running it, on the
    same terms as a third-party script. What it cannot choose is
    where those bytes land.

2. Containment

    Every path in the desired tree must be relative, and must still
    lie inside the destination after normalization. Refused
    outright: absolute paths, paths that climb out via "..", empty
    or "." components, any backslash, any NUL — which terminates a
    pathname rather than appearing in one — and, in any component,
    shapes Windows resolves somewhere other than an ordinary file
    under the destination: drive and UNC forms (C:..., \\server),
    colons (NTFS alternate data streams), trailing dots or spaces
    (Windows strips them before resolving, so ".. " climbs), and
    reserved device names — CON, PRN, AUX, NUL, CONIN$, CONOUT$,
    and COM/LPT followed by a digit 1-9 or a superscript 1-3,
    case-insensitive, extension or not. All judged lexically, so a
    tree gets the same verdict on every host.

    The invoker may narrow that destination further by naming path
    components the Projection prunes. A desired or removal path that
    enters one is refused as Containment. Observation checks a name
    before stat or open, and apply checks each ancestry component again,
    so neither stage reads through the pruned directory. Link-target
    grading cannot prove a target that enters one remains inside the
    destination; the refusing policy therefore grades it external. The
    external-target permission writes the pointer without entering the
    pruned path. A containing directory is marked incomplete when the
    walk skips a pruned child, so no plan removes or replaces that parent
    on the assumption that the unobserved child is absent. A prune set
    that overlaps an in-target state directory is rejected. A refusal may
    state that pruned contents exist, but it does not name their paths.

    Normalization alone does not close the hole. A projected symlink
    "logs -> /etc" followed by a projected file "logs/x" is a write
    to /etc/x — the zip-slip pattern, with the traversal smuggled
    through a pointer instead of a "..". So during apply no ancestor
    component of a write may be a symlink. The refusal is structural:
    it does not matter where the link came from or when it appeared.

    A link proiectio owns whose target resolves inside dest is the
    one exception the walk follows, and following is not writing: a
    removal travels through such a link so that pruning judges the
    directory that actually lost a child, while a write the link
    relocated is refused — the bytes would land at one path with the
    manifest recording another. [./implementation.lex] section 3
    states the answer for each of write, removal and release.

3. Symlinks

    A symlink's *placement* is a path in the tree and is confined
    like any other.

    Its *target* is graded:

        | in-dest   | relative target resolving inside dest —      |
        |           | always allowed                               |
        | external  | absolute, relative escaping dest, reaching   |
        |           | outside through a link dest will hold, or    |
        |           | a backslash or drive designator anywhere —   |
        |           | refused unless --allow-external-targets      |
        | not a     | empty, or carrying a NUL — refused under     |
        | path      | either policy                                |

    :: table header=0 ::

    Grading resolves the target string from the link's parent and
    follows the destination's own links hop by hop, so "pivot/passwd"
    grades in-dest where "pivot" is a directory and external where
    dest holds "pivot -> /etc". The flag asks whether a pointer
    reaches outside dest, which is a question about the filesystem;
    string arithmetic alone answers a different one. An ordinary
    chain is unaffected — "shared -> real" under "rc -> shared/rc"
    needs no flag. A visited set ends a chain that revisits a hop,
    and a hop whose on-disk target is not UTF-8 ends one too; both
    end it outside, since a chain that never lands cannot land in
    dest.

    The destination a pointer is graded against is the one the run
    *leaves*: the tree's own links are hops, a link the run removes
    is not. That is what stops two links each landing in dest from
    composing into one that does not — "b -> ." and "a ->
    b/../escape" both land in dest read separately, and together "a"
    points at dest's parent. The price: a target's verdict depends on
    what the destination holds, so the same tree may need the flag in
    one destination and not another. Tree *paths* keep the
    host-independent lexical verdict of section 2.

    Apply re-grades against the disk immediately before publishing a
    link, and refuses one whose pivot was swapped since the plan.
    Under the flag there is no verdict to re-check. Re-grading is
    what makes publication order matter, since a run may be putting
    the pivot its own pointer resolves through in place;
    [./implementation.lex] section 6 owns that order and what it
    guarantees.

    Two spellings grade external on every host, for the target as
    written and for every followed hop: a backslash anywhere, which
    is a separator on one host and a name on another; and a leading
    Windows drive designator — a letter and a colon, with a slash
    (C:/escape) or without (C:escape) — which Windows resolves
    against that drive rather than the link's parent. Other colon
    shapes stay names: "victim:stream" addresses a sibling's NTFS
    stream, under the destination. A "." or empty component resolves
    away as on disk, and ".." pops what resolution walked — after a
    followed hop that is the hop's own parent, not the directory the
    target was spelled from.

    Nothing is written *through* any of these links; the apply-time
    walk of section 2 enforces that. What reaches disk is the target
    string verbatim, so a link projected from a source tree keeps
    working when it stayed relative and in-tree.

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
    of wedging behind the Foreign rule ([./design.lex] carries the
    one exception, a save that itself fails). It lives with the
    destination (<dest>/.proiectio by default) — implicitly a
    proiectio-owned path.
