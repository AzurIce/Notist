import { SUPPORTED_DIAGRAMS } from "./generated/diagram-catalog.js";
import { RUNTIME_CATALOG_MAX_SAFE_INTEGER } from "./generated/binding-contract.js";
import { SYSTEM_ADAPTER_IDS, WEB_CAPABILITIES, WEB_CAPABILITY_IDS, WEB_BINDING_OPERATION_IDS, WEB_BINDING_OPERATIONS, WEB_OUTPUT_IDS, WEB_OUTPUTS, } from "./generated/capability-surface.js";
export { SUPPORTED_DIAGRAMS };
export { SYSTEM_ADAPTER_IDS, WEB_CAPABILITIES, WEB_CAPABILITY_IDS, WEB_BINDING_OPERATION_IDS, WEB_BINDING_OPERATIONS, WEB_OUTPUT_IDS, WEB_OUTPUTS, };
export const SUPPORTED_THEMES = [
    "default",
    "base",
    "dark",
    "forest",
    "neutral",
    "neo",
    "neo-dark",
    "redux",
    "redux-dark",
    "redux-color",
    "redux-dark-color",
];
export const BUNDLED_THEME_PRESETS = [
    "editor-light",
    "editor-dark",
    "one-dark",
    "gruvbox-light",
    "gruvbox-dark",
    "ayu-light",
    "ayu-dark",
];
export const SUPPORTED_ASCII_DIAGRAMS = [
    "class",
    "er",
    "flowchart",
    "gantt",
    "gitgraph",
    "journey",
    "kanban",
    "mindmap",
    "packet",
    "sequence",
    "state",
    "timeline",
    "treeView",
    "xychart",
    "zenuml",
];
export const BINDING_STATUS_CODE_NAMES = [
    "MERMAN_OK",
    "MERMAN_INVALID_ARGUMENT",
    "MERMAN_UTF8_ERROR",
    "MERMAN_OPTIONS_JSON_ERROR",
    "MERMAN_NO_DIAGRAM",
    "MERMAN_PARSE_ERROR",
    "MERMAN_RENDER_ERROR",
    "MERMAN_UNSUPPORTED_OPERATION",
    "MERMAN_PANIC",
    "MERMAN_INTERNAL_ERROR",
    "MERMAN_RESOURCE_LIMIT_EXCEEDED",
];
export const TEXT_MEASUREMENT_PROVIDER_IDS = [
    "host-callback",
    "vendored",
];
export function isThemeName(theme) {
    return SUPPORTED_THEMES.includes(theme);
}
export function isBundledThemePresetName(preset) {
    return BUNDLED_THEME_PRESETS.includes(preset);
}
export function isDiagramType(diagram) {
    return SUPPORTED_DIAGRAMS.includes(diagram);
}
export function isAsciiDiagramType(diagram) {
    return SUPPORTED_ASCII_DIAGRAMS.includes(diagram);
}
export function isBindingStatusCodeName(codeName) {
    return BINDING_STATUS_CODE_NAMES.includes(codeName);
}
const MAX_SAFE_RESOURCE_COUNT_DECIMAL = String(RUNTIME_CATALOG_MAX_SAFE_INTEGER);
const U64_MAX_DECIMAL = "18446744073709551615";
const CANONICAL_WIDE_UNSIGNED_DECIMAL = /^[1-9]\d*$/;
function isBindingResourceCount(value) {
    if (typeof value === "number") {
        return Number.isSafeInteger(value) && value >= 0;
    }
    if (typeof value !== "string" || !CANONICAL_WIDE_UNSIGNED_DECIMAL.test(value)) {
        return false;
    }
    return compareCanonicalUnsignedDecimals(value, MAX_SAFE_RESOURCE_COUNT_DECIMAL) > 0 &&
        compareCanonicalUnsignedDecimals(value, U64_MAX_DECIMAL) <= 0;
}
function compareCanonicalUnsignedDecimals(left, right) {
    if (left.length !== right.length)
        return left.length < right.length ? -1 : 1;
    if (left === right)
        return 0;
    return left < right ? -1 : 1;
}
export function isBindingErrorPayload(error) {
    if (!error || typeof error !== "object") {
        return false;
    }
    const payload = error;
    const resource = payload.details && typeof payload.details === "object"
        ? payload.details.resource
        : undefined;
    const hasValidDetails = payload.details === undefined ||
        (!!resource &&
            typeof resource === "object" &&
            typeof resource.cause === "string" &&
            typeof resource.limit_id === "string" &&
            typeof resource.phase === "string" &&
            isBindingResourceCount(resource.actual) &&
            isBindingResourceCount(resource.max) &&
            typeof resource.profile === "string");
    return (payload.ok === false &&
        typeof payload.version === "number" &&
        typeof payload.code === "number" &&
        typeof payload.code_name === "string" &&
        typeof payload.kind === "string" &&
        (payload.capability_id === null ||
            typeof payload.capability_id === "string") &&
        hasValidDetails &&
        typeof payload.message === "string");
}
export function normalizeThemeName(theme) {
    return theme && isThemeName(theme) ? theme : "default";
}
export function normalizeBundledThemePresetName(preset) {
    return preset && isBundledThemePresetName(preset) ? preset : null;
}
//# sourceMappingURL=public-catalog.js.map