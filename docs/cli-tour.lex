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
        wrote      bin/tool              (exec)
        wrote      config/settings.toml
        linked     current               -> releases/1.2.3
        3 written, 0 skipped

        $ proiectio write deploy.toml --dest ~/apps/site --owner site

    :: shell ::

    A directory tree, verbatim, metadata copied from the source:

        $ proiectio write --tree ./skeleton --dest /srv/www

    :: shell ::

    An archive as the tree — --tree accepts .tar, .tar.gz/.tgz,
    .tar.zst and .zip, and treats the archive's members as the
    source tree. The extension picks the decoder; a wrong pick is an
    error, not a guess. --strip N drops N leading path components. A
    member the strip leaves with no path at all — the AppleDouble
    ._ files a stock macOS tar embeds, typically — is skipped and
    reported as a dropped row; a strip that would consume every
    member the archive has fails the load instead of projecting an
    empty tree.

    A release tarball wrapped in a top-level directory:

        $ proiectio write --tree ./skeleton-1.2.tar.gz --strip 1

    :: shell ::

    Loose files — sugar for a one-entry-per-basename tree:

        $ proiectio write ./motd ./banner.txt

    :: shell ::

    Unchanged files are skipped and their mtimes survive.

    Re-running any write is a no-op:

        $ proiectio write deploy.toml
        skipped    bin/tool              (exec)
        skipped    config/settings.toml
        skipped    current               -> releases/1.2.3
        3 unchanged

    :: shell ::

    The one exception is a mapping with an external-target link: the
    write refuses without --allow-external-targets even when disk
    already matches and status reports clean. The flag is
    per-invocation permission, never recorded state, so every run of
    that mapping needs it.

2. Dry Runs and Exit Codes

    --dry-run prints the plan — the same classification apply would
    act on — and writes nothing. Exit codes are the verdict, on dry
    and real runs alike, so CI can gate on either.

    The three exit codes:

        | 0 | applied, or nothing to do                                           |
        | 1 | usage or I/O error                                                  |
        | 2 | refusal — drift, foreign file, containment, external symlink target |

    :: table header=0 ::

    A clean dry run:

        $ proiectio write deploy.toml --dry-run
        would skip       bin/tool              (exec)
        would skip       config/settings.toml
        would skip       current               -> releases/1.2.3
        $ echo $?
        0

    :: shell ::

    A refused path is a row like any other, and the run still exits 2:

        $ echo edited >> bin/tool
        $ proiectio write deploy.toml --dry-run
        would refuse     bin/tool              (drifted) (from mapping /home/you/deploy.toml)
        would skip       config/settings.toml
        would skip       current               -> releases/1.2.3
        $ echo $?
        2

    :: shell ::

    Refusals are overridden one policy at a time, always from the invocation:

        $ proiectio write deploy.toml --force                   # overwrite drifted files
        $ proiectio write deploy.toml --allow-external-targets  # permit symlinks out of dest

    :: shell ::

3. Status

    Reads the manifest, classifies every recorded path, writes nothing:

        $ rm current
        $ proiectio status --dest ~/apps/site
        drifted  bin/tool
        clean    config/settings.toml
        missing  current

    :: shell ::

4. Removal

    rm removes what the manifest owns. A drifted file refuses (exit 2)
    unless --force; directories emptied by removal are pruned.

    Everything under an owner:

        $ proiectio rm --dest ~/apps/site --owner site
        removed    bin/tool              (exec)
        removed    config/settings.toml
        removed    current
        3 removed

    :: shell ::

    Or a subset by path:

        $ proiectio rm config/settings.toml
        removed    config/settings.toml
        1 removed

    :: shell ::

5. The Mapping File

    A TOML file. Keys are projected paths — relative, and confined to
    the destination after normalization. Every file entry carries
    exactly one of contents and source; relative source paths resolve
    against the mapping file's own directory, so a mapping and its
    assets travel together. Metadata is the executable bit — from the source file,
    or platform default for inline contents, overridable per entry.

    A mapping:

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
    time its members expand into ordinary file and symlink entries,
    hashed and tracked individually — status reports drift per
    member, rm removes per member. The one trace of the archive
    downstream is the dropped record each member a strip consumed
    leaves in the write's report; the projected entries themselves
    remember nothing. Directory members carry no entry of their
    own, as a walked directory does not: a projected tree implies its
    directories from its files' parent components, so an archive
    whose only content is an empty directory projects nothing.
    Members are confined like any other tree content
    ([./security.lex]). An archive that appears *inside* a source
    tree is just a file and is copied verbatim; extraction happens
    only where it is explicitly requested — an \[archives.] entry, or
    --tree pointed at the archive itself.

6. Options

    The projection flags:

        | --dest <dir>             | target directory; default cwd                  |
        | --owner <name>           | manifest owner; default from the configuration |
        | --state-dir <dir>        | manifest location; default <dest>/.proiectio   |
        | --dry-run                | plan and report, write nothing                 |
        | --force                  | overwrite drifted files; remove them under rm  |
        | --allow-external-targets | permit symlink targets outside dest            |
        | --tree <path>            | project a directory or archive                 |
        | --strip <n>              | drop n leading components (archive trees)      |

    :: table header=0 ::
