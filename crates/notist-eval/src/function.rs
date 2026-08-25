use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use notist_model::{ElementSchema, Node, NodeValue, TextRange};

use crate::{
    BoundArguments, EvalDiagnostic, Evaluation, FunctionSignature, ShapingRegistry, Type, Value,
    evaluate_fragment,
};

/// A semantic package contribution shared by native and Wasm packages.
///
/// A contribution is installed as one transaction: all functions, aliases,
/// signatures, and shaping schemas are validated before any registry is
/// changed. HTML/rendering data intentionally lives outside this eval type.
#[derive(Clone)]
pub struct PluginContribution {
    /// Stable package identity used by functions and element namespaces.
    pub package: String,
    /// Executable functions contributed by the package.
    pub functions: Vec<Arc<dyn Function>>,
    /// Statically visible signatures, including data-only elements.
    pub signatures: Vec<(String, FunctionSignature)>,
    /// Shaping metadata for the package's elements.
    pub elements: Vec<ElementSchema>,
    /// Compatibility aliases from a public name to a registered function.
    pub aliases: Vec<(String, String)>,
}

impl std::fmt::Debug for PluginContribution {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginContribution")
            .field("package", &self.package)
            .field(
                "functions",
                &self
                    .functions
                    .iter()
                    .map(|function| function.name())
                    .collect::<Vec<_>>(),
            )
            .field("signatures", &self.signatures)
            .field("elements", &self.elements)
            .field("aliases", &self.aliases)
            .finish()
    }
}

impl PluginContribution {
    /// Creates an empty package contribution.
    pub fn new(package: impl Into<String>) -> Self {
        Self {
            package: package.into(),
            functions: Vec::new(),
            signatures: Vec::new(),
            elements: Vec::new(),
            aliases: Vec::new(),
        }
    }

    /// Adds an executable native or plugin function.
    pub fn function(mut self, function: impl Function + 'static) -> Self {
        self.functions.push(Arc::new(function));
        self
    }

    /// Adds an already allocated executable function.
    pub fn function_arc(mut self, function: Arc<dyn Function>) -> Self {
        self.functions.push(function);
        self
    }

    /// Adds a statically visible signature.
    pub fn signature(mut self, name: impl Into<String>, signature: FunctionSignature) -> Self {
        self.signatures.push((name.into(), signature));
        self
    }

    /// Adds shaping metadata for one element.
    pub fn element(mut self, schema: ElementSchema) -> Self {
        self.elements.push(schema);
        self
    }

    /// Adds a compatibility alias to a function name.
    pub fn alias(mut self, alias: impl Into<String>, target: impl Into<String>) -> Self {
        self.aliases.push((alias.into(), target.into()));
        self
    }
}

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

/// The package identity that owns a registered function.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum FunctionOwner {
    /// A semantic package, such as `core` or a Wasm package id.
    Package(String),
}

impl FunctionOwner {
    /// Creates an owner from a semantic package identity.
    pub fn package(package: impl Into<String>) -> Self {
        Self::Package(package.into())
    }
}

/// A declarative element constructor backed by an [`ElementSchema`].
///
/// The registry validates and binds arguments before this function runs, so
/// the implementation only has to project bound arguments onto a [`Node`]
/// addressed to the element itself. This is the eval contribution used by
/// pure schema plugins (and eventually by the core package) when no
/// executable code is required.
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
    ) -> Result<Value, Vec<EvalDiagnostic>> {
        let trailing = self.signature.trailing_content.as_deref();
        let mut node = Node {
            name: self.element_name.clone(),
            args: Vec::new(),
            children: Vec::new(),
            block: self.block,
            range: input.range,
        };
        for parameter in &self.signature.parameters {
            if Some(parameter.name.as_str()) == trailing {
                continue;
            }
            let Some(value) = input.arguments.get(&parameter.name) else {
                continue;
            };
            node.args
                .push((parameter.name.clone(), value_to_node_value(value)));
        }
        if let Some(name) = trailing {
            node.children = input.arguments.take_content(name);
        }
        Ok(Value::Content(vec![node]))
    }
}

/// Converts an evaluated runtime value into the node value domain. Functions
/// cannot cross the element boundary and degrade to [`NodeValue::None`];
/// schemas should reject such parameters up front.
fn value_to_node_value(value: &Value) -> NodeValue {
    match value {
        Value::None => NodeValue::None,
        Value::Bool(value) => NodeValue::Bool(*value),
        Value::Int(value) => NodeValue::Int(*value),
        Value::Float(value) => NodeValue::Float(*value),
        Value::String(value) => NodeValue::String(value.clone()),
        Value::Content(forest) => NodeValue::Stream(forest.clone()),
        Value::Function(_) => NodeValue::None,
    }
}

