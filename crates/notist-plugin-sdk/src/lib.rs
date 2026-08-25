//! Notist plugin SDK.
//!
//! 插件是 Rust crate，编译为 wasip2 组件。作者实现 [`Plugin`] 与
//! [`ElementFn`]，在 `Plugin::init` 里把元素注册进 [`Registrar`]，再用
//! [`export_plugin!`](crate::export_plugin) 生成组件导出。
//!
//! 数据表示与宿主共享：作者直接操作 `notist_model::Node` / `NodeValue`，
//! 插件返回的 Node forest 由宿主统一继续规约。

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

pub use notist_model::{
    Node, NodeValue, PluginElementDecl as ElementDecl, PluginParamDecl as ParamDecl, wire,
};

/// One named field on a leaf node.
pub type Field = (String, NodeValue);

/// Reduction context passed to [`ElementFn::reduce`].
pub struct EvalCtx<'a> {
    package: &'a str,
    /// The qualified name this dispatch was addressed to
    /// (`{manifest package}::{element}`).
    call_name: String,
    /// Source range of the dispatched root call.
    range: notist_model::TextRange,
}

impl<'a> EvalCtx<'a> {
    /// The manifest package id; use it to qualify returned leaf names.
    pub fn package(&self) -> &'a str {
        self.package
    }

    /// The fully qualified `{package}::{name}` name of the dispatched call.
    pub fn call_name(&self) -> &str {
        &self.call_name
    }

    /// Source range of the dispatched root call.
    pub fn range(&self) -> notist_model::TextRange {
        self.range
    }

    /// Returns the fully qualified `{package}::{name}` element name.
    pub fn qualified_name(&self, name: &str) -> String {
        format!("{}::{}", self.package, name)
    }
}

/// One semantic element contributed by a plugin.
///
/// Implementations must be pure and deterministic: the same inputs must
/// always reduce to the same output stream.
pub trait ElementFn: Send + Sync + 'static {
    /// The declaration registered during [`Plugin::init`].
    fn decl(&self) -> ElementDecl;

    /// Reduces one dispatched call into a node stream.
    fn reduce(
        &self,
        ctx: &mut EvalCtx<'_>,
        args: &Args,
        body: &[Node],
    ) -> Result<Vec<Node>, String>;
}

/// Plugin entry point. `init` runs exactly once at load time, inside the same
/// fuel budget as `evaluate`.
pub trait Plugin {
    fn init(registrar: &mut Registrar);
}

/// Collects the elements a plugin contributes.
#[derive(Default)]
pub struct Registrar {
    elements: Vec<ElementDecl>,
    dispatch: BTreeMap<String, Arc<dyn ElementFn>>,
}

impl Registrar {
    /// Declares a data-only element: no runtime handler. Its document calls
    /// stay unreduced and render straight from name + fields.
    pub fn declare(&mut self, decl: ElementDecl) {
        if !decl.name.is_empty()
            && !decl.name.contains("::")
            && !self
                .elements
                .iter()
                .any(|existing| existing.name == decl.name)
        {
            let mut decl = decl;
            decl.computed = false;
            self.elements.push(decl);
        } else {
            panic!(
                "element `{}` is empty, qualified, or already registered",
                decl.name
            );
        }
    }

    /// Registers one element implementation. Duplicate names are rejected at
    /// registration time so failures surface in the author's test run rather
    /// than as host load diagnostics.
    pub fn element<E: ElementFn>(&mut self, element: E) {
        let decl = element.decl();
        if !decl.name.is_empty()
            && !decl.name.contains("::")
            && self
                .dispatch
                .insert(decl.name.clone(), Arc::new(element))
                .is_none()
        {
            self.elements.push(decl);
        } else {
            panic!(
                "element `{}` is empty, qualified, or already registered",
                decl.name
            );
        }
    }
}

/// Bound arguments of one dispatched call.
#[derive(Default)]
pub struct Args {
    values: BTreeMap<String, NodeValue>,
}

impl Args {
    pub fn get(&self, name: &str) -> Option<&NodeValue> {
        self.values.get(name)
    }

    pub fn get_string(&self, name: &str) -> Option<&str> {
        match self.values.get(name) {
            Some(NodeValue::String(value)) => Some(value.as_str()),
            _ => None,
        }
    }

