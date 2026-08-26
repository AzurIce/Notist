import { createMermanRuntimeState, withMermanRuntimeState, } from "./runtime-state.js";
/// Reject server runtimes at the public browser-package boundary.
///
/// A main-window package may use `window` and `document`; an editor package may instead run in a
/// real browser Worker. Node and SSR runtimes match neither shape. This check intentionally runs
/// before a caller-supplied loader so a custom WASM source cannot turn a browser package into an
/// undocumented server transport.
export function assertBrowserRuntime() {
    const processLike = globalThis.process;
    const isNodeRuntime = processLike?.release?.name === "node" &&
        typeof processLike.versions?.node === "string";
    const isDenoRuntime = "Deno" in globalThis;
    const isBunRuntime = "Bun" in globalThis;
    if (isNodeRuntime || isDenoRuntime || isBunRuntime) {
        throw new Error("Merman browser packages require a browser main-thread or Web Worker realm. Use a native or Node transport for SSR and server runtimes.");
    }
    const isBrowserWindow = typeof window !== "undefined" && typeof document !== "undefined";
    const workerGlobalScope = globalThis.WorkerGlobalScope;
    const isBrowserWorker = typeof workerGlobalScope === "function" && globalThis instanceof workerGlobalScope;
    if (!isBrowserWindow && !isBrowserWorker) {
        throw new Error("Merman browser packages require a browser main-thread or Web Worker realm. Use a native or Node transport for SSR and Node.js.");
    }
}
function withPackageDefaultWasmSource(init, wasm) {
    if (typeof init === "function") {
        return { loader: init, wasm };
    }
    if (init?.loader && init.wasm === undefined) {
        return { loader: init.loader, wasm };
    }
    return init;
}
export function bindSurfaceRuntime(surfaceLoader, implementation, packageDefaultWasmSource) {
    const sharedLoader = surfaceLoader;
    const state = createMermanRuntimeState(sharedLoader);
    const runtime = {};
    for (const [name, binding] of Object.entries(implementation)) {
        runtime[name] =
            name === "initMerman" && packageDefaultWasmSource !== undefined
                ? (init) => withMermanRuntimeState(state, () => binding(withPackageDefaultWasmSource(init, packageDefaultWasmSource)))
                : (...args) => withMermanRuntimeState(state, () => binding(...args));
    }
    return runtime;
}
//# sourceMappingURL=surface-runtime.js.map