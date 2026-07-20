use std::collections::HashMap;
use std::sync::Arc;

use notist_model::{Content, TextRange};

use crate::{BoundArguments, EvalDiagnostic, Evaluation, FunctionSignature, Type, lower_fragment};

/// The input supplied to a content-producing function.
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionInput<'a> {
    /// The function name used at the call site.
    pub name: &'a str,
    /// Arguments after expression evaluation and signature binding.
    pub arguments: BoundArguments,
    /// The complete call range.
    pub range: TextRange,
}

/// Evaluated content returned by a function.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FunctionOutput {
    /// Content produced by the function.
    pub content: Content,
}

impl FunctionOutput {
    /// Creates an output containing only evaluated content.
    pub fn content(content: Content) -> Self {
        Self { content }
    }
}

/// A callable that produces semantic content.
pub trait Function: Send + Sync {
    /// Returns the globally unique function name.
    fn name(&self) -> &str;

    /// Returns the statically checkable function signature.
    fn signature(&self) -> FunctionSignature;

    /// Evaluates bound arguments into content.
    fn call(
        &self,
        context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>>;
}

/// A registry of built-in and plugin-provided functions.
#[derive(Default)]
pub struct FunctionRegistry {
    functions: HashMap<String, Arc<dyn Function>>,
}

impl FunctionRegistry {
    /// Creates an empty function registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a registry containing all core Notist functions.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        crate::builtin::register_builtins(&mut registry)
            .expect("built-in function names must be unique");
        registry
    }

    /// Registers a function after validating its signature, and rejects duplicate names.
    pub fn register(&mut self, function: impl Function + 'static) -> Result<(), RegistryError> {
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
        self.functions.insert(name, Arc::new(function));
        Ok(())
    }

    /// Looks up a function by its qualified name.
    pub fn get(&self, name: &str) -> Option<&dyn Function> {
        self.functions.get(name).map(Arc::as_ref)
    }

    /// Iterates over all registered functions in unspecified order.
    pub fn functions(&self) -> impl Iterator<Item = &dyn Function> {
        self.functions.values().map(Arc::as_ref)
    }
}

/// Validates a function signature against the contracts the registry enforces.
fn validate_signature(signature: &FunctionSignature) -> Option<RegistryErrorReason> {
    // The current `Function` trait can only produce Content, so the declared
    // result type must be Content until value-returning functions exist.
    if signature.result != Type::Content {
        return Some(RegistryErrorReason::InvalidSignature(format!(
            "result type must be Content, found {}",
            signature.result
        )));
    }
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
    if let Some(trailing) = signature.trailing_content {
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
