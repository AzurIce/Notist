//! component-echo: the minimal `plugin` world sample.
//!
//! Two kinds of contributions:
//!
//! - `echo` is a **computed** element: its handler passes the trailing body
//!   through verbatim (productive contract — never returns a node addressed
//!   to itself);
//! - `note` is a **data-only** declaration: no runtime handler, so document
//!   calls stay unreduced and render straight from name + fields.

use notist_model::{Node, NodeValue};
use notist_plugin_sdk::{Args, ElementDecl, ElementFn, EvalCtx, Registrar};

pub struct Echo;

impl ElementFn for Echo {
    fn decl(&self) -> ElementDecl {
        ElementDecl::new("echo")
            .block(true)
            .param_default("message", "String", "hello")
            .trailing_content("body")
            .body_mode("flow")
    }

    fn reduce(
        &self,
        _ctx: &mut EvalCtx<'_>,
        args: &Args,
        body: &[Node],
    ) -> Result<Vec<Node>, String> {
        if !body.is_empty() {
            return Ok(body.to_vec());
        }
        let message = args.get_string("message").unwrap_or("hello");
        let mut leaf = Node::call("core::text", notist_model::TextRange::new(0, 0));
        leaf.args.push(("text".into(), NodeValue::from(message)));
        Ok(vec![leaf])
    }
}

pub struct Plugin;

impl notist_plugin_sdk::Plugin for Plugin {
    fn init(registrar: &mut Registrar) {
        registrar.element(Echo);
        registrar.declare(
            ElementDecl::new("note")
                .block(true)
                .param_default("message", "String", "hello")
                .trailing_content("body")
                .body_mode("flow")
                .data_only(),
        );
    }
}

notist_plugin_sdk::export_plugin!("component-echo", Plugin);
