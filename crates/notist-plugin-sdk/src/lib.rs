//! Notist plugin SDK.
//!
//! 插件是 Rust crate，编译为零导入 core wasm module。作者实现 [`Plugin`] 与
//! [`ElementFn`]，在 `Plugin::init` 里把元素注册进 [`Registrar`]，再用
//! [`export_plugin!`](crate::export_plugin) 生成原始内存 ABI 导出。
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
// Freestanding core-module ABI.
//
// The component world above rides on the canonical ABI; the exports below
// speak raw linear memory instead so the same plugin logic can be compiled
// as a zero-import core wasm module and instantiated by any embedder —
// wasmi natively, or the browser's own WebAssembly engine.
//
// Convention (little-endian, all pointers into guest linear memory):
// - `notist_alloc(len: u32) -> u32`  guest allocates `len` writable bytes;
// - `notist_free(ptr: u32, len: u32)` releases a buffer handed to the host
//   or allocated for it; `len` is the length the host observed;
// - `notist_init() -> i64` and `notist_evaluate(ptr: u32, len: u32) -> i64`
//   pack a result as `(ptr: u32) << 32 | (len: u32)`. A set sign bit on the
//   length half marks an error message string instead of a payload.
// Guest buffers are always `shrink_to_fit`-ed before being handed out, so
// `len == capacity` and `notist_free(ptr, len)` reclaims exactly what was
// allocated.
// ---------------------------------------------------------------------------

#[doc(hidden)]
pub mod core_abi {
    /// Packs a guest buffer handle; `len` carries the error sign bit.
    pub fn pack(ptr: u32, len: u32) -> i64 {
        (((ptr as u64) << 32) | (len as u64 & 0xffff_ffff)) as i64
    }

    /// Hands ownership of a result buffer to the host.
    pub fn pack_ok(mut bytes: Vec<u8>) -> i64 {
        bytes.shrink_to_fit();
        let handle = pack(bytes.as_ptr() as u32, bytes.len() as u32);
        std::mem::forget(bytes);
        handle
    }

    /// Hands ownership of an error message string to the host. The flag is
    /// the top bit of the length half, so the payload stays at most 2 GiB.
    pub fn pack_error(message: &str) -> i64 {
        pack_ok(message.as_bytes().to_vec()) | (1_i64 << 31)
    }

    /// Reclaims a buffer handed to the host. `len` must be the length the
    /// host observed; buffers were shrunk before handoff, so capacity == len.
    ///
    /// # Safety
    /// `ptr` must have been produced by this module's allocator and not
    /// freed before.
    pub unsafe fn reclaim(ptr: u32, len: u32) {
        if len == 0 {
            return;
        }
        // SAFETY: see the function contract; capacity == len by convention.
        drop(unsafe { Vec::<u8>::from_raw_parts(ptr as *mut u8, 0, len as usize) });
    }
}

/// Shared freestanding `notist_init` implementation.
#[doc(hidden)]
pub fn core_init<P: Plugin>() -> i64 {
    let state = build_guest_state::<P>();
    match wire::encode_declarations(&state.declarations) {
        Ok(bytes) => {
            let _ = GUEST_STATE.set(state);
            core_abi::pack_ok(bytes)
        }
        Err(message) => core_abi::pack_error(&format!("plugin init failed: {message}")),
    }
}

/// Shared freestanding `notist_evaluate` implementation. Takes ownership of
/// the host-written request buffer (`ptr`, `len`) and frees it before
/// returning.
///
/// # Safety
/// `ptr` must have been returned by `notist_alloc` with `len` bytes still
/// valid, and must not be freed by the host.
pub unsafe fn core_evaluate(package: &str, ptr: u32, len: u32) -> i64 {
    // SAFETY: the caller guarantees `ptr` is a live `notist_alloc` buffer of
    // `len` bytes that the host has not freed.
    let request = unsafe { Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize) };
    match GUEST_STATE.get() {
        None => core_abi::pack_error("plugin not initialized"),
        Some(state) => match evaluate_dispatch(state, package, request) {
            Ok(bytes) => core_abi::pack_ok(bytes),
            Err(message) => core_abi::pack_error(&message),
        },
    }
}

// ---------------------------------------------------------------------------
// Export macro. The WIT text is embedded inline so plugin authors do not need
// a local copy of the interface file.
// ---------------------------------------------------------------------------

/// Generates freestanding core-wasm exports for a [`Plugin`] implementation.
///
/// This is the only plugin export form: the crate compiles to a plain
/// zero-import core wasm module (e.g. `--target wasm32-unknown-unknown`) with
/// no WASI and no component layer, so the same `.wasm` artifact instantiates
/// under wasmi natively and under the browser's own WebAssembly engine. See
/// the module docs for the raw memory ABI.
#[macro_export]
macro_rules! export_plugin {
    ($package:expr, $plugin:ty) => {
        #[allow(non_camel_case_types)]
        type __NotistPluginCore = $plugin;

        /// ABI revision of the freestanding core-module exports.
        #[unsafe(no_mangle)]
        pub extern "C" fn notist_abi() -> i32 {
            1
        }

        /// Allocates `len` writable bytes for the host to fill.
        #[unsafe(no_mangle)]
        pub extern "C" fn notist_alloc(len: u32) -> u32 {
            let mut buffer = Vec::<u8>::with_capacity(len as usize);
            let ptr = buffer.as_mut_ptr() as u32;
            std::mem::forget(buffer);
            ptr
        }

        /// Releases a buffer handed to the host (`notist_init`,
        /// `notist_evaluate`) or allocated for it (`notist_alloc`).
        #[unsafe(no_mangle)]
        pub extern "C" fn notist_free(ptr: u32, len: u32) {
            // SAFETY: buffers are only handed out by the paths above and are
            // freed exactly once by the host.
            unsafe { $crate::core_abi::reclaim(ptr, len) }
        }

        /// Runs the plugin's `init`; returns the encoded declarations.
        #[unsafe(no_mangle)]
        pub extern "C" fn notist_init() -> i64 {
            $crate::core_init::<__NotistPluginCore>()
        }

        /// Evaluates one dispatched call from the host-written request
        /// buffer; takes ownership of the buffer and returns the response.
        #[unsafe(no_mangle)]
        pub extern "C" fn notist_evaluate(ptr: u32, len: u32) -> i64 {
            // SAFETY: the host allocated `ptr` via `notist_alloc` and has not
            // freed it.
            unsafe { $crate::core_evaluate($package, ptr, len) }
        }
    };
}
