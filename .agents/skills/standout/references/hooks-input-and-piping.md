# Hooks, inputs, and piping

Use these extension points to keep cross-cutting shell concerns out of handlers.

## Hooks

Hooks run in registration order and stop on the first `HookError`:

- `pre_dispatch(&ArgMatches, &mut CommandContext)` validates, authenticates, or inserts request state before the handler.
- `post_dispatch(&ArgMatches, &CommandContext, serde_json::Value)` transforms the serialized handler data before rendering.
- `post_output(&ArgMatches, &CommandContext, RenderedOutput)` transforms or observes text/binary/silent output after rendering.

Attach hooks through `command_with`, derive attributes, or
`.hooks(path, Hooks::new()...)` — one phase through one of those, never two, which
is a configuration error at `build()`. A `#[dispatch]` key names a `fn`, not a
closure, so a hook that needs a value the composition root built reads it from
`ctx.app_state.get::<T>()`. Pre-dispatch and post-output hooks receive the
deepest subcommand's `ArgMatches`, so a root-level argument they read has to be
`.global(true)`. Keep reusable behavior in the CLI-free library,
keep handlers as adapters, and use hooks only when the shell concern crosses
commands or pipeline phases.

## Declarative inputs

An `InputChain<T>` tries sources in order, validates the resolved value, and runs as pre-dispatch configuration:

```rust
use standout::cli::CommandContextInput;
use standout::input::{ArgSource, InputChain, StdinSource};

let chain = InputChain::<String>::new()
    .try_source(ArgSource::new("title"))
    .try_source(StdinSource::new())
    .validate(|s| !s.trim().is_empty(), "title cannot be empty");

// On a clap enum variant: #[dispatch(pure, inputs = crate::handlers::add_inputs)],
// where `add_inputs` is a `fn(CommandConfig<H>) -> CommandConfig<H>` calling
// `config.input("title", chain)`. Without a variant:
let app = App::builder()
    .command_with("add", handlers::add_Handler, |cfg| cfg.input("title", chain))?;

// In the handler:
let title: &String = ctx.input("title")?;
```

Sources include clap args/flags, environment, stdin, clipboard, configuration
(`ConfigSource::new(Option<T>)`, the idiom for "flag beats config key"),
defaults, editor, and interactive prompts. They are not process-global: stdin,
clipboard and the prompt responder travel on an `InputSources` value passed into
`run_with`, which `TestHarness` builds from `.piped_stdin`, `.clipboard` and
`.prompts`. A hand-written `InputCollector` wrapping stdin must implement
`bind_sources` over `sources.stdin_arc()`, or it keeps reading the real process
stdin under a harness that piped something else.

## Piping

Pipes are post-output hooks for text output:

- `.pipe_to(command)` sends plain text to a command and preserves original output.
- `.pipe_through(command)` replaces output with the command's stdout.
- `.pipe_to_clipboard()` consumes output after copying it.

Multiple pipes chain in registration order. Binary and silent output pass through unchanged. Commands execute through the platform shell, so keep command strings fixed; never interpolate untrusted input.

Inspect `crates/standout-dispatch/src/hooks.rs`, `crates/standout-input/docs/topics/framework-integration.md`, and `crates/standout-pipe/docs/topics/piping.md` for detailed APIs.
