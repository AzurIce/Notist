//! Notist plugin SDK.
//!
//! 插件是 Rust crate，编译为 wasip2 组件。作者实现 [`Plugin`] 与
//! [`ElementFn`]，在 `Plugin::init` 里把元素注册进 [`Registrar`]，再用
//! [`export_plugin!`](crate::export_plugin)（或需要互调时的
//! [`export_host_plugin!`](crate::export_host_plugin)）生成组件导出。
//!
//! 数据表示与宿主共享：作者直接操作 `notist_model::Node` / `NodeValue`，
//! SDK 版本即载荷 ABI 版本。

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

pub use notist_model::{Node, NodeValue};

// ---------------------------------------------------------------------------
// Author-facing declarations (init surface)
// ---------------------------------------------------------------------------

/// Author-facing element declaration. Mirrors the WIT `element-decl` record;
/// the host validates names and signatures after `init` returns.
#[derive(Clone, Debug)]
pub struct ElementDecl {
    /// Element name inside the package namespace (no `{package}::` prefix).
    pub name: String,
    pub version: u32,
    pub block: bool,
    /// Whether a runtime handler exists in the dispatch map. Data-only
    /// declarations (`Registrar::declare`) stay unreduced.
    pub computed: bool,
    pub parameters: Vec<ParamDecl>,
    pub trailing_content: Option<String>,
    pub body_mode: Option<String>,
    pub role: Option<String>,
    pub kind: Option<String>,
}

impl ElementDecl {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            version: 1,
            block: false,
            computed: true,
            parameters: Vec::new(),
            trailing_content: None,
            body_mode: None,
            role: None,
            kind: None,
        }
    }

    pub fn version(mut self, version: u32) -> Self {
        self.version = version;
        self
    }

    pub fn block(mut self, block: bool) -> Self {
        self.block = block;
        self
    }

    /// Marks this declaration as data-only: no runtime handler. Its document
    /// calls stay unreduced and render straight from name + fields.
    pub fn data_only(mut self) -> Self {
        self.computed = false;
        self
    }

    pub fn param(mut self, name: &str, ty: &str) -> Self {
        self.parameters.push(ParamDecl {
            name: name.to_owned(),
            ty: ty.to_owned(),
            default_json: None,
        });
        self
    }

    pub fn param_default(mut self, name: &str, ty: &str, default_json: &str) -> Self {
        self.parameters.push(ParamDecl {
            name: name.to_owned(),
            ty: ty.to_owned(),
            default_json: Some(default_json.to_owned()),
        });
        self
    }

    pub fn trailing_content(mut self, name: &str) -> Self {
        self.trailing_content = Some(name.to_owned());
        self
    }

    pub fn body_mode(mut self, body_mode: &str) -> Self {
        self.body_mode = Some(body_mode.to_owned());
        self
    }

    pub fn role(mut self, role: &str) -> Self {
        self.role = Some(role.to_owned());
        self
    }

    pub fn kind(mut self, kind: &str) -> Self {
        self.kind = Some(kind.to_owned());
        self
    }
}

/// One declared parameter of a plugin element.
#[derive(Clone, Debug)]
pub struct ParamDecl {
    pub name: String,
    /// Notist type syntax: `None`, `Bool`, `Int`, `Float`, `String`,
    /// `Content`, or `T?` for optional types.
    pub ty: String,
    /// JSON encoding of the declared default value.
    pub default_json: Option<String>,
}

/// One named field on a leaf node.
pub type Field = (String, NodeValue);

/// Reduction context passed to [`ElementFn::reduce`].
pub struct EvalCtx<'a> {
    package: &'a str,
    /// The qualified name this dispatch was addressed to
    /// (`{manifest package}::{element}`).
    call_name: String,
    host_call: HostCall<'a>,
}

