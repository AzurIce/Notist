use std::collections::HashMap;
use std::sync::Arc;

use notist_model::{Content, CustomField, Element, ElementValue, TextRange};

use crate::leaf::{CapabilityPolicy, Principal};
use crate::{
    BoundArguments, EvalDiagnostic, Evaluation, FunctionSignature, Type, Value, lower_fragment,
};

/// The input supplied to a native or plugin function.
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionInput<'a> {
    /// The function name used at the call site.
    pub name: &'a str,
    /// Arguments after expression evaluation and signature binding.
    pub arguments: BoundArguments,
    /// The complete call range.
    pub range: TextRange,
}

/// Evaluated value returned by a native, user, or plugin function.
#[derive(Clone, Debug, PartialEq)]
pub enum FunctionOutput {
    /// A final runtime value.
    Value(Value),
    /// A final Content value.
    Content(Content),
    /// A not-yet-reduced sequence of calls.
    Calls(crate::call::CallContent),
    /// A unified-node forest awaiting reduction. This is the canonical
    /// output shape for component plugins speaking the node ABI.
    Nodes(Vec<notist_model::Node>),
}

impl FunctionOutput {
    /// Creates a Content-valued output.
    pub fn content(content: Content) -> Self {
        Self::Content(content)
    }

    pub fn value(value: Value) -> Self {
        Self::Value(value)
    }

    /// Creates a call-content output that the host should reduce.
    pub fn calls(calls: crate::call::CallContent) -> Self {
        Self::Calls(calls)
    }
}

/// The package that owns a registered function.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum FunctionOwner {
    /// The language-owned host/core package.
    Host,
    /// A plugin package.
    Plugin(String),
}

/// A declarative element constructor backed by an [`ElementSchema`].
///
/// The registry validates and binds arguments before this function runs, so
/// the implementation only has to project bound arguments onto
/// [`Element::Custom`]. This is the eval contribution used by pure schema
/// plugins (and eventually by the core package) when no executable code is
/// required.
pub struct ElementFunction {
    element_name: String,
    block: bool,
    signature: FunctionSignature,
    owner: FunctionOwner,
}

impl ElementFunction {
    /// Creates a declarative element constructor.
    pub fn new(
        element_name: impl Into<String>,
        signature: FunctionSignature,
        block: bool,
        owner: FunctionOwner,
    ) -> Self {
        Self {
            element_name: element_name.into(),
            block,
            signature,
            owner,
        }
    }
}

impl Function for ElementFunction {
    fn name(&self) -> &str {
        &self.element_name
    }

    fn signature(&self) -> FunctionSignature {
        self.signature.clone()
    }

    fn owner(&self) -> FunctionOwner {
        self.owner.clone()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let trailing = self.signature.trailing_content.as_deref();
        let mut fields = Vec::new();
        for parameter in &self.signature.parameters {
            if Some(parameter.name.as_str()) == trailing {
                continue;
            }
            let Some(value) = input.arguments.get(&parameter.name) else {
                continue;
            };
            fields.push(CustomField {
                name: parameter.name.clone(),
                value: value_to_element_value(value),
            });
        }
        let body = match trailing {
            Some(name) => input.arguments.take_content(name),
            None => Content::new(),
        };
        Ok(FunctionOutput::content(Content::single(
            Element::Custom {
                name: self.element_name.clone(),
                body,
                block: self.block,
                fields,
            },
            input.range,
        )))
    }
}

/// Converts an evaluated runtime value into the serializable element field
/// value domain. Functions cannot cross the element boundary and degrade to
/// [`ElementValue::None`]; schemas should reject such parameters up front.
fn value_to_element_value(value: &Value) -> ElementValue {
    match value {
        Value::None => ElementValue::None,
        Value::Bool(value) => ElementValue::Bool(*value),
        Value::Int(value) => ElementValue::Int(*value),
        Value::Float(value) => ElementValue::Float(*value),
        Value::String(value) => ElementValue::String(value.clone()),
        Value::Content(content) => ElementValue::Content(content.clone()),
        Value::Function(_) => ElementValue::None,
    }
}

/// A native or plugin callable that consumes and produces runtime values.
pub trait Function: Send + Sync {
    /// Returns the globally unique function name.
    fn name(&self) -> &str;

    /// Returns the statically checkable function signature.
    fn signature(&self) -> FunctionSignature;

