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

2. Containment

    Every path in the desired tree must be relative, and must still
    lie inside the destination after normalization. Refused
    outright: absolute paths, paths that climb out via "..", empty
    or "." components, any backslash, and Windows drive or UNC
    forms (C:..., \\server) in any component — all judged
    lexically, so a Windows-authored tree gets the same verdict on
    every host.

    Normalization alone does not close the hole. A projected symlink
    "logs -> /etc" followed by a projected file "logs/x" is a write
    to /etc/x — the zip-slip pattern, with the traversal smuggled
    through a pointer instead of a "..". So during apply no ancestor
    component of a write may be a symlink, unless proiectio itself
    owns that link and its target resolves inside dest. The refusal
    is structural: it does not matter where the link came from or
    when it appeared.

3. Symlinks

    A symlink's *placement* is a path in the tree and is confined
    like any other. Its *target* is graded:

    | in-dest   | relative target resolving inside dest —      |
    |           | always allowed                               |
    | external  | absolute, or relative escaping dest —        |
    |           | refused unless --allow-external-targets      |

    Resolution follows filesystem semantics — the target string is
    resolved from the link's parent directory — and happens once, at
    plan time, purely to classify. What reaches disk is the target
    string verbatim; proiectio never rewrites it. A link projected
    from a source tree therefore keeps working when it stayed
    relative and in-tree, because the layout around it is preserved.

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
    re-hashes the target and refuses if the disk changed after the
    plan — the drift check holds across the gap between the two
    calls, not just at plan time.

    The manifest itself is written atomically, after every other
    write — and on a failed apply still persisted, recording what
    was actually written, so a partial run heals on re-run instead
    of wedging behind the Foreign rule. It lives with the
    destination (<dest>/.proiectio by default) — implicitly a
    proiectio-owned path.