enum HostCall<'a> {
    Available(&'a dyn Fn(Vec<u8>) -> Result<Vec<u8>, String>),
    Unavailable,
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

    /// Returns the fully qualified `{package}::{name}` element name.
    pub fn qualified_name(&self, name: &str) -> String {
        format!("{}::{}", self.package, name)
    }

    /// Calls another registered semantic function on behalf of this plugin.
    ///
    /// Only available in components built with
    /// [`export_host_plugin!`](crate::export_host_plugin); the host checks
    /// capabilities and continues reduction before answering.
    pub fn call(
        &mut self,
        name: &str,
        arguments: Vec<Field>,
        body: Vec<Node>,
    ) -> Result<Vec<Node>, String> {
        let mut request = Node::call(name, notist_model::TextRange::new(0, 0));
        for (arg_name, value) in arguments {
            request.args.push((arg_name, value));
        }
        for child in body {
            request.children.push(child);
        }
        let bytes = notist_model::wire::encode_forest(std::slice::from_ref(&request))?;
        let response = match &self.host_call {
            HostCall::Available(call) => call(bytes)?,
            HostCall::Unavailable => {
                return Err(
                    "this component was built without `host.call`; export it with \
                     `export_host_plugin!` to combine host functions"
                        .to_owned(),
                );
            }
        };
        notist_model::wire::decode_forest(&response)
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
// Guest runtime shared by both generated worlds.
// ---------------------------------------------------------------------------

/// Guest state built once during `init`.
#[doc(hidden)]
pub struct GuestState {
    pub declarations: Vec<GuestElementDecl>,
    pub dispatch: BTreeMap<String, Arc<dyn ElementFn>>,
}

/// Shape shared by both generated worlds' hoisted `element-decl` types.
#[doc(hidden)]
pub struct GuestElementDecl {
    pub name: String,
    pub version: u32,
    pub block: bool,
    pub computed: bool,
    pub parameters: Vec<(String, String, Option<String>)>,
    pub trailing_content: Option<String>,
    pub body_mode: Option<String>,
    pub role: Option<String>,
    pub kind: Option<String>,
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
    let declarations = registrar
        .elements
        .iter()
        .map(|element| GuestElementDecl {
            name: element.name.clone(),
            version: element.version,
            block: element.block,
            computed: element.computed,
            parameters: element
                .parameters
                .iter()
                .map(|param| {
                    (
                        param.name.clone(),
                        param.ty.clone(),
                        param.default_json.clone(),
                    )
                })
                .collect(),
            trailing_content: element.trailing_content.clone(),
            body_mode: element.body_mode.clone(),
            role: element.role.clone(),
            kind: element.kind.clone(),
        })
        .collect();
    GuestState {
        declarations,
        dispatch: registrar.dispatch,
    }
}

/// Shared `evaluate` dispatcher used by both generated worlds.
///
/// Decodes the dispatched call node, runs the matching [`ElementFn`], and
/// encodes the returned forest back onto the bytes ABI.
#[doc(hidden)]
pub fn evaluate_dispatch(
    state: &GuestState,
    package: &str,
    request: Vec<u8>,
    host_call: Option<&HostCallFn<'_>>,
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
        host_call: match host_call {
            Some(call) => HostCall::Available(call),
            None => HostCall::Unavailable,
        },
    };
    let nodes = element.reduce(&mut ctx, &args, &body)?;
    notist_model::wire::encode_forest(&nodes)
}

type HostCallFn<'a> = dyn Fn(Vec<u8>) -> Result<Vec<u8>, String> + 'a;

/// Creates an empty leaf node addressed to `name`.
pub fn leaf(name: &str, block: bool) -> Node {
    let mut node = Node::call(name, notist_model::TextRange::new(0, 0));
    node.block = block;
    node
}

// ---------------------------------------------------------------------------
// Export macros. The WIT text is embedded inline so plugin authors do not
// need a local copy of the interface file.
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

interface types {
    record param-decl {
        name: string,
        ty: string,
        default-json: option<string>,
    }

    record element-decl {
        name: string,
        version: u32,
        block: bool,
        computed: bool,
        parameters: list<param-decl>,
        trailing-content: option<string>,
        body-mode: option<string>,
        role: option<string>,
        kind: option<string>,
    }
}

world plugin {
    use types.{element-decl};
    export init: func() -> list<element-decl>;
    export evaluate: func(request: list<u8>) -> result<list<u8>, string>;
}
",
            });

            struct NotistGuest;

            impl Guest for NotistGuest {
                fn init() -> Vec<notist::plugin::types::ElementDecl> {
                    let state = $crate::build_guest_state::<PluginImpl>();
                    let declarations = state
                        .declarations
                        .iter()
                        .map(|decl| notist::plugin::types::ElementDecl {
                            name: decl.name.clone(),
                            version: decl.version,
                            block: decl.block,
                            computed: decl.computed,
                            parameters: decl
                                .parameters
                                .iter()
                                .map(
                                    |(name, ty, default_json)| {
                                        notist::plugin::types::ParamDecl {
                                            name: name.clone(),
                                            ty: ty.clone(),
                                            default_json: default_json.clone(),
                                        }
                                    },
                                )
                                .collect(),
                            trailing_content: decl.trailing_content.clone(),
                            body_mode: decl.body_mode.clone(),
                            role: decl.role.clone(),
                            kind: decl.kind.clone(),
                        })
                        .collect();
                    let _ = $crate::GUEST_STATE.set(state);
                    declarations
                }

                fn evaluate(request: Vec<u8>) -> Result<Vec<u8>, String> {
                    let state = $crate::GUEST_STATE.get().ok_or("plugin not initialized")?;
                    $crate::evaluate_dispatch(state, $package, request, None)
                }
            }

            export!(NotistGuest);
        }
    };
}