    /// Evaluates bound arguments into a runtime value.
    fn call(
        &self,
        context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>>;

    /// Returns the package that owns this function.
    ///
    /// Native core functions return [`FunctionOwner::Host`]; Wasm plugin
    /// functions override this with their package id.
    fn owner(&self) -> FunctionOwner {
        FunctionOwner::Host
    }
}

/// A registry of built-in and plugin-provided functions.
#[derive(Default)]
pub struct FunctionRegistry {
    functions: HashMap<String, Arc<dyn Function>>,
    aliases: HashMap<String, String>,
    capability_policy: CapabilityPolicy,
}

impl Clone for FunctionRegistry {
    fn clone(&self) -> Self {
        Self {
            functions: self.functions.clone(),
            aliases: self.aliases.clone(),
            capability_policy: self.capability_policy.clone(),
        }
    }
}

impl std::fmt::Debug for FunctionRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FunctionRegistry")
            .field("functions", &self.functions.keys().collect::<Vec<_>>())
            .field("aliases", &self.aliases.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl FunctionRegistry {
    /// Creates an empty function registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a registry containing all core Notist functions.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        crate::core::register_builtins(&mut registry)
            .expect("built-in function names must be unique");
        registry
    }

    /// Registers a function after validating its signature, and rejects duplicate names.
    pub fn register(&mut self, function: impl Function + 'static) -> Result<(), RegistryError> {
        self.register_arc(Arc::new(function))
    }

    /// Registers an already-allocated function object.
    pub fn register_arc(&mut self, function: Arc<dyn Function>) -> Result<(), RegistryError> {
        let name = function.name().to_owned();
        if self.functions.contains_key(&name) {
            return Err(RegistryError {
                name,
                reason: RegistryErrorReason::Duplicate,
            });
        }
        let signature = function.signature();
        if let Some(reason) = validate_signature(&signature) {
            return Err(RegistryError { name, reason });
        }
        self.functions.insert(name, function);
        Ok(())
    }

    /// Registers an alias that resolves to an already-registered function.
    ///
    /// Aliases give `core::*` qualified names to prelude constructors without
    /// duplicating function objects.
    pub fn register_alias(
        &mut self,
        alias: impl Into<String>,
        target: impl Into<String>,
    ) -> Result<(), RegistryError> {
        let alias = alias.into();
        let target = target.into();
        if self.functions.contains_key(&alias) || self.aliases.contains_key(&alias) {
            return Err(RegistryError {
                name: alias,
                reason: RegistryErrorReason::Duplicate,
            });
        }
        if !self.functions.contains_key(&target) {
            return Err(RegistryError {
                name: alias,
                reason: RegistryErrorReason::InvalidSignature(format!(
                    "alias target `{target}` is not registered"
                )),
            });
        }
        self.aliases.insert(alias, target);
        Ok(())
    }

    /// Removes a function and any alias with the given name.
    pub fn unregister(&mut self, name: &str) {
        self.functions.remove(name);
        self.aliases.remove(name);
    }

    /// Looks up a function by name or alias.
    pub fn get(&self, name: &str) -> Option<&dyn Function> {
        let mut current = name;
        for _ in 0..16 {
            if let Some(function) = self.functions.get(current) {
                return Some(function.as_ref());
            }
            current = self.aliases.get(current)?;
        }
        None
    }

    /// Iterates over all registered functions in unspecified order.
    pub fn functions(&self) -> impl Iterator<Item = &dyn Function> {
        self.functions.values().map(Arc::as_ref)
    }

    /// Grants `caller` permission to dispatch `callee`.
    ///
    /// Grants are stored on the registry so both legacy reduction and the
    /// Stream + Leaf reduction engine see the same effective plugin policy.
    pub fn allow(&mut self, caller: Principal, callee: impl Into<String>) {
        self.capability_policy = self.capability_policy.clone().allow(caller, callee);
    }

    /// Returns the current capability policy.
    pub fn policy(&self) -> CapabilityPolicy {
        self.capability_policy.clone()
    }
}

/// Validates a function signature against the contracts the registry enforces.
fn validate_signature(signature: &FunctionSignature) -> Option<RegistryErrorReason> {
    for parameter in &signature.parameters {
        if let Some(default) = &parameter.default
            && !parameter.ty.accepts(&default.ty())
        {
            return Some(RegistryErrorReason::InvalidSignature(format!(
                "default value for parameter `{}` is {}, expected {}",
                parameter.name,
                default.ty(),
                parameter.ty
            )));
        }
    }
    if let Some(trailing) = signature.trailing_content.as_deref() {
        let content_parameter = signature
            .parameters
            .iter()
            .find(|parameter| parameter.name == trailing && parameter.ty == Type::Content);
        if content_parameter.is_none() {
            return Some(RegistryErrorReason::InvalidSignature(format!(
                "trailing Content parameter `{trailing}` is not declared as a Content parameter"
            )));
        }
    }
    None
}

/// An error returned when a function cannot be registered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryError {
    /// The name of the function that failed to register.
    pub name: String,
    /// Why the registration failed.
    pub reason: RegistryErrorReason,
}

/// The reason a function registration was rejected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistryErrorReason {
    /// A function with the same name is already registered.
    Duplicate,
    /// The declared signature violates a registry-enforced contract.
    InvalidSignature(String),
}

/// Services available while a function is being evaluated.
pub struct FunctionContext<'a> {
    pub(crate) registry: &'a FunctionRegistry,
    pub(crate) depth: usize,
}

impl FunctionContext<'_> {
    /// Explicitly parses and lowers a source fragment as nested Notist content.
    pub fn evaluate(&self, source: &str, range: TextRange) -> Evaluation {
        const MAX_DEPTH: usize = 64;
        if self.depth >= MAX_DEPTH {
            return Evaluation {
                diagnostics: vec![EvalDiagnostic {
                    message: "function evaluation exceeded the recursion limit".into(),
                    range,
                }],
                ..Evaluation::default()
            };
        }

        lower_fragment(source, range.start, self.registry, self.depth + 1)
    }
}
