# Notist Language Prototype

A self-contained `.notc` evaluator with first-class Content and Item values, local packages, Rust-style module paths, registered WASM functions, and a separate HTML consumer.

```sh
cargo test -j4 -p notist-next -- --test-threads=4
cargo run -j4 -p notist-next -- examples-path --html
cargo run -j4 -p notist-next -- crates-next/notist-next/examples/demo --bundle /tmp/notist-demo
```

Use `crates-next/notist-next/examples/demo` as `examples-path`. Serve a bundle over HTTP to load its ES modules. `--snapshot` emits debug JSON; `--request` emits the portable input used by the browser debugger.

## Packages

`Notist.toml` declares `[package] name` and `[dependencies] alias = { path = "..." }`. No package entry or component declaration fields: `docs/README.notc` is the root module, and component entries are discovered by convention. `.not` files participate in module discovery but markup lowering is explicitly unavailable in this prototype.

`vault` always names the defining package root. `self` and repeated `super` name the current and parent modules. Dependency aliases name dependency roots. Files are discovered automatically; directory modules can be implicit. Spaces map one-to-one to `_`; numeric path segments and Unicode alphanumeric segments are accepted. Conflicting logical module paths are errors. Other punctuation is rejected.

```notc
use vault::helpers::{self as helpers, format};
use vault::2026_notes as notes;
use theme::colors::*;
wasm "../wasm/semantic.wasm";
let diagram = (source: String, theme: String = "default") =>
    item("mermaid", (source: source, theme: theme));
diagram("graph LR; A --> B");
```

Top-level `let` and registered WASM functions are exported; imported bindings are local. A module initializes once per evaluation. Cyclic imports and duplicate bindings are diagnosed. Functions have lexical capture, self-recursion, runtime parameter types, defaults, named arguments, and trailing Content. Default expressions see preceding parameters. There is no overloading or implicit rewrite stage.

## Content and Rendering

`[]` constructs Content. `()` / `(a,)` / `(a,b)` construct Lists; `(:)` / `(key: value)` construct Dicts. `none` is distinct from empty Content and Error Content. `item(name, args)` returns Item with structured arguments and source location. `.name` and `.args` inspect an Item. Content accepts Items; Item arguments may contain nested Content. Error nodes survive alongside other output.

The HTML consumer maps core paragraph/section/heading/list/list-item/strong/em nodes to native elements. Other names map to `notist-<name>`. Each field is JSON-encoded into `data-<field>`; unsupported tag/attribute names are rendering errors. The browser renderer passes the original structured args to `render(args, { renderContent })`; Content fields are rendered explicitly by the component. The component module's default export is a custom element class. The host owns registration. Synchronous and asynchronous `render` failures are local diagnostics; later event-handler failures remain the component's responsibility.

`components/` is copied intact, under package-specific directories, through the complete dependency graph. `components/<name>.js` and `components/<name>/main.js` are component entry forms; other files are resources. Component names must be lowercase ASCII letters, digits, or `-`, and name conflicts fail package loading. Components ship browser-ready modules and relative assets. CSS is unrestricted for native/light DOM; Shadow DOM components expose their own theme hooks.

Discovery is limited to those two entry forms. Both forms declaring the same name are a conflict, including across packages. Put helper JS in component subdirectories or a shared directory without `main.js`; every top-level `.js` is an entry. Component names are not normalized like module filenames. The loader generates the `components.json` renderer map automatically.

## WASM

Wasmi 2.0 runs import-free, fuel-limited instances. Exports: `memory`, `alloc(i32)->i32`, `notist_register(i32,i32)->i64`, and functions with the same pointer/length ABI. Return pointer occupies the high 32 bits; length occupies the low 32. UTF-8 JSON input is an argument array.

Registration returns ordinary JSON, independently from value decoding:

```json
{"functions":{"echo":{"export":"echo","params":[{"name":"source","type":"String","default":""}],"result":"String"}}}
```

Registration is cached per binary path per evaluation and bindings are installed atomically after validation. Function calls use fresh instances. Result values use JSON primitives, arrays, `{"dict":{...}}`, `{"item":"name","args":{...}}`, `{"text":"..."}`, `{"sequence":[...]}`, or `{"error":"..."}`. Functions/modules cannot cross the ABI. Registered types currently use the built-in type vocabulary; exported user-defined types are not implemented.

## Scope

The demo tests transitive components and WASM registration. Mermaid/Shader components display source text; they do not bundle those rendering engines. The core package is ordinary `.notc`. Registry/version resolution, public/private declarations, complete `.not` lowering, static type inference, editor integration, and custom WASM type definitions remain outside this prototype. No changes are required in sister editors until they adopt this language.

`cargo run -j4 -p notist-next --example build_fixture` regenerates the WASM fixture and portable package request. `just web-build` and `just web-fixtures` build the browser evaluator; `just web-compare` compares only native/browser `result.content` and `result.diagnostics`.
