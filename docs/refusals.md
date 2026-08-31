# When proiectio says no

Exit 0 means the run applied, or had nothing to do. Exit 1 is a usage or I/O
error. Exit 2 is a refusal: acting would touch something the projection does not
own or cannot verify. A plan containing any refusal is refused whole and writes
nothing. A refusal met during apply — the disk changed between planning and
acting — stops the run there, and the report carries the rows applied before
it. The refusal message names the flag that overrides it, where a flag exists.

## The status vocabulary

`status` reads the manifest, classifies every recorded path, and writes nothing.
Four verdicts:

- `clean` — disk matches the record: bytes, kind, executable bit.
- `drifted` — a recorded path was edited on disk.
- `missing` — a recorded path is gone from disk.
- `foreign` — a path is on disk and absent from the manifest.

```console
$ echo edited >> ./site/bin/tool
$ rm ./site/current
$ proiectio status --dest ./site
drifted  bin/tool
clean    config/settings.toml
missing  current
```

Plain `status` always exits 0 — the report is the product. `--check` exits 2 when
any row is not clean, the same 2 a dry run spends on the same finding.

```console
$ proiectio status --dest ./site --check
drifted  bin/tool
clean    config/settings.toml
missing  current
$ echo $?
2
```

The full rules live in [docs/dev/cli-tour.lex](dev/cli-tour.lex) sections 2–4 and
[docs/dev/design.lex](dev/design.lex) section 2.

## Someone edited a projected file (drifted)

```console
$ proiectio write deploy.toml --dest ./site --owner site --dry-run
would refuse     bin/tool              (drifted) (from mapping /home/you/work/deploy.toml)
would skip       config/settings.toml
would link       current               -> releases/1.2.3
pass --force to touch them anyway, where the projection can still tell what it would replace
$ echo $?
2
```

Three ways out:

- Adopt the edit — copy the on-disk contents into the mapping entry or its
  source file, then re-run the write.
- Overwrite the edit — re-run with `--force`.
- Restore the file by hand to what the projection wrote, then re-run.

```console
$ proiectio write deploy.toml --dest ./site --owner site --force
overwrote  bin/tool              (exec)
skipped    config/settings.toml
linked     current               -> releases/1.2.3
2 written, 1 skipped
```

`rm` refuses a drifted file the same way, and `--force` removes it anyway.

## A file is in the way (foreign)

```console
$ echo "theirs" > ./foreign-dest/motd.txt
$ proiectio write claim.toml --dest ./foreign-dest --owner site
would refuse     motd.txt  (foreign) (from mapping /home/you/work/claim.toml)
no flag overrides this: remove the paths by hand to let the projection write them — for a block, the marker region rather than the container holding it
$ echo $?
2
```

Remove the path by hand, then re-run the write. The projection acts only on what
its manifest records.

## A symlink pointing out of the destination

```console
$ proiectio write toolchain.toml --dest ./link-dest --owner site
would refuse     toolchain  (external target) -> /opt/toolchains/rust-1.80 (from mapping /home/you/work/toolchain.toml)
pass --allow-external-targets to write them
$ echo $?
2
```

Pass `--allow-external-targets` to permit it.

```console
$ proiectio write toolchain.toml --dest ./link-dest --owner site --allow-external-targets
linked     toolchain  -> /opt/toolchains/rust-1.80
1 written, 0 skipped
```

The flag is per-invocation permission, never recorded state: every run of that
mapping needs it, including a run where disk already matches and status reports
clean. [docs/dev/security.lex](dev/security.lex) section 3 grades targets.

## A path that leaves the destination

A key that normalizes outside the destination is refused before anything is
planned.

```console
$ proiectio write escape.toml --dest ./site --owner site
Error: refusing paths that violate containment: ../outside.txt (from mapping /home/you/work/escape.toml)
$ echo $?
2
```

Rewrite the key as a path inside the destination. A key beneath a symlinked
ancestor is refused the same way: remove the link, or project to a path that does
not pass through one. [docs/dev/security.lex](dev/security.lex) section 2 states the
grading rules.

## Two owners, one destination

Two owners may hold one path while they write identical bytes. The second write
joins the entry as a second owner and reports `skipped`.

```console
$ proiectio write shared.toml --dest ./share-dest --owner two
skipped    etc/motd
1 unchanged
```

`rm` under one owner releases that owner from the entry and leaves the file for
the remaining owner.

```console
$ proiectio rm --dest ./share-dest --owner one
released   etc/motd
1 released
$ ls ./share-dest/etc
motd
```

A write whose content differs from what another owner holds refuses as an owner
conflict, naming the holders.

```console
$ proiectio write conflict.toml --dest ./share-dest --owner three --dry-run
would refuse     etc/motd  (owner conflict) (held by two) (from mapping /home/you/work/conflict.toml)
$ echo $?
2
```

Write the same bytes as the holder, or project to a different path.

## Missing paths

A recorded path gone from disk is not a refusal. The next write rewrites it.

```console
$ proiectio write deploy.toml --dest ./site --owner site
skipped    config/settings.toml
linked     current               -> releases/1.2.3
1 written, 1 skipped
```

`rm` forgets the record and unlinks nothing.

```console
$ rm ./site/config/settings.toml
$ proiectio rm --dest ./site --owner site
forgot     config/settings.toml
removed    current               -> releases/1.2.3
1 removed, 1 forgotten
```

## Gating CI

Two gates, asking different questions:

```console
$ proiectio write deploy.toml --dest ./site --owner site --dry-run
$ proiectio status --dest ./site --check
```

A dry run asks whether one desired tree still applies: it refuses drift on the
paths it claims, plans a rewrite for a missing one, and ignores foreign paths
it does not claim. `status --check` holds the whole destination to its
manifest: any drifted, missing, or foreign row exits 2.

`--output json` and `--output csv` carry the same verdicts for tooling.

```console
$ proiectio write shared.toml --dest ./share-dest --owner two --output csv
path,verdict,detail,shape,executable,target,owners,origin,phase
etc/motd,Skipped,,file,false,,"[""two""]","{""Mapping"":{""path"":""/home/you/work/shared.toml""}}",applied
```

See the [README](../README.md) for the commands themselves and
[docs/mapping.md](mapping.md) for the mapping file.
