use std::collections::HashMap;
use std::sync::Arc;

use notist_model::{Annotation, Content, TextRange};

use crate::{EvalDiagnostic, Evaluation, lower_fragment};

/// A borrowed source fragment passed unchanged to a processor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawSource<'a> {
    /// The fragment text without the opaque scope delimiters.
    pub text: &'a str,
    /// The fragment range in the original source file.
    pub range: TextRange,
}

/// The syntax-level input supplied to a processor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessorInput<'a> {
    /// The processor name used at the call site.
    pub name: &'a str,
    /// Raw argument text without the surrounding parentheses.
    pub arguments: Option<&'a str>,
    /// Raw body text without the surrounding brackets.
    pub body: RawSource<'a>,
    /// The complete opaque scope range.
    pub range: TextRange,
}

/// Evaluated content and annotations returned by a processor.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProcessorOutput {
    /// Content produced by the processor.
    pub content: Content,
    /// Additional annotations produced while processing the body.
    pub annotations: Vec<Annotation>,
}

impl ProcessorOutput {
    /// Creates an output containing only evaluated content.
    pub fn content(content: Content) -> Self {
        Self {
            content,
            annotations: Vec::new(),
        }
    }

    /// Converts a nested evaluation into processor output.
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

/// A processor that expands an opaque scope into semantic content.
pub trait Processor: Send + Sync {
    /// Returns the globally unique processor name.
    fn name(&self) -> &str;

    /// Processes raw arguments and body source into content.
    fn process(
        &self,
        context: &ProcessContext<'_>,
        input: ProcessorInput<'_>,
    ) -> Result<ProcessorOutput, Vec<EvalDiagnostic>>;
}

/// A registry of built-in and plugin-provided processors.
#[derive(Default)]
pub struct ProcessorRegistry {
    processors: HashMap<String, Arc<dyn Processor>>,
}

impl ProcessorRegistry {
    /// Creates an empty processor registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a processor and rejects duplicate names.
    pub fn register(&mut self, processor: impl Processor + 'static) -> Result<(), RegistryError> {
        let name = processor.name().to_owned();
        if self.processors.contains_key(&name) {
            return Err(RegistryError { name });
        }
        self.processors.insert(name, Arc::new(processor));
        Ok(())
    }

    /// Looks up a processor by its qualified name.
    pub fn get(&self, name: &str) -> Option<&dyn Processor> {
        self.processors.get(name).map(Arc::as_ref)
    }
}

/// An error returned when a processor name is registered more than once.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryError {
    /// The duplicate processor name.
    pub name: String,
}

/// Services available while an opaque scope is being processed.
pub struct ProcessContext<'a> {
    pub(crate) registry: &'a ProcessorRegistry,
    pub(crate) depth: usize,
}

impl ProcessContext<'_> {
    /// Parses and lowers a raw body as nested Notist source.
    pub fn evaluate(&self, source: RawSource<'_>) -> Evaluation {
        const MAX_DEPTH: usize = 64;
        if self.depth >= MAX_DEPTH {
            return Evaluation {
                diagnostics: vec![EvalDiagnostic {
                    message: "processor expansion exceeded the recursion limit".into(),
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
