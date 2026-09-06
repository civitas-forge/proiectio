# Rendering and output

Standout renders human output in two passes:

```text
serializable data -> MiniJinja -> semantic [style] tags -> terminal style transform
```

Keep templates in files and style semantic tags with CSS:

```jinja
[title]Todos[/title]
{% for item in items %}
[{{ item.status }}]{{ item.title }}[/{{ item.status }}]
{% endfor %}
```

```css
.title { color: cyan; font-weight: bold; }
.done { color: green; }
.pending { color: yellow; }
```

`embed_templates!` and runtime template loading accept `.jinja`, `.jinja2`, `.j2`, `.stpl` and `.txt`. `embed_styles!` accepts CSS plus legacy YAML. A stylesheet filename supplies its theme name, which `.default_theme(name)` has to select. Prefer CSS and MiniJinja for new application code.

## Representation and style

"What is produced" and "does it carry color" are two settings. `--output` names a structured encoding only; with no `--output` the run renders the human template, which the flag cannot name. `--color` decides whether that rendered text carries escapes.

| `--output` | Result | Agent use |
| --- | --- | --- |
| *(absent)* | Template; color per `--color` | Normal user output |
| `term-debug` | Template with style tags preserved | Inspect style placement |
| `json`, `yaml`, `csv`, `ndjson` | Direct serialization; template skipped | Parse or assert on data |

`term`, `text`, `auto` and `xml` are gone; passing one is a usage error. `--color always` is what `--output term` meant and `--color never` is what `--output text` meant. `csv` takes one flat record or an array of them; a nested value is a render error unless the command declares a `CsvProjection`. `ndjson` writes one compact JSON object per line.

An unresolved style tag is a warning and an unstyled degrade, or a run failure under `strict_style_tags`. Structured modes skip injected template context.

Prefer `--output json` plus a parser whenever an agent needs facts rather than presentation. Use `--color never` when the rendered wording matters. Do not scrape ANSI output.

A failure under `json`, `yaml`, `csv` or `ndjson` is a diagnostic document on stdout, not prose on stderr. An `AppFailure` or `ExternalFailure` is the exception: it writes its verbatim bytes to stderr under every encoding and adds the document.

`--output-file-path=PATH` writes output to the file and suppresses duplicate stdout. `--no-pager` suppresses the pager a `#[dispatch(pageable)]` command and every help page are otherwise eligible for, which `<APP>_PAGER` or `PAGER` names. Applications can rename or disable each injected flag through `AppBuilder` (`output_flag`, `color_flag`, `pager_flag`, `output_file_flag`, and the `no_*` forms).

Read `crates/standout-render/src/output.rs`, `crates/standout-render/docs/topics/templating.md`, `crates/standout-render/docs/topics/styling-system.md`, and `docs/topics/output-modes.md` for the detailed surface. If prose says output mode is in `CommandContext`, follow the current Rust type instead: it is a render-layer concern, and a handler cannot branch on it.
