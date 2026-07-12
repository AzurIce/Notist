use std::collections::HashMap;
use std::sync::Arc;

use notist_model::{Annotation, Content, TextRange};
use notist_syntax::BodyForm;

use crate::{BoundArguments, EvalDiagnostic, Evaluation, FunctionSignature, lower_fragment};

/// A borrowed source fragment passed unchanged to a function.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawSource<'a> {
    /// The fragment text without the call delimiters.
    pub text: &'a str,
    /// The fragment range in the original source file.
    pub range: TextRange,
}

/// The evaluated or raw trailing body supplied to a function.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CallBody<'a> {
    /// A normal call body lowered recursively as Notist content.
    Content(Content),
    /// A raw call body preserved exactly as source text.
    Raw(RawSource<'a>),
}

/// The input supplied to a content-producing function.
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionInput<'a> {
    /// The function name used at the call site.
    pub name: &'a str,
    /// Arguments after expression evaluation and signature binding.
    pub arguments: BoundArguments<'a>,
    /// The syntax-selected trailing body.
    pub body: CallBody<'a>,
    /// Whether the body opener is followed immediately by a newline.
    pub body_form: BodyForm,
    /// The complete call range.
    pub range: TextRange,
}

/// Evaluated content and annotations returned by a function.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FunctionOutput {
    /// Content produced by the function.
    pub content: Content,
    /// Additional annotations produced while evaluating the body.
    pub annotations: Vec<Annotation>,
}

impl FunctionOutput {
    /// Creates an output containing only evaluated content.
    pub fn content(content: Content) -> Self {
        Self {
            content,
            annotations: Vec::new(),
        }
    }

    /// Converts a nested evaluation into function output.
    pub fn from_evaluation(evaluation: Evaluation) -> Result<Self, Vec<EvalDiagnostic>> {
        if evaluation.diagnostics.is_empty() {
            Ok(Self {
                content: evaluation.content,
                annotations: evaluation.annotations,
            })
        } else {
            Err(evaluation.diagnostics)
        }
    }
}

/// A callable that produces semantic content.
pub trait Function: Send + Sync {
    /// Returns the globally unique function name.
    fn name(&self) -> &str;

    /// Returns the statically checkable function signature.
    fn signature(&self) -> FunctionSignature;

    /// Evaluates bound arguments and the syntax-selected body into content.
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

    /// Registers a function and rejects duplicate names.
    pub fn register(&mut self, function: impl Function + 'static) -> Result<(), RegistryError> {
        let name = function.name().to_owned();
        if self.functions.contains_key(&name) {
            return Err(RegistryError { name });
        }
        self.functions.insert(name, Arc::new(function));
        Ok(())
    }

    /// Looks up a function by its qualified name.
    pub fn get(&self, name: &str) -> Option<&dyn Function> {
        self.functions.get(name).map(Arc::as_ref)
    }
}

/// An error returned when a function name is registered more than once.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryError {
    /// The duplicate function name.
    pub name: String,
}

/// Services available while a function is being evaluated.
pub struct FunctionContext<'a> {
    pub(crate) registry: &'a FunctionRegistry,
    pub(crate) depth: usize,
}

impl FunctionContext<'_> {
    /// Explicitly parses and lowers raw source as nested Notist content.
    pub fn evaluate(&self, source: RawSource<'_>) -> Evaluation {
        const MAX_DEPTH: usize = 64;
        if self.depth >= MAX_DEPTH {
            return Evaluation {
                diagnostics: vec![EvalDiagnostic {
                    message: "function evaluation exceeded the recursion limit".into(),
                    range: source.range,
                }],
                ..Evaluation::default()
            };
        }

        lower_fragment(
            source.text,
            source.range.start,
            self.registry,
            self.depth + 1,
        )
    }
}
