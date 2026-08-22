//! Notist plugin SDK.
//!
//! 插件是 Rust crate，编译为 wasip2 组件。作者实现 [`Plugin`] 与
//! [`ElementFn`]，在 `Plugin::init` 里把元素注册进 [`Registrar`]，再用
//! [`export_plugin!`](crate::export_plugin)（或需要互调时的
//! [`export_host_plugin!`](crate::export_host_plugin)）生成组件导出。
//!
//! ```ignore
//! use notist_plugin_sdk::{ElementDecl, ElementFn, EvalCtx, Args, Node, Plugin, Registrar};
//!
//! struct Echo;
//!
//! impl ElementFn for Echo {
//!     fn decl(&self) -> ElementDecl {
//!         ElementDecl::new("echo").block(true).param("message", "String")
//!     }
//!
//!     fn reduce(&self, ctx: &mut EvalCtx, args: &Args, _body: &[Node]) -> Result<Vec<Node>, String> {
//!         let message = args.get_string("message").unwrap_or("hello");
//!         Ok(vec![Node::leaf(&ctx.qualified_name("echo"), true)
//!             .field("message", Value::from(message))])
//!     }
//! }
//!
//! struct Plugin;
//!
//! impl notist_plugin_sdk::Plugin for Plugin {
//!     fn init(reg: &mut Registrar) {
//!         reg.element(Echo);
//!     }
//! }
//!
//! notist_plugin_sdk::export_plugin!("component-echo", Plugin);
//! ```

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

/// Author-facing element declaration. Mirrors the WIT `element-decl` record
/// and the host manifest shape; the host validates names and signatures after
/// `init` returns.
#[derive(Clone, Debug)]
pub struct ElementDecl {
    /// Element name inside the package namespace (no `{package}::` prefix).
    pub name: String,
    pub version: u32,
    pub block: bool,
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

/// One value crossing the plugin ABI. Functions never cross the boundary.
#[derive(Clone, Debug)]
pub enum Value {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Stream(Vec<Node>),
    Array(Vec<Value>),
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Int(value)
    }
}

impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Value::Int(i64::from(value))
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Float(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::String(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::String(value)
    }
}

/// One named field on a leaf node.
pub type Field = (String, Value);

/// One terminal leaf node a plugin may return from `reduce`.
///
/// `name` must be the fully qualified `{package}::{element}` spelling; use
/// [`EvalCtx::qualified_name`] to build it.
#[derive(Clone, Debug)]
pub struct Node {
    pub name: String,
    pub fields: Vec<Field>,
    pub body: Vec<Node>,
    pub block: bool,
}

impl Node {
    pub fn leaf(name: &str, block: bool) -> Self {
        Self {
            name: name.to_owned(),
            fields: Vec::new(),
            body: Vec::new(),
            block,
        }
    }

    pub fn field(mut self, name: &str, value: impl Into<Value>) -> Self {
        self.fields.push((name.to_owned(), value.into()));
        self
    }

    pub fn child(mut self, node: Node) -> Self {
        self.body.push(node);
        self
    }
}

/// Bound arguments of one dispatched call.
#[derive(Default)]
pub struct Args {
    values: BTreeMap<String, Value>,
}

impl Args {
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.values.get(name)
    }

    pub fn get_string(&self, name: &str) -> Option<&str> {
        match self.values.get(name) {
            Some(Value::String(value)) => Some(value.as_str()),
            _ => None,
        }
    }

    pub fn get_int(&self, name: &str) -> Option<i64> {
        match self.values.get(name) {
            Some(Value::Int(value)) => Some(*value),
            _ => None,
        }
    }

    pub fn get_bool(&self, name: &str) -> Option<bool> {
        match self.values.get(name) {
            Some(Value::Bool(value)) => Some(*value),
            _ => None,
        }
    }
}

/// Reduction context passed to [`ElementFn::reduce`].
pub struct EvalCtx<'a> {
    package: &'a str,
    /// The qualified name this dispatch was addressed to
    /// (`{manifest package}::{element}`). Leaves returned under exactly this
    /// name are guaranteed to match the host-side registration namespace.
    call_name: String,
    host_call: HostCall<'a>,
}

type HostCallFn<'a> = dyn Fn(Vec<u8>) -> Result<Vec<u8>, String> + 'a;

enum HostCall<'a> {
    Available(&'a HostCallFn<'a>),
    Unavailable,
}

impl<'a> EvalCtx<'a> {
    /// The manifest package id; use it to qualify returned leaf names.
    pub fn package(&self) -> &'a str {
        self.package
    }

    /// The fully qualified `{package}::{name}` name of the dispatched call.
    ///
    /// Returning leaves named after this value keeps them aligned with the
    /// namespace the host registered, even when the same component binary is
    /// repackaged under a different package id.
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
        let request = WireCall {
            name: name.to_owned(),
            arguments: arguments
                .into_iter()
                .map(|(name, value)| WireArgument {
                    name,
                    value: value_to_wire(&value),
                })
                .collect(),
            body: (!body.is_empty()).then(|| body.iter().map(node_to_wire).collect()),
        };
        let bytes = serde_json::to_vec(&request)
            .map_err(|error| format!("cannot encode host call: {error}"))?;
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
        let nodes: Vec<WireNode> = serde_json::from_slice(&response)
            .map_err(|error| format!("invalid host.call response: {error}"))?;
        nodes
            .iter()
            .map(wire_to_node)
            .collect::<Result<Vec<_>, String>>()
    }
}

