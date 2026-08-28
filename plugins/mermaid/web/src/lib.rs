//! Browser-side Mermaid renderer for the Notist mermaid plugin.
//!
//! Wraps `mermaid-rs-renderer` (mmdr) as a small wasm module so the plugin's
//! Web Component can render diagrams locally — no CDN, no mermaid.js. Theme
//! names match the vocabulary validated by the semantic component
//! (`default`, `dark`, `neutral`, `forest`), so what passed build-time
//! validation is exactly what renders here.

use wasm_bindgen::prelude::*;

// `default` is the public plugin spelling.  mmdr's modern preset has the
// restrained typography and contrast expected by the HTML target, while the
// classic Mermaid preset is intentionally still available as `base`/`mermaid`.
const DEFAULT_THEME: &str = "modern";

/// Renders Mermaid `source` into an SVG document string.
///
/// `theme` accepts the same names as the plugin's semantic validation;
/// unknown or empty themes fall back to the modern renderer preset. Parse and
/// render failures are returned as JS errors carrying mmdr's message.
#[wasm_bindgen]
pub fn render(source: &str, theme: &str) -> Result<String, JsValue> {
    let requested_theme = theme.trim();
    let renderer_theme =
        if requested_theme.is_empty() || requested_theme.eq_ignore_ascii_case("default") {
            DEFAULT_THEME
        } else {
            requested_theme
        };
    let theme = mermaid_rs_renderer::Theme::from_name(renderer_theme)
        .unwrap_or_else(|| mermaid_rs_renderer::Theme::from_name(DEFAULT_THEME).expect("static"));
    // Compact defaults keep ordinary diagrams readable in a document column;
    // the web component provides horizontal scrolling for genuinely wide ones.
    let options = mermaid_rs_renderer::RenderOptions::modern()
        .with_node_spacing(34.0)
        .with_rank_spacing(42.0);
    let options = mermaid_rs_renderer::RenderOptions { theme, ..options };
    mermaid_rs_renderer::render_with_options(source, options)
        .map_err(|error| JsValue::from_str(&error.to_string()))
}

/// Reports the wrapped mmdr version, for diagnostics and cache busting.
#[wasm_bindgen]
pub fn renderer_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

#[cfg(test)]
mod tests {
    use super::render;

    #[test]
    fn public_default_uses_modern_renderer_theme() {
        let svg = render("flowchart LR\n  A[One] --> B[Two]", "default").expect("diagram renders");
        assert!(svg.contains("#F8FAFC"), "modern node fill missing: {svg}");
        assert!(svg.contains("#0F172A"), "modern text color missing: {svg}");
    }

    #[test]
    fn explicit_classic_alias_remains_available() {
        let svg = render("flowchart LR\n  A --> B", "base").expect("diagram renders");
        assert!(svg.contains("#ECECFF"), "classic node fill missing: {svg}");
    }
}
