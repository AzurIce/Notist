//! Browser-side Mermaid renderer for the Notist mermaid plugin.
//!
//! Wraps `mermaid-rs-renderer` (mmdr) as a small wasm module so the plugin's
//! Web Component can render diagrams locally — no CDN, no mermaid.js. Theme
//! names match the vocabulary validated by the semantic component
//! (`default`, `dark`, `neutral`, `forest`), so what passed build-time
//! validation is exactly what renders here.

use wasm_bindgen::prelude::*;

const DEFAULT_THEME: &str = "default";

/// Renders Mermaid `source` into an SVG document string.
///
/// `theme` accepts the same names as the plugin's semantic validation;
/// unknown or empty themes fall back to the default preset. Parse and render
/// failures are returned as JS errors carrying mmdr's message.
#[wasm_bindgen]
pub fn render(source: &str, theme: &str) -> Result<String, JsValue> {
    let theme = mermaid_rs_renderer::Theme::from_name(theme)
        .unwrap_or_else(|| mermaid_rs_renderer::Theme::from_name(DEFAULT_THEME).expect("static"));
    let options = mermaid_rs_renderer::RenderOptions {
        theme,
        ..mermaid_rs_renderer::RenderOptions::default()
    };
    mermaid_rs_renderer::render_with_options(source, options)
        .map_err(|error| JsValue::from_str(&error.to_string()))
}

/// Reports the wrapped mmdr version, for diagnostics and cache busting.
#[wasm_bindgen]
pub fn renderer_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}
