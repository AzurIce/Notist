//! Evaluation and structural normalization for Notist documents.

mod lower;
mod processor;
mod structure;

use notist_model::{Annotation, Content, TextRange};
use notist_syntax::Parse;

pub use processor::{
    ProcessContext, Processor, ProcessorInput, ProcessorOutput, ProcessorRegistry, RawSource,
    RegistryError,
};
pub use structure::structure;

/// The result of lowering syntax and expanding opaque scopes.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Evaluation {
    /// Evaluated elements in source order.
    pub content: Content,
    /// Metadata ranges projected from scopes and processors.
    pub annotations: Vec<Annotation>,
    /// Recoverable syntax and evaluation diagnostics.
    pub diagnostics: Vec<EvalDiagnostic>,
}

/// A diagnostic produced while lowering or expanding content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvalDiagnostic {
    /// A user-facing diagnostic message.
    pub message: String,
    /// The original source range associated with the diagnostic.
    pub range: TextRange,
}

/// A structured document together with diagnostics preserved from evaluation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StructuredEvaluation {
    /// The paragraph, list, and block structure derived from evaluated content.
    pub document: notist_model::StructuredDocument,
    /// Diagnostics produced before and during processor expansion.
    pub diagnostics: Vec<EvalDiagnostic>,
}

/// Evaluates Notist source with an empty processor registry.
pub fn lower(source: &str, parse: &Parse) -> Evaluation {
    lower::lower_parsed(source, parse, 0, &ProcessorRegistry::new(), 0)
}

/// Evaluates Notist source using a configurable processor registry.
pub struct Evaluator {
    registry: ProcessorRegistry,
}

impl Evaluator {
    /// Creates an evaluator using the provided processor registry.
    pub fn new(registry: ProcessorRegistry) -> Self {
        Self { registry }
    }

    /// Parses and evaluates a complete source file.
    pub fn evaluate(&self, source: &str) -> Evaluation {
        lower_fragment(source, 0, &self.registry, 0)
    }

    /// Evaluates an already parsed complete source file.
    pub fn evaluate_parsed(&self, source: &str, parse: &Parse) -> Evaluation {
        lower::lower_parsed(source, parse, 0, &self.registry, 0)
    }

    /// Returns the processor registry used by this evaluator.
    pub fn registry(&self) -> &ProcessorRegistry {
        &self.registry
    }
}

impl Default for Evaluator {
    fn default() -> Self {
        Self::new(ProcessorRegistry::new())
    }
}

pub(crate) fn lower_fragment(
    source: &str,
    base_offset: usize,
    registry: &ProcessorRegistry,
    depth: usize,
) -> Evaluation {
    let parse = notist_syntax::parse(source);
    lower::lower_parsed(source, &parse, base_offset, registry, depth)
}

#[cfg(test)]
mod tests {
    use notist_model::{Block, Content, Element, ElementNode, TextRange};

    use super::*;

    struct QuoteProcessor;

    impl Processor for QuoteProcessor {
        fn name(&self) -> &str {
            "quote"
        }

        fn process(
            &self,
            context: &ProcessContext<'_>,
            input: ProcessorInput<'_>,
        ) -> Result<ProcessorOutput, Vec<EvalDiagnostic>> {
            let nested = ProcessorOutput::from_evaluation(context.evaluate(input.body))?;
            Ok(ProcessorOutput {
                content: Content::single(
                    Element::Custom {
                        name: "quote".into(),
                        body: nested.content,
                        block: true,
                    },
                    input.range,
                ),
                annotations: nested.annotations,
            })
        }
    }