/// One semantic element contributed by a plugin.
///
/// Implementations must be pure and deterministic: the same inputs must
/// always reduce to the same output stream.
pub trait ElementFn: Send + Sync + 'static {
    /// The declaration registered during [`Plugin::init`].
    fn decl(&self) -> ElementDecl;

    /// Reduces one dispatched call into a `Call | Leaf` node stream.
    ///
    /// The host continues reduction over any returned `call` nodes under the
    /// same depth / fuel / capability budget as this dispatch.
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

// ---------------------------------------------------------------------------
// Wire schema. Mirrors `notist-plugin-host/src/wire.rs`; keep both sides in
// sync when the payload schema changes.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
enum WireValue {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Stream(Vec<WireNode>),
    Array(Vec<WireValue>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WireArgument {
    name: String,
    value: WireValue,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WireField {
    name: String,
    value: WireValue,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WireCall {
    name: String,
    #[serde(default)]
    arguments: Vec<WireArgument>,
    #[serde(default)]
    body: Option<Vec<WireNode>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WireLeaf {
    name: String,
    #[serde(default)]
    fields: Vec<WireField>,
    #[serde(default)]
    body: Vec<WireNode>,
    block: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireNode {
    Call(WireCall),
    Leaf(WireLeaf),
}

fn value_to_wire(value: &Value) -> WireValue {
    match value {
        Value::None => WireValue::None,
        Value::Bool(value) => WireValue::Bool(*value),
        Value::Int(value) => WireValue::Int(*value),
        Value::Float(value) => WireValue::Float(*value),
        Value::String(value) => WireValue::String(value.clone()),
        Value::Stream(nodes) => WireValue::Stream(nodes.iter().map(node_to_wire).collect()),
        Value::Array(values) => WireValue::Array(values.iter().map(value_to_wire).collect()),
    }
}

fn wire_to_value(value: &WireValue) -> Result<Value, String> {
    Ok(match value {
        WireValue::None => Value::None,
        WireValue::Bool(value) => Value::Bool(*value),
        WireValue::Int(value) => Value::Int(*value),
        WireValue::Float(value) => Value::Float(*value),
        WireValue::String(value) => Value::String(value.clone()),
        WireValue::Stream(nodes) => {
            let mut converted = Vec::with_capacity(nodes.len());
            for node in nodes {
                if let WireNode::Leaf(_) = node {
                    converted.push(wire_to_node(node)?);
                } else {
                    return Err("unreduced call inside an argument stream".to_owned());
                }
            }
            Value::Stream(converted)
        }
        WireValue::Array(values) => {
            let mut converted = Vec::with_capacity(values.len());
            for value in values {
                converted.push(wire_to_value(value)?);
            }
            Value::Array(converted)
        }
    })
}

fn node_to_wire(node: &Node) -> WireNode {
    WireNode::Leaf(WireLeaf {
        name: node.name.clone(),
        fields: node
            .fields
            .iter()
            .map(|(name, value)| WireField {
                name: name.clone(),
                value: value_to_wire(value),
            })
            .collect(),
        body: node.body.iter().map(node_to_wire).collect(),
        block: node.block,
    })
}

fn wire_to_node(node: &WireNode) -> Result<Node, String> {
    match node {
        WireNode::Leaf(leaf) => Ok(Node {
            name: leaf.name.clone(),
            fields: leaf
                .fields
                .iter()
                .map(|field| Ok((field.name.clone(), wire_to_value(&field.value)?)))
                .collect::<Result<Vec<_>, String>>()?,
            body: leaf
                .body
                .iter()
                .map(|node| match node {
                    WireNode::Leaf(_) => wire_to_node(node),
                    WireNode::Call(_) => Err("unreduced call inside a leaf body".to_owned()),
                })
                .collect::<Result<Vec<_>, String>>()?,
            block: leaf.block,
        }),
        WireNode::Call(_) => Err("unreduced call in a reduced response".to_owned()),
    }
}

fn args_from_wire(arguments: Vec<WireArgument>) -> Result<Args, String> {
    let mut values = BTreeMap::new();
    for argument in arguments {
        values.insert(argument.name.clone(), wire_to_value(&argument.value)?);
    }
    Ok(Args { values })
}

fn body_from_wire(body: Option<Vec<WireNode>>) -> Result<Vec<Node>, String> {
    let mut nodes = Vec::new();
    for node in body.unwrap_or_default() {
        match node {
            WireNode::Leaf(_) => nodes.push(wire_to_node(&node)?),
            WireNode::Call(_) => return Err("unreduced call in the trailing body".to_owned()),
        }
    }
    Ok(nodes)
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
/// Decodes the encoded `Call` request, dispatches by element name, and
/// encodes the returned node stream back onto the bytes ABI.
#[doc(hidden)]
pub fn evaluate_dispatch(
    state: &GuestState,
    package: &str,
    request: Vec<u8>,
    host_call: Option<&HostCallFn<'_>>,
) -> Result<Vec<u8>, String> {
    let call: WireCall =
        serde_json::from_slice(&request).map_err(|error| format!("invalid request: {error}"))?;
    let local_name = call
        .name
        .rsplit("::")
        .next()
        .unwrap_or(&call.name)
        .to_owned();
    let element = state
        .dispatch
        .get(&local_name)
        .ok_or_else(|| format!("unknown element `{}`", call.name))?;
    let args = args_from_wire(call.arguments)?;
    let body = body_from_wire(call.body)?;
    let mut ctx = EvalCtx {
        package,
        call_name: call.name.clone(),
        host_call: match host_call {
            Some(call) => HostCall::Available(call),
            None => HostCall::Unavailable,
        },
    };
    let nodes = element.reduce(&mut ctx, &args, &body)?;
    let wire_nodes: Vec<WireNode> = nodes.iter().map(node_to_wire).collect();
    serde_json::to_vec(&wire_nodes).map_err(|error| format!("cannot encode response: {error}"))
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
