let activeRuntimeState = null;
export function createMermanRuntimeState(defaultLoader) {
    return {
        defaultLoader,
        wasmModule: null,
        initPromise: null,
        supportedDiagramsCache: null,
        diagramFamilyCapabilitiesCache: null,
        runtimeCatalogCache: null,
        presentationCatalogCache: null,
        supportedThemesCache: null,
    };
}
export function currentMermanRuntimeState(defaultState) {
    return activeRuntimeState ?? defaultState;
}
export function withMermanRuntimeState(state, run) {
    const previous = activeRuntimeState;
    activeRuntimeState = state;
    try {
        return run();
    }
    finally {
        activeRuntimeState = previous;
    }
}
//# sourceMappingURL=runtime-state.js.map