    #[test]
    fn lowers_transparent_scopes_references_and_parbreaks() {
        let source = "Hello #[[[self::target]]]@concept,#important\n\nAfter";
        let evaluation = Evaluator::default().evaluate(source);

        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert_eq!(evaluation.content.elements.len(), 4);
        assert!(matches!(
            &evaluation.content.elements[0].element,
            Element::Text(text) if text == "Hello "
        ));
        assert!(matches!(
            evaluation.content.elements[1].element,
            Element::Reference(_)
        ));
        assert!(matches!(
            evaluation.content.elements[2].element,
            Element::Parbreak
        ));
        assert_eq!(evaluation.annotations.len(), 1);
        assert_eq!(
            evaluation.annotations[0].metadata.id.as_deref(),
            Some("concept")
        );
        assert_eq!(evaluation.annotations[0].metadata.tags, ["important"]);
    }

    #[test]
    fn preserves_unknown_opaque_scopes_as_recoverable_elements() {
        let evaluation = Evaluator::default().evaluate("#missing(x=1)[[[ignored]]]");

        assert_eq!(evaluation.diagnostics.len(), 1);
        assert_eq!(
            evaluation.diagnostics[0].message,
            "unknown processor `missing`"
        );
        assert_eq!(evaluation.content.elements.len(), 1);
        assert!(matches!(
            &evaluation.content.elements[0].element,
            Element::UnresolvedProcessor { name, body, .. }
                if name == "missing" && body == "[[ignored]]"
        ));
    }

    #[test]
    fn processors_can_reparse_raw_bodies_as_notist() {
        let mut registry = ProcessorRegistry::new();
        registry.register(QuoteProcessor).unwrap();
        let evaluator = Evaluator::new(registry);
        let evaluation = evaluator.evaluate("Before\n\n#quote[Inside [[self::target]].]\n\nAfter");

        assert!(evaluation.diagnostics.is_empty());
        let structured = structure(evaluation);
        assert_eq!(structured.document.blocks.len(), 3);
        assert!(matches!(structured.document.blocks[0], Block::Paragraph(_)));
        assert!(matches!(structured.document.blocks[1], Block::Element(_)));
        assert!(matches!(structured.document.blocks[2], Block::Paragraph(_)));
    }

    #[test]
    fn structuring_groups_paragraphs_and_adjacent_list_items() {
        let range = TextRange::new(0, 1);
        let item = || ElementNode {
            element: Element::ListItem(Content::single(Element::Text("item".into()), range)),
            range,
        };
        let evaluation = Evaluation {
            content: Content {
                elements: vec![
                    ElementNode {
                        element: Element::Text("intro".into()),
                        range,
                    },
                    ElementNode {
                        element: Element::Parbreak,
                        range,
                    },
                    item(),
                    item(),
                    ElementNode {
                        element: Element::Heading {
                            level: 1,
                            body: Content::single(Element::Text("title".into()), range),
                        },
                        range,
                    },
                    ElementNode {
                        element: Element::Text("tail".into()),
                        range,
                    },
                ],
            },
            ..Evaluation::default()
        };

        let structured = structure(evaluation);
        assert_eq!(structured.document.blocks.len(), 4);
        assert!(matches!(structured.document.blocks[0], Block::Paragraph(_)));
        assert!(matches!(&structured.document.blocks[1], Block::List(items) if items.len() == 2));
        assert!(matches!(structured.document.blocks[2], Block::Element(_)));
        assert!(matches!(structured.document.blocks[3], Block::Paragraph(_)));
    }

    #[test]
    fn registry_rejects_duplicate_processor_names() {
        let mut registry = ProcessorRegistry::new();
        registry.register(QuoteProcessor).unwrap();
        let error = registry.register(QuoteProcessor).unwrap_err();
        assert_eq!(error.name, "quote");
    }

    #[test]
    fn structuring_preserves_evaluation_diagnostics() {
        let evaluation = Evaluator::default().evaluate("#missing[body]");
        let structured = structure(evaluation);
        assert_eq!(structured.diagnostics.len(), 1);
        assert_eq!(
            structured.diagnostics[0].message,
            "unknown processor `missing`"
        );
    }
}