/// Like [`export_plugin!`], but the component imports `host.call` and
/// [`EvalCtx::call`] can combine host-reduced functions.
#[macro_export]
macro_rules! export_host_plugin {
    ($package:expr, $plugin:ty) => {
        #[allow(non_camel_case_types)]
        type __NotistPlugin = $plugin;

        #[allow(dead_code)]
        mod __notist_plugin_host {
            use super::__NotistPlugin as PluginImpl;

            ::wit_bindgen::generate!({
                world: "plugin-host",
                inline: "
package notist:plugin;

interface types {
    record param-decl {
        name: string,
        ty: string,
        default-json: option<string>,
    }

    record element-decl {
        name: string,
        version: u32,
        block: bool,
        computed: bool,
        parameters: list<param-decl>,
        trailing-content: option<string>,
        body-mode: option<string>,
        role: option<string>,
        kind: option<string>,
    }
}

interface host {
    call: func(request: list<u8>) -> result<list<u8>, string>;
}

world plugin-host {
    use types.{element-decl};
    import host;
    export init: func() -> list<element-decl>;
    export evaluate: func(request: list<u8>) -> result<list<u8>, string>;
}
",
            });

            struct NotistGuest;

            impl Guest for NotistGuest {
                fn init() -> Vec<notist::plugin::types::ElementDecl> {
                    let state = $crate::build_guest_state::<PluginImpl>();
                    let declarations = state
                        .declarations
                        .iter()
                        .map(|decl| notist::plugin::types::ElementDecl {
                            name: decl.name.clone(),
                            version: decl.version,
                            block: decl.block,
                            computed: decl.computed,
                            parameters: decl
                                .parameters
                                .iter()
                                .map(
                                    |(name, ty, default_json)| {
                                        notist::plugin::types::ParamDecl {
                                            name: name.clone(),
                                            ty: ty.clone(),
                                            default_json: default_json.clone(),
                                        }
                                    },
                                )
                                .collect(),
                            trailing_content: decl.trailing_content.clone(),
                            body_mode: decl.body_mode.clone(),
                            role: decl.role.clone(),
                            kind: decl.kind.clone(),
                        })
                        .collect();
                    let _ = $crate::GUEST_STATE.set(state);
                    declarations
                }

                fn evaluate(request: Vec<u8>) -> Result<Vec<u8>, String> {
                    let state = $crate::GUEST_STATE.get().ok_or("plugin not initialized")?;
                    $crate::evaluate_dispatch(
                        state,
                        $package,
                        request,
                        Some(&|bytes: Vec<u8>| notist::plugin::host::call(&bytes)),
                    )
                }
            }

            export!(NotistGuest);
        }
    };
}
