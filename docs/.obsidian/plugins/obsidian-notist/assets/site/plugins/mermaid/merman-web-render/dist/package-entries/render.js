import { assertBrowserRuntime, bindSurfaceRuntime } from "../surface-runtime.js";
import { initMerman as runtime_initMerman, getMerman as runtime_getMerman, isMermanInitialized as runtime_isMermanInitialized, runtimeCatalog as runtime_runtimeCatalog, presentationCatalog as runtime_presentationCatalog, supportedDiagrams as runtime_supportedDiagrams, diagramFamilyCapabilities as runtime_diagramFamilyCapabilities, supportedThemes as runtime_supportedThemes, transportApiVersion as runtime_transportApiVersion, packageVersion as runtime_packageVersion, } from "../runtime-core.js";
import { renderSvg as runtime_renderSvg, svgPlanJson as runtime_svgPlanJson, renderSvgWithTextMeasurer as runtime_renderSvgWithTextMeasurer, layoutJsonWithTextMeasurer as runtime_layoutJsonWithTextMeasurer, renderSvgElement as runtime_renderSvgElement, renderSvgToElement as runtime_renderSvgToElement, parseJson as runtime_parseJson, parseObject as runtime_parseObject, layoutJson as runtime_layoutJson, layoutObject as runtime_layoutObject, } from "../runtime-render.js";
export { MERMAN_TEXT_MEASUREMENT_PROTOCOL_VERSION, } from "../generated/text-measurement-abi.js";
export { UNAVAILABLE_DIAGRAM_DETECTION, encodeOptions, } from "../runtime-core.js";
export { BUNDLED_THEME_PRESETS, SUPPORTED_THEMES, SUPPORTED_DIAGRAMS, SUPPORTED_ASCII_DIAGRAMS, BINDING_STATUS_CODE_NAMES, isThemeName, isDiagramType, isAsciiDiagramType, isBindingStatusCodeName, isBundledThemePresetName, isBindingErrorPayload, normalizeThemeName, normalizeBundledThemePresetName, } from "../public-catalog.js";
export { BINDING_OPTIONS_SCHEMA_VERSION, RESOURCE_LIMIT_IDS, RESOURCE_LIMIT_METADATA, RESOURCE_OVERRIDE_IDS, RESOURCE_PROFILES, isKnownResourceLimitId, rawResourceOptionsJson, resourceLimitMetadata, resourceOptions, resourceOptionsJson, } from "../generated/resource-contract.js";
export { assertNavigableSvgForDom, assertSelfContainedSvgForDom, prepareNavigableSvgForDomMount, prepareSelfContainedSvgForDomMount, } from "../svg-safety.js";
export { createBrowserTextMeasurementSession, } from "../runtime-render.js";
export const MERMAN_WASM_URL = new URL("../../artifacts/wasm/merman_wasm_bg.wasm", import.meta.url).href;
let wasmModulePromise;
export function loadMermanWasmModule() {
    assertBrowserRuntime();
    wasmModulePromise ?? (wasmModulePromise = loadPackageWasmModule());
    return wasmModulePromise;
}
async function loadPackageWasmModule() {
    // @ts-ignore -- wasm-bindgen output is assembled after TypeScript compilation.
    const module = await import("../../artifacts/wasm/merman_wasm.js");
    return {
        ...module,
        default(input) {
            return module.default(input ?? { module_or_path: MERMAN_WASM_URL });
        },
    };
}
const implementation = {
    initMerman: runtime_initMerman,
    getMerman: runtime_getMerman,
    isMermanInitialized: runtime_isMermanInitialized,
    runtimeCatalog: runtime_runtimeCatalog,
    presentationCatalog: runtime_presentationCatalog,
    supportedDiagrams: runtime_supportedDiagrams,
    diagramFamilyCapabilities: runtime_diagramFamilyCapabilities,
    supportedThemes: runtime_supportedThemes,
    transportApiVersion: runtime_transportApiVersion,
    packageVersion: runtime_packageVersion,
    renderSvg: runtime_renderSvg,
    svgPlanJson: runtime_svgPlanJson,
    renderSvgWithTextMeasurer: runtime_renderSvgWithTextMeasurer,
    layoutJsonWithTextMeasurer: runtime_layoutJsonWithTextMeasurer,
    renderSvgElement: runtime_renderSvgElement,
    renderSvgToElement: runtime_renderSvgToElement,
    parseJson: runtime_parseJson,
    parseObject: runtime_parseObject,
    layoutJson: runtime_layoutJson,
    layoutObject: runtime_layoutObject,
};
const runtime = bindSurfaceRuntime(loadMermanWasmModule, implementation, MERMAN_WASM_URL);
export function initMerman(init) {
    assertBrowserRuntime();
    return runtime.initMerman(init);
}
export const getMerman = runtime.getMerman;
export const isMermanInitialized = runtime.isMermanInitialized;
export const runtimeCatalog = runtime.runtimeCatalog;
export const presentationCatalog = runtime.presentationCatalog;
export const supportedDiagrams = runtime.supportedDiagrams;
export const diagramFamilyCapabilities = runtime.diagramFamilyCapabilities;
export const supportedThemes = runtime.supportedThemes;
export const transportApiVersion = runtime.transportApiVersion;
export const packageVersion = runtime.packageVersion;
export const renderSvg = runtime.renderSvg;
export const svgPlanJson = runtime.svgPlanJson;
export const renderSvgWithTextMeasurer = runtime.renderSvgWithTextMeasurer;
export const layoutJsonWithTextMeasurer = runtime.layoutJsonWithTextMeasurer;
export const renderSvgElement = runtime.renderSvgElement;
export const renderSvgToElement = runtime.renderSvgToElement;
export const parseJson = runtime.parseJson;
export const parseObject = runtime.parseObject;
export const layoutJson = runtime.layoutJson;
export const layoutObject = runtime.layoutObject;
//# sourceMappingURL=render.js.map