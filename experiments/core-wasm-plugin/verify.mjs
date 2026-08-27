// Cross-runtime conformance check for a freestanding notist plugin module.
//
// Loads the same zero-import core wasm module the native host loads, runs
// `notist_init` + `notist_evaluate` through raw linear memory, and compares
// the response byte-for-byte against fixtures captured from the Rust host.
//
// Usage: node verify.mjs <plugin.wasm> <fixtures-dir>
//   <fixtures-dir>/request.bin   forest bytes the host would send
//   <fixtures-dir>/response.bin  bytes the native host got back

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Worker, isMainThread, parentPort, workerData } from "node:worker_threads";

const args = isMainThread ? process.argv.slice(2) : workerData.args;
const [wasmPath, fixturesDir] = args;
if (!wasmPath || !fixturesDir) {
  console.error("usage: node verify.mjs <plugin.wasm> <fixtures-dir>");
  process.exit(2);
}

function instantiate() {
  const bytes = readFileSync(wasmPath);
  // Zero imports: the freestanding module needs nothing from the embedder.
  const module = new WebAssembly.Module(bytes);
  const instance = new WebAssembly.Instance(module, {});
  return instance.exports;
}

function verify() {
  const exports = instantiate();
  const memory = exports.memory;

  const readHandle = (handle) => {
    const packed = BigInt.asUintN(64, BigInt(handle));
    const ptr = Number(packed >> 32n);
    const lenRaw = Number(packed & 0xffff_ffffn);
    const is_error = (lenRaw & 0x8000_0000) !== 0;
    const len = lenRaw & 0x7fff_ffff;
    const bytes = new Uint8Array(memory.buffer, ptr, len).slice();
    exports.notist_free(ptr, len);
    if (is_error) {
      throw new Error(`plugin returned error: ${new TextDecoder().decode(bytes)}`);
    }
    return bytes;
  };

  const hex = (bytes) => [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");

  // init must reproduce the declarations the Rust guest state encodes.
  const declarations = readHandle(exports.notist_init());
  const expectedDeclarations = readFileSync(join(fixturesDir, "declarations.bin"));
  if (hex(declarations) !== hex(expectedDeclarations)) {
    console.error(`DECLARATIONS MISMATCH\n  expected: ${hex(expectedDeclarations)}\n  actual:   ${hex(declarations)}`);
    process.exit(1);
  }
  console.log(`init: ${declarations.length} bytes of declarations match`);

  // abi revision
  console.log(`abi: ${exports.notist_abi()}`);

  // evaluate the exact request the native host sent, compare byte-for-byte.
  const request = readFileSync(join(fixturesDir, "request.bin"));
  const expected = readFileSync(join(fixturesDir, "response.bin"));
  const reqPtr = exports.notist_alloc(request.length);
  new Uint8Array(memory.buffer, reqPtr, request.length).set(request);
  const response = readHandle(exports.notist_evaluate(reqPtr, request.length));

  if (hex(response) !== hex(expected)) {
    console.error(`MISMATCH\n  expected: ${hex(expected)}\n  actual:   ${hex(response)}`);
    process.exit(1);
  }
  console.log(`evaluate: ${response.length} bytes match the native host response`);
  console.log("PASS");
}

if (isMainThread) {
  // Run once on the main thread and once in a Worker (mirrors the browser
  // deployment where plugin instantiation lives in a worker).
  verify();
  new Worker(new URL(import.meta.url), {
    workerData: { args },
  })
    .on("message", (message) => {
      console.log(`worker: ${message}`);
      console.log("PASS (worker)");
    })
    .on("error", (error) => {
      console.error(error);
      process.exit(1);
    });
} else {
  verify();
  parentPort.postMessage("ok");
}
