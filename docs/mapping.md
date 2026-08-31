# Mapping files

A mapping is a TOML file describing a set of files and symlinks to project onto a
destination directory. `version = 1` is required. Every table key is a path
relative to the destination.

Run one with `proiectio write deploy.toml --dest ./site --owner site`. The
[README](../README.md) covers the rest of the command surface.

## A complete mapping

```toml
version = 1

[files."config/settings.toml"]
contents = """
listen = ":8080"
"""

[files."bin/tool"]
source = "assets/tool.sh"
executable = true

[links."current"]
target = "releases/1.2.3"

[archives."vendor/"]
source = "assets/vendor.tar.gz"
strip = 1
```

The archive holds `vendor-1.4/LICENSE` and `vendor-1.4/lib/core.js`. Its members
expand into entries of their own:

```console
$ proiectio write deploy.toml --dest ./site --owner site --dry-run
would write      bin/tool              (exec)
would write      config/settings.toml
would link       current               -> releases/1.2.3
would write      vendor/LICENSE
would write      vendor/lib/core.js
```

## [files]

A `[files]` entry sets exactly one of `contents` and `source`.

- `contents` is an inline string, written as its literal bytes.
- `source` is a path to a file whose bytes are copied. A relative `source`
  resolves against the mapping file's own directory, not the working directory,
  so a mapping and its assets travel together.

Setting both, or neither, fails the load:

```console
$ proiectio write both.toml --dest ./site
Error: mapping /home/you/work/both.toml: files entry "a.txt" must set exactly one of `contents` and `source`
$ echo $?
1
```

`executable = true|false` sets the executable bit. Without it, a sourced entry
takes the bit from the source file and an inline entry takes the platform
default. The key overrides either:

```console
$ proiectio write exec.toml --dest ./site --owner site --dry-run
would write      a-inline
would write      b-inline-exec  (exec)
would write      c-sourced      (exec)
would write      d-sourced-off
```

`c-sourced` and `d-sourced-off` name the same executable source file; only
`d-sourced-off` sets `executable = false`.

## [links]

A `[links]` entry sets `target`. The string is written to disk verbatim and
resolves relative to the link's parent directory, as any symlink does. Nothing
needs to exist at the far end — a dangling target is allowed, and the manifest
hashes the target string rather than what it points at.

A target resolving inside the destination needs no flag. One that leaves the
destination — an absolute path, or a relative path that climbs out — is refused
unless the invocation passes `--allow-external-targets`:

```console
$ proiectio write toolchain.toml --dest ./link-dest --owner site
would refuse     toolchain  (external target) -> /opt/toolchains/rust-1.80 (from mapping /home/you/work/toolchain.toml)
pass --allow-external-targets to write them
$ echo $?
2
$ proiectio write toolchain.toml --dest ./link-dest --owner site --allow-external-targets
linked     toolchain  -> /opt/toolchains/rust-1.80
1 written, 0 skipped
$ echo $?
0
```

The flag is permission granted per invocation and is never recorded, so every
run of that mapping needs it — including a re-run where disk already matches.
[docs/dev/security.lex](dev/security.lex) section 3 states how a target is graded
in-dest or external.

## [archives]

An `[archives]` entry sets `source` and an optional `strip = N`. The key is a
path prefix the members expand under.

Supported formats are `.tar`, `.tar.gz`/`.tgz`, `.tar.zst` and `.zip`. The
extension picks the decoder; a wrong extension is an error, not a guess:

```console
$ proiectio write wrongext.toml --dest ./site
Error: archive /home/you/work/vendor.zip does not decode as a zip archive: invalid Zip archive: Could not find EOCD
$ echo $?
1
```

Members expand at plan time into ordinary file and link entries under the key
prefix. From there nothing is archive-specific: each entry is hashed, recorded
in the manifest, reported for drift, and removable on its own.

```console
$ proiectio status --dest ./site
clean    bin/tool
clean    config/settings.toml
clean    current
clean    vendor/LICENSE
clean    vendor/lib/core.js
$ proiectio rm vendor/lib/core.js --dest ./site --owner site
removed    vendor/lib/core.js
1 removed
```

`strip = N` drops N leading path components from each member. A member the strip
leaves with no path is skipped and reported as a dropped row:

```console
$ proiectio write mixed.toml --dest ./site --owner v --dry-run
would write      vendor/README
dropped          NOTES          (no path left after strip 1) (from archive /home/you/work/assets/mixed.tar.gz into vendor, named by mapping /home/you/work/mixed.toml)
```

A strip that would consume every member fails the load:

```console
$ proiectio write overstrip.toml --dest ./site
Error: archive /home/you/work/assets/vendor.tar.gz: strip 3 left nothing to project (2 members dropped)
$ echo $?
1
```

An archive reached any other way is not extracted. Inside a `--tree` source
directory an archive is just a file, copied verbatim. Extraction happens only
where it is asked for: an `[archives]` entry, or `--tree` pointed at the archive
itself.

## Path rules

Keys must be relative and must still lie inside the destination after lexical
normalization. A key that climbs out is refused:

```console
$ proiectio write escape.toml --dest ./site --owner site
Error: refusing paths that violate containment: ../outside.txt (from mapping /home/you/work/escape.toml)
$ echo $?
2
```

Absolute keys, `.` or empty components, and spellings Windows resolves somewhere
other than an ordinary file under the destination are refused the same way.
[docs/dev/security.lex](dev/security.lex) section 2 lists them all. The verdict is
lexical, so a mapping gets the same answer on every host.

Two keys may not overlap: one location is planned once.

```console
$ proiectio write overlap.toml --dest ./site --dry-run
would refuse     a/b    (tree conflict) (with a/b/c) (from mapping /home/you/work/overlap.toml)
would refuse     a/b/c  (tree conflict) (with a/b) (from mapping /home/you/work/overlap.toml)
$ echo $?
2
```

Exit 2 is a refusal; see [docs/refusals.md](refusals.md) for what each one means
and how to clear it.

## Defaults and configuration

The `--owner` default and the source-size bound come from configuration:

```console
$ proiectio config list
max_source_size = 524288000
owner = "default"
```

`proiectio config get`, `set` and `unset` read and write these two keys;
[docs/dev/cli-tour.lex](dev/cli-tour.lex) section 7 states the command and how
values resolve.

`max_source_size` is how many bytes one write may read from its sources,
default 500 MiB. An archive counts what it expands to rather than its size on
disk, except a zip, whose file must also fit because its index is read whole
before any member. `--owner` and `--max-source-size` on the command line always
win over the configured value.
