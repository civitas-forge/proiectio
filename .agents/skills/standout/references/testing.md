# Testing Standout applications

Choose the smallest boundary that covers the behavior:

| Level | Covers | Tool |
| --- | --- | --- |
| Core | Validation, filtering, state transitions, persistence | Library interface |
| Adapter | CLI-to-core mapping and returned view DTOs | Direct typed handler call |
| Integration | clap through handler, hooks, and rendering | `standout_test::TestHarness` |
| End to end | Real process, PTY, signals, build/link behavior | `TestHarness::run_process` / `run_pty` |

Test filtering, validation, state transitions, and persistence directly through
the CLI-free library interface. With `#[handler]`, call the preserved typed
function to test flag/argument mapping and CLI-owned returned data rather than
constructing `ArgMatches` for the generated wrapper.

Use `TestHarness` when command registration, input/environment seams, templates, or output modes matter:

```rust
use standout::Representation;
use standout_test::{serial, TestHarness};

#[test]
#[serial]
fn list_is_machine_readable() {
    let result = TestHarness::new()
        .fixture("todos.txt", "buy milk\n")
        .env("TODO_FILE", "todos.txt")
        .terminal_width(80)
        .output_mode(Representation::Json)
        .run(&app(), cli_command(), ["tdoo", "list"]);

    result.assert_success();
    let value: serde_json::Value = serde_json::from_str(result.stdout()).unwrap();
    assert_eq!(value["total"], 1);
}
```

The harness **injects** destination facts rather than detecting them. Defaults
are non-terminal, non-color-capable streams, so output is plain without saying
so; `.color(ColorPolicy::Always)` emits the real escapes a user sees and
`.color_capable_terminal()` emulates a terminal under `Auto`. Set the facts a
command reads with `.stdout_is_terminal(bool)`, `.terminal_width(n)`,
`.icon_mode(...)` and `.color_scheme(...)`; a real terminal answers only in
`run_process()` / `run_pty()`. Stdin, clipboard and prompts travel on
`InputSources` through `.piped_stdin`, `.clipboard` and `.prompts`.

`TestResult` distinguishes every outcome, `Silent` included, and adds
`result()`/`results()` for the run's data whatever representation ran,
`warnings()`, `diagnostic()`/`expect_diagnostic()` for the stdout document a
structured failure writes, `unresolved_tag_names()`, `delivery()` and
`assert_schema_snapshot(name)`.

Only a test that sets env vars or cwd needs `#[serial]`: those are the remaining
process-global seams. Destination facts and input sources are per-run values.

Use JSON to assert returned shape, the default (or `ColorPolicy::Never`) for
rendered strings, and `Representation::TermDebug` for style tags. Run the suite
once under `STANDOUT_STRICT_STYLE_TAGS=1`. See `crates/standout-test/src/lib.rs`,
`docs/topics/testing.md`,
`crates/todo-example/todo-core/src/store.rs`, and
`crates/todo-example/tdoo/src/app.rs`.
