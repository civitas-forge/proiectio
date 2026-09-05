# App and command wiring

App construction belongs in the CLI package. Build the whole shell environment
before dispatch: CLI-resolved dependencies, embedded presentation assets,
theme, commands, and hooks. Reusable libraries must not build or return a
Standout `App`.

```rust
use standout::cli::App;
use standout::{embed_styles, embed_templates};

let app = App::builder()
    .app_state(store)
    .templates(embed_templates!("src/templates"))
    .styles(embed_styles!("src/styles"))
    .default_theme("todo")
    .command_with("list", handlers::list_Handler, |cfg| {
        cfg.template_name("shared")
    })?
    .build()?;
```

`AppBuilder::command_with` is the secondary path, for a command with no clap
enum variant. It takes the `<name>_Handler` unit struct the `#[handler]` macro
generates beside the function, not the `__handler` wrapper. Dot-separated paths
register nested commands. A template name is only needed when the command does
not render the template convention already names for it.

## Derive wiring

Prefer `#[derive(Dispatch)]` when clap variants map to handlers by convention:

```rust
#[derive(clap::Subcommand, standout::cli::Dispatch)]
#[dispatch(handlers = crate::handlers)]
enum Commands {
    #[dispatch(pure)]
    List,
    #[dispatch(pure, template_name = "shared", inputs = crate::handlers::add_inputs)]
    Add,
}

let app = App::builder()
    .commands(Commands::dispatch_config())?
    .build()?;
```

By default, `List` maps to `handlers::list` and registers under its kebab-case
name, so `ListUnits` is `list-units`. Mark a variant `#[dispatch(pure)]` when its
function uses `#[handler]`; the derive then selects `handlers::list__handler`.
Variant attributes also attach `template_name`, hooks, nested dispatch, defaults,
pipes and `pageable`. Anything on `CommandConfig` with no key of its own — an
input chain, a `CsvProjection`, a confirmation — goes in the function
`#[dispatch(inputs = path)]` names, which receives and returns the whole
`CommandConfig`.

A variant holds one path per phase, and a phase registered both here and through
`AppBuilder::hooks` is a configuration error at `build()`.

## Running and partial adoption

`App::run(command, args)` parses, dispatches, prints, and returns `true` when Standout handled the command. It returns `false` for an unmatched command:

```rust
if !app.run(Cli::command(), std::env::args()) {
    run_legacy_path();
}
```

`run_emitted` is `run` up to the exit, returning a `ProcessOutcome` whose
`status` is what the process should leave with. Use `run_with(cmd, args,
TargetProperties::detect(), InputSources::from_process())` when code needs the
outcome without writing either stream; it returns a `CompletedRun`, which wraps
the dispatch outcome plus framework warnings. Match `result.into_outcome()` as
`DispatchResult::{Handled, Binary, Artifact, Silent, Error, NoMatch}` and include
a wildcard because the enum is non-exhaustive.

Embedded resources are compile-time bundles; debug builds re-read the original
source path when available. A template name carries no extension: the registry
resolves it against `.jinja`, `.jinja2`, `.j2`, `.stpl` and `.txt`, in that
priority order. With no name, the template is the registration path with `.`
replaced by `/`.

A theme comes from a stylesheet registry with a named default: `.styles(...)`
together with `.default_theme(name)`. A registry with no `.default_theme` resolves
to no theme at all, and `.styles(...)` beside `.theme(...)` is a `SetupError`.
`AppBuilder::strict_style_tags(true)`, or `STANDOUT_STRICT_STYLE_TAGS=1`, turns an
unresolved style tag from a warning into a run failure.

Evidence: `crates/todo-example/tdoo/src/app.rs`,
`crates/standout/src/cli/builder/`, and
`crates/standout-macros/src/dispatch.rs`.
