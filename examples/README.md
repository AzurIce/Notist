# Notist Language Examples

A `.not`/`.notc` evaluator with one Content value type, uniform Item trees, shared document formation, local packages, registered WASM functions, and a separate HTML consumer.

```sh
cargo test -j4 -p notist-analysis -- --test-threads=4
cargo run -j4 -p notist-cli -- eval examples-path --html
cargo run -j4 -p notist-cli -- eval examples/demo --bundle /tmp/notist-demo
```

Use `examples/demo` as `examples-path`. Serve a bundle over HTTP to load its ES modules. `--snapshot` emits debug JSON from `notist-analysis::debug`; `--request` emits the portable input used by the browser debugger.

## Packages

`Notist.toml` declares `[package] name` and `[dependencies] alias = { path = "..." }`. No package entry or component declaration fields: `docs/README.not` or `docs/README.notc` is the root module, and component entries are discovered by convention. A `.not` file is the implicit outer Content literal; its text, `#` interpolations, styles, nested Content, and `[[module::"Label"]]` wikilinks use the same evaluator as `.notc` Content literals.

`vault` always names the defining package root. `self` and repeated `super` name the current and parent modules. Dependency aliases name dependency roots. Files are discovered automatically; directory modules can be implicit. Filename segments preserve case, Unicode letters/numbers, and underscores; each other character maps to `_`. Numeric-leading names and reserved words receive an `_` prefix, so logical module names follow binding identifier rules. Names without any identifier characters are rejected. Conflicting logical module paths are errors naming both source files. Explicit code names are validated, never normalized. A code target `module::"Parent"::"Label"` and a Markup wikilink `[[module::"Parent"::"Label"]]` both produce a reference value. LabelPath matching and explicit `@(label: "...")` attributes follow [the Item reference rules](../docs/designs/item/README.not).

```notc
use vault::helpers::{self as helpers, format};
use vault::_2026_notes as notes;
use theme::colors::*;
wasm "../wasm/semantic.wasm";
let diagram = (source: String, theme: String = "default") =>
    item("mermaid", (source: source, theme: theme));
diagram("graph LR; A --> B");
```

Top-level `let` and registered WASM functions are exported; imported bindings are local. A module initializes once per evaluation. Cyclic imports and duplicate bindings are diagnosed. Functions have lexical capture, self-recursion, runtime parameter types, defaults, named arguments, and trailing Content. Default expressions see preceding parameters. Ordinary calls execute during evaluation; document formation processes their output without invoking user rewrite hooks.

## Content and Rendering

`[]` always constructs a seq Item, including empty and singleton literals. Its value type is Content, shared with text, space, paragraph, link, error and custom Items. `()` constructs Unit; `(,)`, `(a,)` and `(a,b)` construct Lists; `(a)` groups an expression; `(:)` and `(key: value)` construct Dicts. Unit is distinct from an empty seq. `item(name, args)` returns Content. `.name`, `.args` and `.attributes` inspect its root Item. Structured arguments may contain nested Content; error Items survive alongside other output.

Evaluation produces a raw tree. Shared document formation groups inline runs into paragraphs, resolves annotations and validates content slots. A leading prose annotation targets the whole paragraph; an explicit seq retains its own attribute boundary. Unknown Items are opaque blocks. Packages declare models with `define_element("badge", true, (body: "inline"))` or `define_element("callout", false, (body: "flow"))`. Markup list entries are nested list-item Items; their first formed block is main content and remaining blocks are details.

The HTML consumer maps core paragraph/section/heading/list/list-item/strong/em nodes to native elements. Other names map to `notist-<name>`. Each field is JSON-encoded into `data-<field>`; unsupported tag/attribute names are rendering errors. The browser renderer passes the original structured args to `render(args, { renderContent })`; Content fields are rendered explicitly by the component. The component module's default export is a custom element class. The host owns registration. Synchronous and asynchronous `render` failures are local diagnostics; later event-handler failures remain the component's responsibility.

`components/` is copied intact, under package-specific directories, through the complete dependency graph. `components/<name>.js` and `components/<name>/main.js` are component entry forms; other files are resources. Component names must be lowercase ASCII letters, digits, or `-`, and name conflicts fail package loading. Components ship browser-ready modules and relative assets. CSS is unrestricted for native/light DOM; Shadow DOM components expose their own theme hooks.

Discovery is limited to those two entry forms. Both forms declaring the same name are a conflict, including across packages. Put helper JS in component subdirectories or a shared directory without `main.js`; every top-level `.js` is an entry. Component names are not normalized like module filenames. The loader generates the `components.json` renderer map automatically.

## WASM

Wasmi 2.0 runs import-free, fuel-limited instances. Exports: `memory`, `alloc(i32)->i32`, `notist_register(i32,i32)->i64`, and functions with the same pointer/length ABI. Return pointer occupies the high 32 bits; length occupies the low 32. UTF-8 JSON input is an argument array.

Rust plugins use ordinary functions with SDK attributes:

```rust
use notist_plugin_sdk::{func, init_plugin};

init_plugin!(echo, add);

#[func]
pub fn echo(source: Option<String>) -> Option<String> { source }

#[func(defaults(right = 1))]
pub fn add(left: i64, right: i64) -> i64 { left + right }
```

`init_plugin!` lists the annotated function paths to register. Rust calls still supply every argument. Notist parameter and default rules are defined in `docs/designs/value-and-types.not`; `Option<T>` maps to `T?`. Build plugins as release cdylibs for `wasm32-unknown-unknown`. `just test-plugin-sdk` builds the SDK example and tests it through the evaluator.

Registration and values use the shared structured types in `notist-model::abi`. Registration is cached per binary path per evaluation and bindings are installed atomically after validation. Function calls use fresh instances. The ABI's tagged value encoding is separate from renderer/debug JSON. Content is one Item with tagged values in its arguments and attributes. Target values can cross the ABI; functions and modules cannot. Returned Items receive host-side call locations. `init_plugin!(elements = models; foo, bar)` additionally registers the name-to-ElementModel map returned by `models()`. Executable formation hooks and exported user-defined types are not implemented.

## Scope

The demo tests transitive components and WASM registration. Mermaid/Shader components display source text; they do not bundle those rendering engines. Registry/version resolution, public/private declarations, static type inference, and custom WASM type definitions remain outside this prototype.

`cargo run -j4 -p notist-analysis --example build_fixture` regenerates the WASM fixture and portable package request. `just web-build` and `just web-fixtures` build the browser evaluator; `just web-compare` compares native/browser `evaluation.content`, `result.content` and `result.diagnostics`.
