//! Rust bindings for data-only Notist plugins.
//!
//! ```
//! use notist_plugin_sdk::{func, init_plugin};
//!
//! init_plugin!(echo, double);
//!
//! #[func]
//! pub fn echo(source: Option<String>) -> Option<String> { source }
//!
//! #[func(defaults(value = 2))]
//! pub fn double(value: i64) -> i64 { value * 2 }
//!
//! # fn main() {
//! assert_eq!(echo(None), None);
//! assert_eq!(double(3), 6);
//! # }
//! ```
//! `#[func]` preserves an ordinary Rust function and generates its typed wrapper.
//! `init_plugin!(...)` lists the function paths included in the plugin's registry.
//! `Option<T>` maps to `T?`; omitted arguments become `none` in the Notist host.
//! Explicit defaults are registration data, evaluated without call arguments.
//! Rust calls still supply every parameter. Raw ABI calls supply already-bound arguments.
//!
//! Build a release cdylib for `wasm32-unknown-unknown`. The host creates a fresh
//! instance for registration and each call; imports and persistent state are unsupported.
extern crate self as notist_plugin_sdk;

pub use abi::{Content, Item, Value};
pub use notist_model::{ContentMode, ElementModel, Target, Type, abi};
/// Annotate a synchronous, non-generic Rust function with a Notist export.
/// `#[func(defaults(parameter = expression))]` supplies explicit registration defaults.
///
/// ```compile_fail
/// #[notist_plugin_sdk::func]
/// fn generic<T>(value: T) -> T { value }
/// ```
/// ```compile_fail
/// #[notist_plugin_sdk::func(defaults(missing = 1))]
/// fn wrong_default(value: i64) -> i64 { value }
/// ```
/// ```compile_fail
/// #[notist_plugin_sdk::func(defaults(value = "wrong type"))]
/// fn wrong_type(value: i64) -> i64 { value }
/// ```
pub use notist_plugin_macros::func;
/// Initialize a plugin once, listing its annotated functions (including module paths).
/// Also generates native `notist_registration` and `notist_dispatch` test entry points.
/// `init_plugin!(elements = models; foo, bar)` includes the element definitions returned
/// by `models() -> BTreeMap<String, ElementModel>` in the same registration payload.
pub use notist_plugin_macros::init_plugin;
use std::collections::BTreeMap;

/// Rust representations of values accepted by a Notist function.
pub trait PluginValue: Sized {
    fn ty() -> Type;
    fn into_value(self) -> Value;
    fn from_value(value: Value) -> Result<Self, String>;
}

macro_rules! scalar {
    ($rust:ty, $variant:ident) => {
        impl PluginValue for $rust {
            fn ty() -> Type {
                Type::$variant
            }
            fn into_value(self) -> Value {
                Value::$variant(self)
            }
            fn from_value(value: Value) -> Result<Self, String> {
                match value {
                    Value::$variant(v) => Ok(v),
                    _ => Err(concat!("expected ", stringify!($variant)).into()),
                }
            }
        }
    };
}
scalar!(String, String);
scalar!(i64, Int);
scalar!(bool, Bool);
scalar!(Target, Target);

impl PluginValue for Content {
    fn ty() -> Type {
        Type::Content
    }
    fn into_value(self) -> Value {
        Value::Content(self)
    }
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Content(v) => Ok(v),
            _ => Err("expected Content".into()),
        }
    }
}

impl PluginValue for () {
    fn ty() -> Type {
        Type::None
    }
    fn into_value(self) -> Value {
        Value::None
    }
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::None => Ok(()),
            _ => Err("expected None".into()),
        }
    }
}

impl PluginValue for Value {
    fn ty() -> Type {
        Type::Any
    }
    fn into_value(self) -> Value {
        self
    }
    fn from_value(value: Value) -> Result<Self, String> {
        Ok(value)
    }
}

