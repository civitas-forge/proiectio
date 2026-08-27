Proiectio

    Proiectio is a Rust library that projects a computed set of files onto a
    directory: it writes the files, records what it wrote in a manifest, and
    on a later run makes the directory match the caller's new tree — updating
    what changed, removing what is no longer wanted, and refusing to touch
    what it did not write.

    It exists for tools that render managed files into a directory owned by
    someone else — a harness placing skills and hooks into a checkout, an
    environment placing runtime configuration into a workspace. The caller
    computes the desired files; proiectio owns the mechanics that make
    repeated application safe.

1. What A Projection Guarantees

    - Re-applying an unchanged tree writes nothing, so mtimes survive.
    - A user's edit to a projected file is drift: refused and named, never
      silently overwritten.
    - A file on disk the manifest does not list is foreign: never touched.
    - Removal is exact — what the manifest records for an owner, and nothing
      else — and directories emptied by it are pruned.
    - Every write lands through a tempfile persisted over the path: a crash
      leaves the old file or the new one, never a torn write.
    - A delimited managed region inside a shared file is replaced body-only,
      so an edit elsewhere in that file never reads as drift.

2. What It Does Not Know

    Proiectio carries no consumer vocabulary: content arrives as bytes,
    owners are opaque strings, and nothing in the crate names what the files
    are for. It has no notion of git either; a caller that wants projected
    paths kept out of version control reads the owned-path list from the
    manifest and writes the exclusion itself.

3. The Docs

    [./docs/100-design.lex]:
        The model — desired tree, manifest, disk — the path classification,
        the apply mechanics and the API.

4. License

    Proiectio is available under the MIT License ([./LICENSE]).