    pub fn get_int(&self, name: &str) -> Option<i64> {
        match self.values.get(name) {
            Some(NodeValue::Int(value)) => Some(*value),
            _ => None,
        }
    }

    pub fn get_bool(&self, name: &str) -> Option<bool> {
        match self.values.get(name) {
            Some(NodeValue::Bool(value)) => Some(*value),
            _ => None,
        }
    }

    fn from_pairs(pairs: &[(String, NodeValue)]) -> Self {
        Self {
            values: pairs.iter().cloned().collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// Guest runtime shared by the generated component world.
// ---------------------------------------------------------------------------

/// Guest state built once during `init`.
#[doc(hidden)]
pub struct GuestState {
    pub declarations: Vec<ElementDecl>,
    pub dispatch: BTreeMap<String, Arc<dyn ElementFn>>,
}

/// Shared guest state cell; written once by `init`, read by every
/// `evaluate` dispatch.
#[doc(hidden)]
pub static GUEST_STATE: OnceLock<GuestState> = OnceLock::new();

/// Builds guest state from the author's `Plugin::init` implementation.
#[doc(hidden)]
pub fn build_guest_state<P: Plugin>() -> GuestState {
    let mut registrar = Registrar::default();
    P::init(&mut registrar);
    GuestState {
        declarations: registrar.elements,
        dispatch: registrar.dispatch,
    }
}

/// Shared `evaluate` dispatcher used by the generated component world.
///
/// Decodes the dispatched call node, runs the matching [`ElementFn`], and
/// encodes the returned forest back onto the bytes ABI.
#[doc(hidden)]
pub fn evaluate_dispatch(
    state: &GuestState,
    package: &str,
    request: Vec<u8>,
) -> Result<Vec<u8>, String> {
    let forest = notist_model::wire::decode_forest(&request)?;
    let root = forest
        .first()
        .ok_or_else(|| "dispatch carried no call".to_owned())?;
    let local_name = root
        .name
        .rsplit("::")
        .next()
        .unwrap_or(&root.name)
        .to_owned();
    let element = state
        .dispatch
        .get(&local_name)
        .ok_or_else(|| format!("unknown element `{}`", root.name))?;
    let args = Args::from_pairs(&root.args);
    let body = root.children.clone();
    let mut ctx = EvalCtx {
        package,
        call_name: root.name.clone(),
        range: root.range,
    };
    let nodes = element.reduce(&mut ctx, &args, &body)?;
    notist_model::wire::encode_forest(&nodes)
}

/// Creates an empty leaf node addressed to `name`.
pub fn leaf(name: &str, block: bool) -> Node {
    let mut node = Node::call(name, notist_model::TextRange::new(0, 0));
    node.block = block;
    node
}

// ---------------------------------------------------------------------------
// Export macro. The WIT text is embedded inline so plugin authors do not need
// a local copy of the interface file.
// ---------------------------------------------------------------------------

/// Generates the wasip2 component exports for a [`Plugin`] implementation.
///
/// `$package` must equal the manifest `package` field so returned leaf names
/// qualify correctly. Requires `wit-bindgen` as a dependency of the crate.
#[macro_export]
macro_rules! export_plugin {
    ($package:expr, $plugin:ty) => {
        #[allow(non_camel_case_types)]
        type __NotistPlugin = $plugin;

        #[allow(dead_code)]
        mod __notist_plugin {
            use super::__NotistPlugin as PluginImpl;

            ::wit_bindgen::generate!({
                world: "plugin",
                inline: "
package notist:plugin;

world plugin {
    export init: func() -> result<list<u8>, string>;
    export evaluate: func(request: list<u8>) -> result<list<u8>, string>;
}
",
            });

            struct NotistGuest;

            impl Guest for NotistGuest {
                fn init() -> Result<Vec<u8>, String> {
                    let state = $crate::build_guest_state::<PluginImpl>();
                    let declarations = $crate::wire::encode_declarations(&state.declarations)?;
                    $crate::GUEST_STATE
                        .set(state)
                        .map_err(|_| "plugin already initialized".to_owned())?;
                    Ok(declarations)
                }

                fn evaluate(request: Vec<u8>) -> Result<Vec<u8>, String> {
                    let state = $crate::GUEST_STATE.get().ok_or("plugin not initialized")?;
                    $crate::evaluate_dispatch(state, $package, request)
                }
            }

            export!(NotistGuest);
        }
    };
}
