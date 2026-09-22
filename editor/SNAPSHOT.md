# Debug Snapshot

`analyze(request_json)` returns JSON with protocol `notist-debug-snapshot`.

The request contains `request_id`, `entry`, virtual source `files`, base64 WASM `binaries`, and package `dependencies`. Component resources are loaded by the HTML bundle and are not evaluator input.

Response fields:

- `source.files`: source ids, paths and text; `entry_source_id` selects the entry.
- `syntax`: tokens, statements and parser errors. Statements are `let`, `use`, `wasm`, `expression`, or `error`; ranges are UTF-8 byte offsets.
- `evaluation.events`: ordinary function-call events. They contain serialized output summaries; `node` and `name` are currently null.
- `evaluation.content`: the raw Item tree, including annotation and paragraph-boundary controls.
- `result`: formed Content JSON, raw exported bindings, diagnostics and counters. Content constraint diagnostics use stage `form` and code `content_constraint`.
- `platform`: target and elapsed time. Native/WASM comparison ignores this field.
- `truncated`: reserved output-budget field; it is currently always an empty array.

Every Content uses `{"item": name, "args": {...}, "attributes": {...}, "label": ..., "source": ..., "offset": ...}`. Known source ranges add `span: {source, start, end}` in UTF-8 bytes. Text stores `args.text`, sequences store `args.children`, paragraphs store `args.body`, and errors store `args.message` and `args.code`. Dict values use `{"dict": {...}}`; Lists are arrays; `none` is JSON null. Target values use `{"target": module, "labels": [...]}` and appear in a link Item's `args.target`.

The debugger displays syntax, ordinary evaluation events, raw Content and formed Content. Formation is shared with document queries and rendering. There is no per-rule trace, origin id, or complete event-to-source mapping. Raw and formed trees have separate node identities. Rendering errors belong to the HTML consumer.

`just web-compare` compares `evaluation.content`, `result.content` and `result.diagnostics` between native and browser evaluation.