/// A native or plugin callable that consumes and produces runtime values.
pub trait Function: Send + Sync {
    /// Returns the globally unique function name.
    fn name(&self) -> &str;

    /// Returns the statically checkable function signature.
    fn signature(&self) -> FunctionSignature;

    /// Evaluates bound arguments into a runtime value.
    ///
    /// Functions always return a [`Value`]; content results are
    /// [`Value::Content`] forests that re-enter the reduction fixpoint.
    fn call(
        &self,
        context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<Value, Vec<EvalDiagnostic>>;

    /// Returns the package identity that owns this function.
    fn owner(&self) -> FunctionOwner {
        FunctionOwner::Package("core".into())
    }
}

/// A registry of built-in and plugin-provided functions.
#[derive(Default)]
pub struct FunctionRegistry {
    functions: HashMap<String, Arc<dyn Function>>,
    aliases: HashMap<String, String>,
    signatures: HashMap<String, FunctionSignature>,
}

impl Clone for FunctionRegistry {
    fn clone(&self) -> Self {
        Self {
            functions: self.functions.clone(),
            aliases: self.aliases.clone(),
            signatures: self.signatures.clone(),
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

    /// Creates a core registry for eval's own unit tests.
    #[cfg(test)]
    pub fn with_builtins() -> Self {
        crate::test_core::registry().0
    }

    /// Registers a function after validating its signature, and rejects duplicate names.
    pub fn register(&mut self, function: impl Function + 'static) -> Result<(), RegistryError> {
        self.register_arc(Arc::new(function))
    }

    /// Registers an already-allocated function object.
    pub fn register_arc(&mut self, function: Arc<dyn Function>) -> Result<(), RegistryError> {
        let name = function.name().to_owned();
        if self.functions.contains_key(&name) || self.aliases.contains_key(&name) {
            return Err(RegistryError {
                name,
                reason: RegistryErrorReason::Duplicate,
            });
        }
        let signature = function.signature();
        if let Some(reason) = validate_signature(&signature) {
            return Err(RegistryError { name, reason });
        }
        self.signatures.insert(name.clone(), signature);
        self.functions.insert(name, function);
        Ok(())
    }

    /// Atomically installs one package contribution into function and shaping registries.
    pub fn register_contribution(
        &mut self,
        shaping: &mut ShapingRegistry,
        contribution: &PluginContribution,
    ) -> Result<(), RegistryError> {
        let mut candidate = self.clone();
        let mut candidate_shaping = shaping.clone();
        candidate.install_contribution(&mut candidate_shaping, contribution)?;
        *self = candidate;
        *shaping = candidate_shaping;
        Ok(())
    }

    fn install_contribution(
        &mut self,
        shaping: &mut ShapingRegistry,
        contribution: &PluginContribution,
    ) -> Result<(), RegistryError> {
        let mut names = self.functions.keys().cloned().collect::<HashSet<_>>();
        names.extend(self.aliases.keys().cloned());
        let mut contribution_names = HashSet::new();
        for function in &contribution.functions {
            let name = function.name().to_owned();
            if !contribution_names.insert(name.clone()) || names.contains(&name) {
                return Err(RegistryError {
                    name,
                    reason: RegistryErrorReason::Duplicate,
                });
            }
            if let Some(reason) = validate_signature(&function.signature()) {
                return Err(RegistryError { name, reason });
            }
        }
        let mut signature_names = HashSet::new();
        for (name, signature) in &contribution.signatures {
            if !signature_names.insert(name.clone()) {
                return Err(RegistryError {
                    name: name.clone(),
                    reason: RegistryErrorReason::Duplicate,
                });
            }
            if let Some(reason) = validate_signature(signature) {
                return Err(RegistryError {
                    name: name.clone(),
                    reason,
                });
            }
            let has_function = contribution
                .functions
                .iter()
                .any(|function| function.name() == name);
            if !has_function && (self.signatures.contains_key(name) || names.contains(name)) {
                return Err(RegistryError {
                    name: name.clone(),
                    reason: RegistryErrorReason::Duplicate,
                });
            }
            if let Some(function) = contribution
                .functions
                .iter()
                .find(|function| function.name() == name)
                && function.signature() != *signature
            {
                return Err(RegistryError {
                    name: name.clone(),
                    reason: RegistryErrorReason::InvalidSignature(
                        "declared signature does not match function signature".into(),
                    ),
                });
            }
        }
        for (alias, target) in &contribution.aliases {
            if contribution
                .signatures
                .iter()
                .any(|(name, _)| name == alias)
                || (!contribution_names.contains(alias) && names.contains(alias))
            {
                return Err(RegistryError {
                    name: alias.clone(),
                    reason: RegistryErrorReason::Duplicate,
                });
            }
            if !contribution_names.contains(target) && self.functions.get(target).is_none() {
                return Err(RegistryError {
                    name: alias.clone(),
                    reason: RegistryErrorReason::InvalidSignature(format!(
                        "alias target `{target}` is not registered"
                    )),
                });
            }
            if !contribution_names.insert(alias.clone()) {
                return Err(RegistryError {
                    name: alias.clone(),
                    reason: RegistryErrorReason::Duplicate,
                });
            }
        }
        let mut schema_names = HashSet::new();
        for schema in &contribution.elements {
            if !schema_names.insert(schema.name.clone()) || shaping.get(&schema.name).is_some() {
                return Err(RegistryError {
                    name: schema.name.to_string(),
                    reason: RegistryErrorReason::DuplicateSchema,
                });
            }
        }
        for function in &contribution.functions {
            let name = function.name().to_owned();
            self.signatures.insert(name.clone(), function.signature());
            self.functions.insert(name, Arc::clone(function));
        }
        for (name, signature) in &contribution.signatures {
            self.signatures.insert(name.clone(), signature.clone());
        }
        for (alias, target) in &contribution.aliases {
            self.aliases.insert(alias.clone(), target.clone());
        }
        for schema in &contribution.elements {
            shaping.insert(schema.clone());
        }
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
        self.signatures.remove(name);
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
    /// A function or alias with the same name is already registered.
    Duplicate,
    /// A shaping schema with the same element name is already registered.
    DuplicateSchema,
    /// The declared signature violates a registry-enforced contract.
    InvalidSignature(String),
}

/// Services available while a function is being evaluated.
pub struct FunctionContext<'a> {
    pub(crate) registry: &'a FunctionRegistry,
    pub(crate) depth: usize,
}

impl FunctionContext<'_> {
    /// Explicitly parses and evaluates a source fragment as nested Notist
    /// content, reducing it on the node engine.
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

        evaluate_fragment(source, range.start, self.registry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notist_model::{BodyMode, ElementName, ShapingKind, ShapingRole};

    struct TestFunction {
        name: &'static str,
        owner: FunctionOwner,
    }

    impl Function for TestFunction {
        fn name(&self) -> &str {
            self.name
        }

        fn signature(&self) -> FunctionSignature {
            notist_model::empty_content_signature()
        }

        fn owner(&self) -> FunctionOwner {
            self.owner.clone()
        }

        fn call(
            &self,
            _context: &FunctionContext<'_>,
            _input: FunctionInput<'_>,
        ) -> Result<Value, Vec<EvalDiagnostic>> {
            Ok(Value::None)
        }
    }

    fn schema(package: &str, local: &str) -> ElementSchema {
        ElementSchema::new(
            ElementName::plugin(package, local),
            ShapingKind::Block,
            BodyMode::Flow,
            ShapingRole::None,
        )
    }

    #[test]
    fn contribution_install_is_atomic_on_duplicate_function() {
        let mut registry = FunctionRegistry::new();
        let mut shaping = ShapingRegistry::new();
        let valid = PluginContribution::new("demo")
            .function(TestFunction {
                name: "demo::ok",
                owner: FunctionOwner::Package("demo".into()),
            })
            .element(schema("demo", "ok"));
        registry
            .register_contribution(&mut shaping, &valid)
            .unwrap();

        let invalid = PluginContribution::new("other")
            .function(TestFunction {
                name: "demo::ok",
                owner: FunctionOwner::Package("other".into()),
            })
            .element(schema("other", "new"));
        assert!(matches!(
            registry.register_contribution(&mut shaping, &invalid),
            Err(RegistryError {
                reason: RegistryErrorReason::Duplicate,
                ..
            })
        ));
        assert!(registry.get("demo::ok").is_some());
        assert!(shaping.get(&ElementName::plugin("other", "new")).is_none());
    }

    #[test]
    fn function_owner_is_a_package_identity() {
        let function = TestFunction {
            name: "native::value",
            owner: FunctionOwner::Package("native".into()),
        };
        assert_eq!(function.owner(), FunctionOwner::Package("native".into()));
        assert_ne!(function.owner(), FunctionOwner::Package("wasm".into()));
    }
}
