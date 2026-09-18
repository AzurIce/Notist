# Debug Snapshot

`analyze(request_json)` returns JSON with protocol `notist-debug-snapshot`.

Request fields: `request_id`, `entry` (virtual source path), `files` (path to source text), `binaries` (path to standard base64), and `dependencies` (package identity to dependency-alias/identity map). Component resources are renderer data and are not required by the evaluator request. The native `--request` command produces this input from a package. Hosts own file/resource loading; the evaluator uses the same virtual graph on native and WASM.

Response fields:

- `source.files`: source ids, paths and text; `entry_source_id` selects the entry.
- `syntax`: tokens, statements, errors; ranges are UTF-8 byte offsets. Statements are `let`, `use`, `wasm`, `expression`, or `error`. Lambda parameters contain names, types and defaults.
- `evaluation.events`: ordinary function-call events, with source ranges and output summaries.
- `result`: Content JSON, exported bindings, diagnostics and counters. Item nodes contain `item`, `args`, `source`, `offset`. Nested arguments retain tagged Dict/Content values.
- `platform`: target and elapsed time; excluded from native/WASM equality comparisons.
- `truncated`: output budget indicators.

There is no normalization stage. The debugger displays syntax, evaluation and resulting content. Rendering errors belong to the HTML consumer, not these language diagnostics.
