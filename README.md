# proiectio

Proiectio projects a computed set of files onto a directory owned by someone else. It writes the files, records what it wrote in a manifest (`<dest>/.proiectio` by default), and on a later run makes the directory match the caller's new tree: updating what changed, removing what is no longer wanted, and refusing to touch what it did not write.

It is built for tools that render managed files into a checkout or a workspace — a harness placing skills and hooks into a repository, an environment placing runtime configuration into a working directory. It ships as the `proiectio` CLI and the `libproiectio` Rust library, with feature parity between them.

## Install

```console
$ cargo install proiectio --locked
```

Rust 1.96 or later. Unix only: macOS and Linux.

## Quick start

A mapping file lists the paths to project and where their contents come from. Relative `source` paths resolve against the mapping file's own directory.

```toml
version = 1

[files."config/settings.toml"]
contents = """
listen = ":8080"
"""

[files."bin/tool"]
source = "./assets/tool.sh"
executable = true

[links."current"]
target = "releases/1.2.3"
```

```console
$ proiectio write deploy.toml --dest ./site --owner site
wrote      bin/tool              (exec)
wrote      config/settings.toml
linked     current               -> releases/1.2.3
3 written, 0 skipped
```

Re-running the same write converges on the same tree.

```console
$ proiectio write deploy.toml --dest ./site --owner site
skipped    bin/tool              (exec)
skipped    config/settings.toml
skipped    current               -> releases/1.2.3
3 unchanged
```

Unchanged files are skipped, and their mtimes survive.

## Or project a tree or archive

`--tree` takes a directory and projects it verbatim, metadata copied from the source.

```console
$ proiectio write --tree ./skeleton --dest ./tree-dest --owner skel
wrote      .gitignore
wrote      notes/README
2 written, 0 skipped
```

It also takes an archive — `.tar`, `.tar.gz`/`.tgz`, `.tar.zst`, `.zip` — and treats the archive's members as the source tree. `--strip N` drops N leading path components.

```console
$ proiectio write --tree ./release-1.2.tar.gz --strip 1 --dest ./tree-dest --owner rel
wrote      share/data.txt
wrote      share/guide.txt
2 written, 0 skipped
```

Two or more file positionals project those files under their basenames; a single positional is always read as a mapping.

## Check and remove

`status` reads the manifest, classifies every recorded path, and writes nothing.

```console
$ proiectio status --dest ./site
clean    bin/tool
clean    config/settings.toml
clean    current
$ echo edited >> ./site/bin/tool
$ rm ./site/current
$ proiectio status --dest ./site
drifted  bin/tool
clean    config/settings.toml
missing  current
```

Plain `status` exits 0. `--check` exits 2 when any row is not clean, so CI can gate on it.

```console
$ proiectio status --dest ./site --check
drifted  bin/tool
clean    config/settings.toml
missing  current
$ echo $?
2
```

`rm` removes everything the manifest records under an owner. Directories emptied by removal are pruned.

```console
$ proiectio rm --dest ./site --owner site
removed    config/settings.toml
removed    current               -> releases/1.2.3
2 removed
```

Or a subset, by path:

```console
$ proiectio rm share/guide.txt --dest ./tree-dest --owner rel
removed    share/guide.txt
1 removed
```

## When it says no

| Exit | Meaning |
| --- | --- |
| 0 | applied, or nothing to do |
| 1 | usage or I/O error |
| 2 | refusal — a deliberate no: drift, a foreign path, a containment violation |

A refused path is a row like any other, and the run exits 2. `--dry-run` reports the same classification a real run would act on, and writes nothing.

```console
$ proiectio write deploy.toml --dest ./site --owner site --dry-run
would refuse     bin/tool              (drifted) (from mapping /home/you/work/deploy.toml)
would skip       config/settings.toml
would link       current               -> releases/1.2.3
pass --force to touch them anyway, where the projection can still tell what it would replace
$ echo $?
2
$ proiectio write deploy.toml --dest ./site --owner site --force
overwrote  bin/tool              (exec)
skipped    config/settings.toml
linked     current               -> releases/1.2.3
2 written, 1 skipped
```

Permission lives on the invocation, never in the mapping: a symlink whose target leaves the destination needs `--allow-external-targets` on every run that projects it.

The full catalogue of refusals and how to resolve each is in [docs/refusals.md](docs/refusals.md).

## Machine output

`--output json|yaml|xml|csv` replaces the report with a machine-readable one, on every command.

```console
$ proiectio write shared.toml --dest ./share-dest --owner two --output csv
path,verdict,detail,shape,executable,target,owners,origin,phase
etc/motd,Skipped,,file,false,,"[""two""]","{""Mapping"":{""path"":""/home/you/work/shared.toml""}}",applied
```

## More

- [docs/mapping.md](docs/mapping.md) — the mapping-file reference.
- [docs/refusals.md](docs/refusals.md) — when proiectio says no.
- [docs/design.lex](docs/design.lex) — the model.
- [docs/cli-tour.lex](docs/cli-tour.lex) — the CLI contract.
- [docs/security.lex](docs/security.lex) — the trust split and symlink rules.

The library is [libproiectio](https://crates.io/crates/libproiectio) on crates.io, documented at [docs.rs/libproiectio](https://docs.rs/libproiectio).

MIT licensed; see [LICENSE](LICENSE).
