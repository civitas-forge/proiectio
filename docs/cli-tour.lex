The CLI

    Proiectio ships as a Rust library and a CLI with feature parity;
    every command is a thin wrapper over the plan/apply/status API
    ([./design.lex]). The split that shapes every invocation: the
    mapping or tree says *what* is projected, the invocation says
    *where* and *how much* — destination, owner, and every
    permission-granting flag live on the command line, never in the
    content. [./security.lex] explains why.

1. Writing

    One command, four ways to hand it a desired tree. The destination
    defaults to the current directory; --dest may point anywhere the
    invoker can write.

    A mapping file (a single file positional):

        $ proiectio write deploy.toml
        wrote   config/settings.toml
        wrote   bin/tool            (exec)
        linked  current -> releases/1.2.3
        3 written, 0 skipped

        $ proiectio write deploy.toml --dest ~/apps/site --owner site

    :: shell ::

    A directory tree, verbatim, metadata copied from the source:

        $ proiectio write --tree ./skeleton --dest /srv/www

    :: shell ::

    An archive as the tree — --tree accepts .tar, .tar.gz/.tgz,
    .tar.zst and .zip, and treats the archive's members as the
    source tree. The
    extension picks the decoder; a wrong pick is an error, not a
    guess. --strip N drops N leading path components, for release
    tarballs wrapped in a top-level directory:

        $ proiectio write --tree ./skeleton-1.2.tar.gz --strip 1

    :: shell ::

    Loose files — sugar for a one-entry-per-basename tree:

        $ proiectio write ./motd ./banner.txt

    :: shell ::

    Re-running any write is a no-op: unchanged files are skipped and
    their mtimes survive.

        $ proiectio write deploy.toml
        3 unchanged

    :: shell ::

2. Dry Runs and Exit Codes

    --dry-run prints the plan — the same classification apply would
    act on — and writes nothing. Exit codes are the verdict, on dry
    and real runs alike, so CI can gate on either:

    | 0 | applied, or nothing to do                              |
    | 1 | usage or I/O error                                     |
    | 2 | refusal — drift, foreign file, containment, external   |
    |   | symlink target                                         |

        $ proiectio write deploy.toml --dry-run
        would overwrite  config/settings.toml   (clean, content changed)
        would refuse     bin/tool               (drifted - local edit)
        $ echo $?
        2

    :: shell ::

    Refusals are overridden one policy at a time, always from the
    invocation:

        $ proiectio write deploy.toml --force                   # overwrite drifted files
        $ proiectio write deploy.toml --allow-external-targets  # permit symlinks out of dest

    :: shell ::

3. Status

    Reads the manifest, classifies every recorded path, writes
    nothing:

        $ proiectio status --dest ~/apps/site
        clean    config/settings.toml
        drifted  bin/tool
        missing  current

    :: shell ::

4. Removal

    rm removes what the manifest owns — everything under an owner, or
    a subset by path. A drifted file refuses (exit 2) unless --force;
    directories emptied by removal are pruned.

        $ proiectio rm --dest ~/apps/site --owner site
        $ proiectio rm config/settings.toml

    :: shell ::

5. The Mapping File

    A TOML file. Keys are projected paths — relative, and confined to
    the destination after normalization. Every file entry carries
    exactly one of contents and source; relative source paths resolve
    against the mapping file's own directory, so a mapping and its
    assets travel together. Metadata is the executable bit — from the source file,
    or platform default for inline contents, overridable per entry.

    version = 1

    [files."config/settings.toml"]
    contents = """
    listen = ":8080"
    """

    [files."bin/tool"]
    source = "./assets/tool.sh"
    executable = true

    # standard symlink semantics: target is written verbatim and
    # resolves relative to the link's parent, inside dest
    [links."current"]
    target = "releases/1.2.3"

    # absolute target: refused unless the invoker passes
    # --allow-external-targets
    [links."toolchain"]
    target = "/opt/toolchains/rust-1.80"

    # extracted under the key prefix at plan time; each member
    # becomes an ordinary manifest entry
    [archives."vendor/"]
    source = "./assets/vendor.tar.gz"
    strip = 1

    :: toml ::

    An archive entry is a tree constructor, not a node type: at plan
    time its members expand into ordinary file, directory, and
    symlink entries, hashed and tracked individually — status reports
    drift per member, rm removes per member, and nothing downstream
    remembers an archive existed. Members are confined like any other
    tree content ([./security.lex]). An archive that appears *inside*
    a source tree is just a file and is copied verbatim; extraction
    happens only where it is explicitly requested — an [archives.]
    entry, or --tree pointed at the archive itself.

6. Options

    | --dest <dir>             | target directory; default cwd       |
    | --owner <name>           | manifest owner; default "default"   |
    | --state-dir <dir>        | manifest location; default          |
    |                          | <dest>/.proiectio                   |
    | --dry-run                | plan and report, write nothing      |
    | --force                  | overwrite drifted files             |
    | --allow-external-targets | permit symlink targets outside dest |
    | --tree <path>            | project a directory or archive      |
    | --strip <n>              | drop n leading components           |
    |                          | (archive trees)                     |
