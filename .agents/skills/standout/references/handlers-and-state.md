# Handlers and state

Use the `#[handler]` macro to keep clap extraction outside the typed function:

```rust
use serde::Serialize;
use standout::cli::{CommandContext, Output};
use standout::handler;
use todo_core::{Todo, TodoFilter, TodoStore};

#[derive(Serialize)]
pub struct TodoView {
    pub id: u32,
    pub title: String,
    pub done: bool,
}

impl From<Todo> for TodoView {
    fn from(todo: Todo) -> Self {
        Self { id: todo.id, title: todo.title, done: todo.done }
    }
}

#[derive(Serialize)]
pub struct TodoListView {
    pub todos: Vec<TodoView>,
    pub total: usize,
}

#[handler]
pub fn list(
    #[flag] all: bool,
    #[ctx] ctx: &CommandContext,
) -> Result<Output<TodoListView>, anyhow::Error> {
    let store = ctx.app_state.get_required::<TodoStore>()?;
    let filter = if all { TodoFilter::All } else { TodoFilter::Pending };
    let todos: Vec<_> = store.list(filter).into_iter().map(TodoView::from).collect();
    let total = todos.len();
    Ok(Output::Render(TodoListView { todos, total }))
}
```

Supported parameter annotations are `#[flag]` for booleans, `#[arg]` for typed required/optional/vector values, `#[ctx]`, and `#[matches]`. `name = "cli-name"` overrides the inferred flag or argument name.

The macro preserves `list(all, ctx)` and generates `list__handler(matches, ctx)` plus argument-verification metadata. Wire the generated wrapper; call the typed function in unit tests:

```rust
let Output::Render(result) = list(false, &ctx).unwrap() else {
    panic!("expected rendered data");
};
assert_eq!(result.total, result.todos.len());
```

Handlers are CLI adapters, not the home of reusable application behavior. A
handler may map a flag to a library type, obtain a dependency from app state,
call the library, and map the result to a CLI-owned view DTO. Validation,
filtering rules, and state transitions belong behind the library interface.

## Output contract

`Output<T>` has exactly these shapes:

- `Output::Render(data)` renders a template or serializes `data` in a structured mode.
- `Output::Silent` completes without output.
- `Output::Binary { data, filename }` returns bytes and a suggested filename.
- `Output::Artifact(artifact)` hands Standout the bytes, the destination and the success report.
- `Output::WithStatus { .. }`, built with `.with_exit_status(status)`, is any of the above plus the status the process leaves with. It is still a success; nothing becomes a diagnostic. This is the typed alternative to a `process::exit` after printing.

A failure that pins its own status and stderr bytes is `AppFailure::new(status, "verbatim stderr\n")`; `ExternalFailure` keeps its meaning for a status another program declared. A handler with a detail and a source position returns `Diagnostic::error(..).detail(..).range(..)`.

Do not branch presentation in a handler. `CommandContext` contains `command_path`, `app_state`, and per-dispatch `extensions`; it deliberately does **not** contain the representation. It does carry `ctx.warn(message)`, which is where a framework-level warning goes now that there is no thread-local collector, and `ctx.input_sources()`, which a chain resolved in a handler needs (`chain.resolve_from(matches, ctx.input_sources())`).

A command whose result accrues while it runs takes `results: &mut Results<E>`, calls `results.emit(event)?`, and returns `Summary::Render(..)`. Nothing else may reach stdout or stderr.

## State boundaries

Register long-lived values once with `.app_state(value)` and retrieve them by concrete type with `ctx.app_state.get_required::<T>()`. Use interior mutability when shared state must mutate.

Construct those dependencies before app assembly. Environment lookup and
configuration-file conventions belong in the CLI; pass explicit values such as
paths or URLs into the CLI-free library.

Inject request-only values in a pre-dispatch hook with `ctx.extensions.insert(value)` and retrieve them with `ctx.extensions.get_required::<T>()`. Declarative named inputs use a typed bag in extensions; access those through `CommandContextInput::input`, not directly.

For full signatures and verification behavior, inspect `crates/standout-dispatch/src/handler.rs`, `crates/standout-macros/src/handler.rs`, and `crates/standout/tests/handler_macro.rs`.
