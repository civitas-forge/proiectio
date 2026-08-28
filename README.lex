Proiectio

    Proiectio is a Rust library and cli that injects(projects)  a set of files onto a directory: it writes the files, records what it wrote in a manifest, and on a later run makes the directory match the caller's new tree — updating what changed, removing what is no longer wanted, and refusing to touch what it did not write.

    It exists for tools that render managed files into a directory owned by
    someone else — a harness placing skills and hooks into a checkout, an
    environment placing runtime configuration into a workspace. The caller
    computes the desired files; proiectio owns the mechanics that make
    repeated application safe.

    It keeps track of what paths it wrote, and digests of their content, hence allowing it to safely keep files with additional changes and otherwise remove it's injected content.

1. Node Types

    1.1. File's Content

        Contents are opaque to proiectio, all it knows is the digest of the content.

    1.2. Symlinks

        Proiectio supports both injecting regular directories, files and symlinks, as well symlinks to projected files or dirs.

    1.3 Directories

        By default, proiectio will merge existing directories with the desired tree, preserving any additional changes, as long as the rules for not overwriting pre existing files is violated.

2. Content Definitions

    The paths and their contents can be defined in two ways: either via a mapping file (a TOML file) or via a directory tree.

    In the former, the file's content can either be inlined as a string or referenced from a file path. In that form, file metadata follows the file system's defaults, and can be overriden.

    If specifying from a directory tree, metadata is copied from the source files.

    Archives (tar, tar.gz, tar.zst, zip) can stand in for a tree — as the tree source itself, or as a mapping entry extracted under a path prefix. Members expand into ordinary entries at plan time and are tracked individually.

3. The Docs

    [./docs/design.lex]:
        The model — desired tree, manifest, disk — the path classification,
        the apply mechanics and the API.

    [./docs/cli-tour.lex]:
        The CLI — writing from mappings, trees, and archives; status,
        removal, exit codes, and the mapping file format.

    [./docs/security.lex]:
        The trust split, containment, symlink grading, and archive
        extraction rules.

4. License

    Proiectio is available under the MIT License ([./LICENSE]).
