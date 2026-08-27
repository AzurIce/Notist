//! Mermaid semantic component.
//!
//! The computed `mermaid` handler validates its Mermaid source at evaluation
//! time with `mermaid-rs-renderer`'s parser and normalizes the call into the
//! data-only `mermaid::diagram` leaf. Final SVG rendering happens in the
//! browser through the package's Web Component contribution; parse failures
//! surface as host diagnostics on the call site.

use notist_model::{Node, NodeValue};
use notist_plugin_sdk::{Args, ElementDecl, ElementFn, EvalCtx, Registrar};

/// Themes accepted here and understood by the browser-side Mermaid renderer.
const THEMES: [&str; 4] = ["default", "dark", "neutral", "forest"];

pub struct Mermaid;

impl ElementFn for Mermaid {
    fn decl(&self) -> ElementDecl {
        ElementDecl::new("mermaid")
            .block(true)
            .param("source", "String")
            .param_default("theme", "String", "default")
            .trailing_content("body")
            .body_mode("flow")
    }

    fn reduce(
        &self,
        ctx: &mut EvalCtx<'_>,
        args: &Args,
        body: &[Node],
    ) -> Result<Vec<Node>, String> {
        let source = args
            .get_string("source")
            .ok_or_else(|| "mermaid source must be a string".to_owned())?;
        if source.trim().is_empty() {
            return Err("mermaid source must not be empty".to_owned());
        }
        let theme = match args.get_string("theme") {
            None | Some("default") => "default",
            Some(theme) if THEMES.contains(&theme) => theme,
            Some(other) => {
                return Err(format!(
                    "unknown mermaid theme `{other}`; expected one of {}",
                    THEMES.join(", ")
                ));
            }
        };

        mermaid_rs_renderer::parse_mermaid_strict(source)
            .map_err(|error| format!("mermaid source failed to parse: {error}"))?;

        let mut diagram = Node::block_call(ctx.qualified_name("diagram"), ctx.range());
        diagram
            .args
            .push(("source".into(), NodeValue::from(source)));
        diagram
            .args
            .push(("theme".into(), NodeValue::from(theme.to_owned())));
        diagram.children = body.to_vec();
        Ok(vec![diagram])
    }
}

pub struct Plugin;

impl notist_plugin_sdk::Plugin for Plugin {
    fn init(registrar: &mut Registrar) {
        registrar.element(Mermaid);
        registrar.declare(
            ElementDecl::new("diagram")
                .block(true)
                .param("source", "String")
                .param_default("theme", "String", "default")
                .trailing_content("body")
                .body_mode("flow")
                .data_only(),
        );
    }
}

// Default: wasip2 component for the native wasmtime component host.
// `core-abi`: zero-import core wasm module for the wasmtime `Module` host
// and the browser's own WebAssembly engine.
#[cfg(not(feature = "core-abi"))]
notist_plugin_sdk::export_plugin!("mermaid", Plugin);
#[cfg(feature = "core-abi")]
notist_plugin_sdk::export_plugin_core!("mermaid", Plugin);