impl<T: PluginValue> PluginValue for Option<T> {
    fn ty() -> Type {
        Type::Optional(Box::new(T::ty()))
    }
    fn into_value(self) -> Value {
        self.map_or(Value::None, T::into_value)
    }
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::None => Ok(None),
            value => T::from_value(value).map(Some),
        }
    }
}

impl<T: PluginValue> PluginValue for Vec<T> {
    fn ty() -> Type {
        Type::List
    }
    fn into_value(self) -> Value {
        Value::List(self.into_iter().map(T::into_value).collect())
    }
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::List(v) => v.into_iter().map(T::from_value).collect(),
            _ => Err("expected List".into()),
        }
    }
}

impl<T: PluginValue> PluginValue for BTreeMap<String, T> {
    fn ty() -> Type {
        Type::Dict
    }
    fn into_value(self) -> Value {
        Value::Dict(self.into_iter().map(|(k, v)| (k, v.into_value())).collect())
    }
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Dict(v) => v
                .into_iter()
                .map(|(k, v)| Ok((k, T::from_value(v)?)))
                .collect(),
            _ => Err("expected Dict".into()),
        }
    }
}

/// A fallible plugin returns a Notist error at the host's call site.
pub trait PluginResult {
    fn ty() -> Type;
    fn into_result(self) -> Result<Value, String>;
}

impl<T: PluginValue> PluginResult for T {
    fn ty() -> Type {
        T::ty()
    }
    fn into_result(self) -> Result<Value, String> {
        Ok(self.into_value())
    }
}

impl<T: PluginValue> PluginResult for Result<T, String> {
    fn ty() -> Type {
        T::ty()
    }
    fn into_result(self) -> Result<Value, String> {
        self.map(T::into_value)
    }
}

#[doc(hidden)]
pub mod __private {
    use super::*;

    pub struct Export {
        pub name: &'static str,
        pub descriptor: fn() -> abi::Function,
        pub invoke: fn(&[u8]) -> Vec<u8>,
    }

    pub fn registration(exports: &[Export]) -> abi::Registration {
        let mut functions = BTreeMap::new();
        for export in exports {
            assert!(
                functions
                    .insert(export.name.into(), (export.descriptor)())
                    .is_none(),
                "duplicate plugin export: {}",
                export.name
            );
        }
        abi::Registration {
            functions,
            elements: BTreeMap::new(),
        }
    }

    pub fn invoke_export(exports: &[Export], name: &str, input: &[u8]) -> Vec<u8> {
        match exports.iter().find(|export| export.name == name) {
            Some(export) => (export.invoke)(input),
            None => dispatch(input, |_| Err(format!("unknown export `{name}`"))),
        }
    }

    pub fn registration_bytes(registration: &abi::Registration) -> Vec<u8> {
        serde_json::to_vec(registration).expect("registration contains only serializable data")
    }

    pub fn dispatch(
        input: &[u8],
        call: impl FnOnce(Vec<Value>) -> Result<Value, String>,
    ) -> Vec<u8> {
        let result = serde_json::from_slice(input)
            .map_err(|e| format!("invalid arguments: {e}"))
            .and_then(call)
            .unwrap_or_else(|message| Value::Content(Content::error(message)));
        serde_json::to_vec(&result).expect("result contains only serializable data")
    }

    #[cfg(target_arch = "wasm32")]
    pub fn alloc(length: u32) -> u32 {
        Box::into_raw(vec![0u8; length as usize].into_boxed_slice()) as *mut u8 as u32
    }

    /// # Safety
    /// `pointer..pointer+length` must be a live, initialized allocation in this instance.
    #[cfg(target_arch = "wasm32")]
    pub unsafe fn input<'a>(pointer: u32, length: u32) -> &'a [u8] {
        unsafe { std::slice::from_raw_parts(pointer as *const u8, length as usize) }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn output(bytes: Vec<u8>) -> u64 {
        let length = bytes.len() as u32;
        let pointer = Box::into_raw(bytes.into_boxed_slice()) as *mut u8 as u32;
        // Allocations are reclaimed when the host drops the instance after this call.
        ((pointer as u64) << 32) | length as u64
    }
}
