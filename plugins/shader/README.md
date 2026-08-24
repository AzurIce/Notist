# shader plugin package

This directory is the distributable Notist Wasm plugin package:

- `plugin.json` declares the Wasm module and element schema.

This package predates the self-describing component ABI: it is a leftover
package built against the initial raw ABI of 2026-08-18, where the element
schema is carried by the manifest's `interfaces.semantic` block and
`semantic.wasm` exports no `init`. New-style component manifests do not carry
a semantic interface; their schema is collected from the component's `init`
export at load time (see `plugins/component-echo`).
- `semantic.wasm` is a small core WebAssembly module implementing the plugin ABI.
- `semantic.wat` is the human-readable WAT source for `semantic.wasm`.
- `assets/shader.js` and `assets/shader.css` are the WebGPU/Web Component assets for HTML targets.

The Wasm module is loaded at runtime by `notist-plugin-host` through Wasmtime.
The HTML target uses WebGPU/WGSL (the browser standard implemented by `wgpu`) to render the shader canvas.

Load it from `Notist.toml`:

```toml
[plugins.shader]
path = "../plugins/shader"
```

Regenerate `semantic.wasm` from WAT with `wat`:

```bash
wat::parse_str(include_str!("semantic.wat"))
```
