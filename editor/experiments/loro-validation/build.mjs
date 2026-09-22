import { build } from "esbuild";
import { copyFile } from "node:fs/promises";

const pkg = process.env.PROBE_LATEST ? "loro-crdt-latest" : "loro-crdt";
await build({ entryPoints: ["browser.js"], bundle: true, format: "esm", outdir: "dist", alias: { "loro-crdt": pkg } });
// Loro's browser entry loads this file via new URL(..., import.meta.url).
await copyFile(new URL(import.meta.resolve(`${pkg}/browser/loro_wasm_bg.wasm`)), "dist/loro_wasm_bg.wasm");
