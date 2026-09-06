# Project map and API gotchas

## Ownership map

| Location | Responsibility |
| --- | --- |
| `crates/standout` | Framework facade, app builder, clap integration, help/topics |
| `crates/standout-dispatch` | Handler contract, context/state, hooks, dispatch primitives |
| `crates/standout-render` | MiniJinja, styles/themes, output modes, tabular rendering |
| `crates/standout-input` | Input chains, sources, readers, interactive responders |
| `crates/standout-pipe` | Post-output command and clipboard piping |
| `crates/standout-macros` | Handler, dispatch, embedding, tabular, and seeker macros |
| `crates/standout-bbparser` | Semantic style-tag parsing and transformation |
| `crates/standout-seeker` | In-memory typed filtering, ordering, and query parsing |
| `crates/standout-test` | In-process application test harness |
| `crates/todo-example/todo-core` | CLI-free worked library: domain behavior and JSON persistence |
| `crates/todo-example/tdoo` | Binary-only worked CLI: app wiring, adapters, views, assets, and harness tests |

Start broad framework changes at `crates/todo-example/README.md`, then use
`todo-core/src/lib.rs` for the library interface and `tdoo/src/app.rs` for CLI
assembly. Use `docs/SUMMARY.md` to locate guides and topics. Follow a claim into
the owning crate's public types and integration tests before copying it.

## Drift checks

Common copied examples can target older APIs. Confirm these current contracts:

- `App::run(...) -> bool`; it does not return `Option<ArgMatches>`. Use `run_with(cmd, args, TargetProperties::detect(), InputSources::from_process())` and match `DispatchResult::NoMatch` on `into_outcome()` when fallback needs matches. `run_to_string` and `dispatch_from` are gone.
- `CommandContext` has `command_path`, `app_state`, and `extensions`; no representation. `OutputMode` is two types now, `Representation` and `StyleMode`, with `ColorPolicy` as the input that decides the style mode.
- Binary handler output is `Output::Binary { data, filename }`, not a tuple variant.
- `#[handler]` preserves the typed function and generates `name__handler`; wire the wrapper and unit-test the original.
- `#[derive(Dispatch)]` maps to `handlers::name`; add `#[dispatch(pure)]` for a `#[handler]`-generated wrapper.
- `TestHarness` injects destination facts rather than detecting them; only a test that sets env or cwd is serial.
- Structured output bypasses templates, so template fixes cannot change JSON/YAML/CSV/NDJSON. XML is gone.
- `AppBuilder::command_with` takes the `<name>_Handler` unit struct; the `GroupBuilder` form the `Dispatch` derive uses takes the `__handler` fn.
- JSON and YAML keys follow declaration order, so a `HashMap` in a view struct serializes differently between runs. Use a struct, `BTreeMap` or `IndexMap`.
- Domain serialization and CLI structured output are separate interfaces. Map
  domain values into CLI-owned view DTOs rather than serializing persistence
  types directly.
- Embedded paths are resolved at compile time; debug hot reload depends on the original path remaining available.

Do not infer a crate's role from its name. In particular, `standout-seeker` is a query engine, not a file/resource resolver.
