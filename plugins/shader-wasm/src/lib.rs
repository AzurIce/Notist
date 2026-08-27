//! Shader semantic component.
//!
//! The computed `shader` handler normalizes its call into the data-only
//! `shader::canvas` leaf rendered by the package's HTML contribution.

use notist_model::{Node, NodeValue};
use notist_plugin_sdk::{Args, ElementDecl, ElementFn, EvalCtx, Registrar};

pub struct Shader;

impl ElementFn for Shader {
    fn decl(&self) -> ElementDecl {
        ElementDecl::new("shader")
            .block(true)
            .param("source", "String")
            .param_default("width", "Int", 800_i64)
            .param_default("height", "Int", 600_i64)
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
            .ok_or_else(|| "shader source must be a string".to_owned())?;
        let width = args.get_int("width").unwrap_or(800);
        let height = args.get_int("height").unwrap_or(600);

        let mut canvas = Node::block_call(ctx.qualified_name("canvas"), ctx.range());
        canvas.args.push(("source".into(), NodeValue::from(source)));
        canvas.args.push(("width".into(), NodeValue::Int(width)));
        canvas.args.push(("height".into(), NodeValue::Int(height)));
        canvas.children = body.to_vec();
        Ok(vec![canvas])
    }
}

pub struct Plugin;

impl notist_plugin_sdk::Plugin for Plugin {
    fn init(registrar: &mut Registrar) {
        registrar.element(Shader);
        registrar.declare(
            ElementDecl::new("canvas")
                .block(true)
                .param("source", "String")
                .param_default("width", "Int", 800_i64)
                .param_default("height", "Int", 600_i64)
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
notist_plugin_sdk::export_plugin!("shader", Plugin);
#[cfg(feature = "core-abi")]
notist_plugin_sdk::export_plugin_core!("shader", Plugin);
