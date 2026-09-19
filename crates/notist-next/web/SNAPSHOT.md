# Debug Snapshot

`analyze(request_json)` returns JSON with protocol `notist-debug-snapshot`.

The request contains `request_id`, `entry`, virtual source `files`, base64 WASM `binaries`, and package `dependencies`. Component resources are loaded by the HTML bundle and are not evaluator input.

Response fields:

- `source.files`: source ids, paths and text; `entry_source_id` selects the entry.
- `syntax`: tokens, statements and parser errors. Statements are `let`, `use`, `wasm`, `expression`, or `error`; ranges are UTF-8 byte offsets.
- `evaluation.events`: ordinary function-call events. They contain serialized output summaries; `node` and `name` are currently null.
- `result`: Content JSON, exported bindings, diagnostics and counters. Item nodes contain `item`, `args`, `source`, and `offset`. Nested arguments retain tagged Dict/Content values.
- `platform`: target and elapsed time. Native/WASM comparison ignores this field.
- `truncated`: reserved output-budget field; it is currently always an empty array.

Content encodings are `{"text": ...}`, `{"sequence": [...]}`, `{"item": name, "args": {...}}`, and `{"error": ...}`. Dict values use `{"dict": {...}}`; `none` is JSON null.

There is no normalization stage, rule trace, origin id, or complete event-to-source mapping. The debugger displays syntax, ordinary evaluation events and resulting Content. Rendering errors belong to the HTML consumer, not language diagnostics.

`just web-compare` compares only `result.content` and `result.diagnostics` between native and browser evaluation.
