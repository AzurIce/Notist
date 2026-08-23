//! component-host-call: the `plugin-host` world sample.
//!
//! `passthrough` requests `core::text` through `host.call`; the host checks
//! the plugin's capabilities and continues reduction before answering, so a
//! package without the grant fails with a capability diagnostic.

use notist_model::{Node, NodeValue};
use notist_plugin_sdk::{Args, ElementDecl, ElementFn, EvalCtx, Registrar};

pub struct Passthrough;

impl ElementFn for Passthrough {
    fn decl(&self) -> ElementDecl {
        ElementDecl::new("passthrough")
    }

    fn reduce(
        &self,
        ctx: &mut EvalCtx<'_>,
        _args: &Args,
        _body: &[Node],
    ) -> Result<Vec<Node>, String> {
        ctx.call(
            "core::text",
            vec![("text".into(), NodeValue::from("hello from host.call"))],
            Vec::new(),
        )
    }
}

pub struct Plugin;

impl notist_plugin_sdk::Plugin for Plugin {
    fn init(registrar: &mut Registrar) {
        registrar.element(Passthrough);
    }
}

notist_plugin_sdk::export_host_plugin!("component-host-call", Plugin);
