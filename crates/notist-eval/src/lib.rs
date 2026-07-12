//! Evaluation and structural normalization for Notist documents.

mod function;
mod lower;
mod structure;

use notist_model::{Annotation, Content, TextRange};
use notist_syntax::Parse;

pub use function::{
    CallBody, Function, FunctionContext, FunctionInput, FunctionOutput, FunctionRegistry,
    RawSource, RegistryError,
};
pub use structure::structure;

/// The result of lowering syntax and evaluating content-producing calls.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Evaluation {
    /// Evaluated elements in source order.
    pub content: Content,
    /// Metadata ranges projected from scopes and functions.
    pub annotations: Vec<Annotation>,
    /// Recoverable syntax and evaluation diagnostics.
    pub diagnostics: Vec<EvalDiagnostic>,
}

/// A diagnostic produced while lowering or evaluating content.
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
    /// Diagnostics produced before and during function evaluation.
    pub diagnostics: Vec<EvalDiagnostic>,
}

/// Evaluates Notist source with an empty function registry.
pub fn lower(source: &str, parse: &Parse) -> Evaluation {
    lower::lower_parsed(source, parse, 0, &FunctionRegistry::new(), 0)
}

/// Evaluates Notist source using a configurable function registry.
pub struct Evaluator {
    registry: FunctionRegistry,
}

impl Evaluator {
    /// Creates an evaluator using the provided function registry.
    pub fn new(registry: FunctionRegistry) -> Self {
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

    /// Returns the function registry used by this evaluator.
    pub fn registry(&self) -> &FunctionRegistry {
        &self.registry
    }
}

impl Default for Evaluator {
    fn default() -> Self {
        Self::new(FunctionRegistry::new())
    }
}

pub(crate) fn lower_fragment(
    source: &str,
    base_offset: usize,
    registry: &FunctionRegistry,
    depth: usize,
) -> Evaluation {
    let parse = notist_syntax::parse(source);
    lower::lower_parsed(source, &parse, base_offset, registry, depth)
}

#[cfg(test)]
mod tests {
    use notist_model::{Block, Content, Element, ElementNode, TextRange, UnresolvedCallBody};

    use super::*;

    struct QuoteFunction;

    impl Function for QuoteFunction {
        fn name(&self) -> &str {
            "quote"
        }

        fn call(
            &self,
            _context: &FunctionContext<'_>,
            input: FunctionInput<'_>,
        ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
            let CallBody::Content(body) = input.body else {
                return Err(vec![EvalDiagnostic {
                    message: "quote requires a content body".into(),
                    range: input.range,
                }]);
            };
            Ok(FunctionOutput {
                content: Content::single(
                    Element::Custom {
                        name: "quote".into(),
                        body,
                        block: true,
                    },
                    input.range,
                ),
                annotations: Vec::new(),
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
    fn preserves_unknown_calls_with_syntax_selected_bodies() {
        let content = Evaluator::default().evaluate("#missing(x=1)[[[visible]]]");
        let raw = Evaluator::default().evaluate("#missing![[[ignored]]]");

        assert_eq!(content.diagnostics.len(), 1);
        assert_eq!(content.diagnostics[0].message, "unknown function `missing`");
        assert_eq!(content.content.elements.len(), 1);
        assert!(matches!(
            &content.content.elements[0].element,
            Element::UnresolvedCall {
                name,
                body: UnresolvedCallBody::Content(body),
                ..
            } if name == "missing" && matches!(body.elements[0].element, Element::Reference(_))
        ));
        assert!(matches!(
            &raw.content.elements[0].element,
            Element::UnresolvedCall {
                body: UnresolvedCallBody::Raw(body),
                ..
            } if body == "[[ignored]]"
        ));
    }

    #[test]
    fn content_calls_receive_lowered_notist_content() {
        let mut registry = FunctionRegistry::new();
        registry.register(QuoteFunction).unwrap();
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
    fn registry_rejects_duplicate_function_names() {
        let mut registry = FunctionRegistry::new();
        registry.register(QuoteFunction).unwrap();
        let error = registry.register(QuoteFunction).unwrap_err();
        assert_eq!(error.name, "quote");
    }

    #[test]
    fn structuring_preserves_evaluation_diagnostics() {
        let evaluation = Evaluator::default().evaluate("#missing[body]");
        let structured = structure(evaluation);
        assert_eq!(structured.diagnostics.len(), 1);
        assert_eq!(
            structured.diagnostics[0].message,
            "unknown function `missing`"
        );
    }
}
