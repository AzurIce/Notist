//! Evaluation and structural normalization for Notist documents.

mod call;
#[path = "../../../plugins/core/lib.rs"]
mod core;
mod function;
mod leaf;
mod lower;
mod stream_lower;
mod structure;
mod type_system;

use std::collections::HashMap;

use notist_model::{Content, TextRange};
use notist_syntax::Parse;

pub use call::{Argument, Call, CallContent, CallNode, reduce, reduce_content, reduce_content_as};
pub use function::{
    ElementFunction, Function, FunctionContext, FunctionInput, FunctionOutput, FunctionOwner,
    FunctionRegistry, RegistryError, RegistryErrorReason,
};
pub use leaf::node_engine::{
    NodeEvaluation, collect_names, evaluate_to_nodes, fully_reduced, nodes_to_element_tree,
    reduce_nodes,
};
pub use leaf::{
    ElementTree, FlatContent, LeafEvaluation, ReduceFrame, ReduceLimits, ShapingRegistry,
    StreamArgument, StreamCall, StreamEvaluation, StreamNode, StreamValue,
    element_tree_to_document, field_value_to_element_value, instance_node_to_legacy,
    instances_to_legacy_content, legacy_content_to_nodes, reduce_call, reduce_flat,
    reduce_flat_recovering, shape_flat, shape_flat_with,
};
pub use structure::structure;
pub use type_system::{
    BoundArguments, DefaultValue, FunctionImplementation, FunctionSignature, FunctionValue,
    Parameter, Type, Value, ValueOrigin,
};

/// The result of lowering syntax and evaluating values inserted into Markup.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Evaluation {
    /// Evaluated elements in source order.
    pub content: Content,
    /// Recoverable syntax and evaluation diagnostics.
    pub diagnostics: Vec<EvalDiagnostic>,
    /// The document root scope's own `let` bindings (D0002 evaluation result).
    pub bindings: HashMap<String, Value>,
    /// The side annotation table: element-sequence intervals to attribute
    /// sets (D0002). Ranges are absolute source byte ranges.
    pub annotations: Vec<AnnotationEntry>,
    /// Module-level attributes declared by `@![...]` at the file start
    /// (D0006), bound to the root scope and published as module metadata.
    pub module_attributes: Vec<notist_syntax::Attributes>,
}

/// One entry of the side annotation table (D0002): an attribute set bound to
/// the value produced over one element-sequence interval.
#[derive(Clone, Debug, PartialEq)]
pub struct AnnotationEntry {
    pub range: TextRange,
    pub attributes: notist_syntax::Attributes,
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
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StructuredEvaluation {
    /// The paragraph, list, and block structure derived from evaluated content.
    pub document: notist_model::StructuredDocument,
    /// Diagnostics produced before and during function evaluation.
    pub diagnostics: Vec<EvalDiagnostic>,
    /// The side annotation table (D0002/D0006): postfix `@...` and
    /// block-prefix `@[...]` attribute sets over absolute source ranges.
    pub annotations: Vec<AnnotationEntry>,
}

/// Evaluates Notist source with an empty function registry.
pub fn lower(source: &str, parse: &Parse) -> Evaluation {
    lower::evaluate_markup(
        source,
        &parse.root,
        0,
        &FunctionRegistry::with_builtins(),
        0,
    )
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
        lower::evaluate_markup(source, &parse.root, 0, &self.registry, 0)
    }

    /// Evaluates a parsed source file with a pre-seeded document scope: the
    /// analysis layer injects imported bindings before evaluation (D0004).
    pub fn evaluate_parsed_with_bindings(
        &self,
        source: &str,
        parse: &Parse,
        bindings: HashMap<String, Value>,
    ) -> Evaluation {
        lower::evaluate_markup_with_bindings(source, &parse.root, 0, &self.registry, 0, bindings)
    }

    /// Evaluates source and recursively shapes it into the unified Leaf tree.
    pub fn evaluate_leaf(&self, source: &str) -> LeafEvaluation {
        LeafEvaluation::from_evaluation(&self.evaluate(source))
    }

    /// Runs the full Stream pipeline: parse → lower → reduce → shape.
    pub fn evaluate_stream(&self, source: &str) -> StreamEvaluation {
        self.evaluate_stream_with_shaping(source, ShapingRegistry::core())
    }

    /// Runs the Stream pipeline with a caller-provided shaping registry.
    ///
    /// Plugin packages contribute their element schemas through the snapshot
    /// shaping registry; this is the entry point that applies them while
    /// folding the reduced Leaf stream into the canonical tree.
    pub fn evaluate_stream_with_shaping(
        &self,
        source: &str,
        shaping: &ShapingRegistry,
    ) -> StreamEvaluation {
        let parse = notist_syntax::parse(source);
        self.evaluate_parsed_stream_with_bindings(source, &parse, HashMap::new(), shaping)
    }

    /// Runs the unified-node pipeline: parse → lower → node reduction.
    ///
    /// The result carries both the reduced `Node` forest and the shaped
    /// canonical tree projected through the instance adapter.
    pub fn evaluate_nodes(&self, source: &str) -> NodeEvaluation {
        self.evaluate_nodes_with_shaping(source, ShapingRegistry::core())
    }

    /// Like [`Self::evaluate_nodes`] with a caller-provided shaping registry.
    pub fn evaluate_nodes_with_shaping(
        &self,
        source: &str,
        shaping: &ShapingRegistry,
    ) -> NodeEvaluation {
        let parse = notist_syntax::parse(source);
        self.evaluate_parsed_nodes_with_bindings(source, &parse, HashMap::new(), shaping)
    }

    /// Unified-node variant of the pre-parsed bindings entry point.
    pub fn evaluate_parsed_nodes_with_bindings(
        &self,
        source: &str,
        parse: &Parse,
        bindings: HashMap<String, Value>,
        shaping: &ShapingRegistry,
    ) -> NodeEvaluation {
        let lowered = stream_lower::lower_markup_stream_with_bindings(
            source,
            &parse.root,
            0,
            &self.registry,
            bindings,
        );
        let mut evaluation =
            leaf::node_engine::evaluate_to_nodes(&lowered.flat, &self.registry, shaping);
        evaluation
            .diagnostics
            .extend(parse.errors.iter().cloned().map(|error| EvalDiagnostic {
                message: error.message,
                range: error.range,
            }));
        evaluation.diagnostics.extend(lowered.diagnostics);
        evaluation
    }

    /// Runs the Stream pipeline for an already parsed source with pre-seeded
    /// root bindings and a caller-provided shaping registry.
    pub fn evaluate_parsed_stream_with_bindings(
        &self,
        source: &str,
        parse: &Parse,
        bindings: HashMap<String, Value>,
        shaping: &ShapingRegistry,
    ) -> StreamEvaluation {
        let lowered = stream_lower::lower_markup_stream_with_bindings(
            source,
            &parse.root,
            0,
            &self.registry,
            bindings,
        );
        // Production reduction runs on the unified-node engine; the legacy
        // stream shapes are rebuilt from its terminal forest for consumers
        // that still speak them.
        let mut evaluation =
            leaf::node_engine::evaluate_to_nodes(&lowered.flat, &self.registry, shaping);
        let reduction_failed = evaluation.forest.is_empty() && !evaluation.diagnostics.is_empty();
        let leaves = evaluation
            .forest
            .iter()
            .map(notist_model::node_to_instance)
            .collect::<Result<Vec<_>, String>>();
        let (reduced, tree) = match leaves {
            Ok(leaves) => (
                FlatContent {
                    nodes: leaves.iter().cloned().map(StreamNode::Leaf).collect(),
                },
                evaluation.tree,
            ),
            Err(message) => {
                evaluation.diagnostics.push(EvalDiagnostic {
                    message,
                    range: notist_model::TextRange::new(0, 0),
                });
                (FlatContent::new(), crate::leaf::ElementTree::default())
            }
        };
        let mut diagnostics = Vec::new();
        diagnostics.extend(parse.errors.iter().cloned().map(|error| EvalDiagnostic {
            message: error.message,
            range: error.range,
        }));
        diagnostics.extend(lowered.diagnostics);
        diagnostics.append(&mut evaluation.diagnostics);
        StreamEvaluation {
            lowered: lowered.flat,
            reduced,
            tree,
            diagnostics,
            bindings: lowered.bindings,
            annotations: lowered.annotations,
            module_attributes: lowered.module_attributes,
            reduction_failed,
        }
    }

    /// Evaluates parsed source and recursively shapes it into the unified Leaf tree.
    pub fn evaluate_parsed_leaf(&self, source: &str, parse: &Parse) -> LeafEvaluation {
        LeafEvaluation::from_evaluation(&self.evaluate_parsed(source, parse))
    }

    /// Returns the function registry used by this evaluator.
    pub fn registry(&self) -> &FunctionRegistry {
        &self.registry
    }
}

impl Default for Evaluator {
    fn default() -> Self {
        Self::new(FunctionRegistry::with_builtins())
    }
}

pub(crate) fn lower_fragment(
    source: &str,
    base_offset: usize,
    registry: &FunctionRegistry,
    depth: usize,
) -> Evaluation {
    let parse = notist_syntax::parse(source);
    lower::evaluate_markup(source, &parse.root, base_offset, registry, depth)
}

#[cfg(test)]
mod tests {
    use notist_model::{Block, Content, Element, ElementNode, TableAlignment, TextRange};

    use super::*;

    #[test]
    fn let_bindings_flow_into_later_markup() {
        // D0001 minimal example: the bound value feeds a later heading and
        // enters the evaluation result's bindings.
        let evaluation = Evaluator::default().evaluate("#let accent = \"violet\"\n\n= #accent\n");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let Some(heading) = evaluation
            .content
            .elements
            .iter()
            .find(|node| matches!(node.element, Element::Heading { .. }))
        else {
            panic!("expected a heading, got {:?}", evaluation.content.elements)
        };
        let Element::Heading { body, .. } = &heading.element else {
            unreachable!()
        };
        assert!(matches!(
            body.elements.as_slice(),
            [ElementNode {
                element: Element::Text(text),
                ..
            }] if text == "violet"
        ));
        assert_eq!(
            evaluation.bindings.get("accent"),
            Some(&Value::String("violet".into()))
        );
    }

    #[test]
    fn if_expression_selects_branches_and_omitting_else_yields_none() {
        let yes = Evaluator::default().evaluate("#if true [yes] else [no]");
        assert!(yes.diagnostics.is_empty(), "{:?}", yes.diagnostics);
        assert!(matches!(
            yes.content.elements.as_slice(),
            [ElementNode {
                element: Element::Text(text),
                ..
            }] if text == "yes"
        ));
        let no = Evaluator::default().evaluate("#if false [yes] else [no]");
        assert!(no.diagnostics.is_empty(), "{:?}", no.diagnostics);
        assert!(matches!(
            no.content.elements.as_slice(),
            [ElementNode {
                element: Element::Text(text),
                ..
            }] if text == "no"
        ));
        let missing = Evaluator::default().evaluate("#if false [yes]");
        assert!(missing.diagnostics.is_empty(), "{:?}", missing.diagnostics);
        assert!(missing.content.elements.is_empty());
    }

    #[test]
    fn functions_are_first_class_closures() {
        // D0003: builtin constructors are first-class values.
        let evaluation =
            Evaluator::default().evaluate("#let make_title = heading\n#make_title[标题]");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert!(evaluation.content.elements.iter().any(|node| {
            matches!(
                node,
                ElementNode {
                    element:
                        Element::Heading {
                            body:
                                Content {
                                    elements: heading_body,
                                },
                            ..
                        },
                    ..
                } if matches!(
                    heading_body.as_slice(),
                    [ElementNode {
                        element: Element::Text(text),
                        ..
                    }] if text == "标题"
                )
            )
        }));
        // Lambda closures evaluate their body in the captured environment.
        let evaluation =
            Evaluator::default().evaluate("#let double = (x: Int) => x * 2\n#double(21)");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert!(
            evaluation
                .content
                .elements
                .iter()
                .any(|node| { matches!(&node.element, Element::Text(text) if text == "42") })
        );
    }

    #[test]
    fn code_block_joins_statement_values_and_scopes_lets() {
        // A block's value is the join of its statements (D0006).
        let evaluation = Evaluator::default().evaluate("#let x = { 1 + 2 }");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert_eq!(evaluation.bindings.get("x"), Some(&Value::Int(3)));
        // Content statements join into one Content value.
        let joined = Evaluator::default().evaluate("#let y = { [a] [b] }");
        assert!(joined.diagnostics.is_empty(), "{:?}", joined.diagnostics);
        let Some(Value::Content(content)) = joined.bindings.get("y") else {
            panic!("expected a Content binding")
        };
        assert_eq!(content.elements.len(), 2);
    }

    struct QuoteFunction;

    impl Function for QuoteFunction {
        fn name(&self) -> &str {
            "test::quote"
        }

        fn signature(&self) -> FunctionSignature {
            FunctionSignature {
                parameters: vec![Parameter {
                    name: "body".into(),
                    ty: Type::Content,
                    default: None,
                }],
                trailing_content: Some("body".into()),
                result: Type::Content,
            }
        }

        fn call(
            &self,
            _context: &FunctionContext<'_>,
            mut input: FunctionInput<'_>,
        ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
            let body = input.arguments.take_content("body");
            Ok(FunctionOutput::content(Content::single(
                Element::Custom {
                    name: "quote".into(),
                    body,
                    block: true,
                    fields: Vec::new(),
                },
                input.range,
            )))
        }
    }

    struct TwoFunction;

    impl Function for TwoFunction {
        fn name(&self) -> &str {
            "test::two"
        }

        fn signature(&self) -> FunctionSignature {
            FunctionSignature {
                parameters: Vec::new(),
                trailing_content: None,
                result: Type::Int,
            }
        }

        fn call(
            &self,
            _context: &FunctionContext<'_>,
            _input: FunctionInput<'_>,
        ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
            Ok(FunctionOutput::value(Value::Int(2)))
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
    }

    #[test]
    fn preserves_unknown_calls_with_optional_trailing_content() {
        let content = Evaluator::default().evaluate("#missing(x=1)[[[self::target]]]");
        let bodyless = Evaluator::default().evaluate("#missing(x=1)");

        assert!(content.diagnostics.is_empty(), "{:?}", content.diagnostics);
        assert_eq!(content.content.elements.len(), 1);
        assert!(matches!(
        &content.content.elements[0].element,
            Element::UnresolvedCall {
                name,
                trailing: Some(body),
                ..
            } if name == "missing" && matches!(body.elements[0].element, Element::Reference(_))
        ));
        assert!(matches!(
            &bodyless.content.elements[0].element,
            Element::UnresolvedCall { trailing: None, .. }
        ));
    }

    #[test]
    fn content_calls_receive_lowered_notist_content() {
        let mut registry = FunctionRegistry::new();
        registry.register(QuoteFunction).unwrap();
        let evaluator = Evaluator::new(registry);
        let evaluation =
            evaluator.evaluate("Before\n\n#test::quote[Inside [[self::target]].]\n\nAfter");

        assert!(evaluation.diagnostics.is_empty());
        let structured = structure(evaluation);
        assert_eq!(structured.document.blocks.len(), 3);
        assert!(matches!(
            &structured.document.blocks[0],
            Block::Element(node) if matches!(node.element, Element::Paragraph(_))
        ));
        assert!(matches!(structured.document.blocks[1], Block::Element(_)));
        assert!(matches!(
            &structured.document.blocks[2],
            Block::Element(node) if matches!(node.element, Element::Paragraph(_))
        ));
    }

    #[test]
    fn plain_markup_produces_text_elements() {
        let evaluator = Evaluator::default();
        let plain = evaluator.evaluate("plain text");
        assert!(plain.diagnostics.is_empty());
        assert!(matches!(
            plain.content.elements[0].element,
            Element::Text(ref text) if text == "plain text"
        ));
    }

    #[test]
    fn structuring_groups_plain_paragraphs() {
        let evaluator = Evaluator::default();
        let structured = structure(evaluator.evaluate("plain *content*"));
        assert!(matches!(
            structured.document.blocks.as_slice(),
            [Block::Element(node)]
                if matches!(&node.element, Element::Paragraph(body)
                    if body.elements.iter().any(|child| matches!(child.element, Element::Strong(_))))
        ));
    }

    #[test]
    fn lowers_backtick_and_fenced_raw_sugar() {
        let evaluator = Evaluator::default();

        let inline = evaluator.evaluate("Before `cargo test` after");
        assert!(inline.diagnostics.is_empty(), "{:?}", inline.diagnostics);
        assert!(matches!(
            &inline.content.elements[1].element,
            Element::Raw {
                text,
                block: false,
                language: None,
            } if text == "cargo test"
        ));

        let fenced = evaluator.evaluate("```rust\nfn main() {}\n```");
        assert!(fenced.diagnostics.is_empty(), "{:?}", fenced.diagnostics);
        assert!(matches!(
            &fenced.content.elements[0].element,
            Element::Raw {
                text,
                block: true,
                language: Some(language),
            } if text == "fn main() {}" && language == "rust"
        ));

        let explicit = evaluator.evaluate("#raw(r#\"cargo test\"#)");
        assert!(
            explicit.diagnostics.is_empty(),
            "{:?}",
            explicit.diagnostics
        );
        assert_eq!(
            inline.content.elements[1].element,
            explicit.content.elements[0].element
        );

        let without_builtins = Evaluator::new(FunctionRegistry::new()).evaluate("`core raw`");
        assert!(
            without_builtins.diagnostics.is_empty(),
            "{:?}",
            without_builtins.diagnostics
        );
        assert!(matches!(
            &without_builtins.content.elements[0].element,
            Element::Raw { text, .. } if text == "core raw"
        ));
    }

    #[test]
    fn lowers_headings_inside_long_form_markup() {
        let evaluated = Evaluator::default().evaluate("= Title\n\nIntro\n\nOutro");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            evaluated.content.elements.first().map(|node| &node.element),
            Some(Element::Heading { level: 1, .. })
        ));
        assert!(
            evaluated
                .content
                .elements
                .iter()
                .any(|node| matches!(&node.element, Element::Text(text) if text.contains("Outro")))
        );
    }

    #[test]
    fn lowers_fenced_raw_block_inside_list_item_body() {
        // An indented fenced raw block belongs to the row body: it lowers
        // into the ListItem content instead of escaping as a sibling.
        let evaluated = Evaluator::default().evaluate("- item\n  ```not\n  x\n  ```\n- next");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            evaluated.content.elements.as_slice(),
            [ElementNode { element: Element::ListItem(first), .. }, ElementNode { element: Element::ListItem(_), .. }]
                if first
                    .elements
                    .iter()
                    .any(|node| matches!(&node.element, Element::Raw { block: true, .. }))
        ));
    }

    #[test]
    fn lowers_indented_mixed_nested_lists() {
        let evaluated =
            Evaluator::default().evaluate("- parent\n  + first child\n  + second child\n- sibling");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            evaluated.content.elements.as_slice(),
            [ElementNode { element: Element::ListItem(parent), .. }, ElementNode { element: Element::ListItem(_), .. }]
                if matches!(parent.elements[1].element, Element::EnumItem { .. })
                    && matches!(parent.elements[2].element, Element::EnumItem { .. })
        ));
        let structured = structure(evaluated);
        assert!(matches!(
            &structured.document.blocks[0],
            Block::Element(ElementNode {
                element: Element::List { ordered: false, items },
                ..
            }) if items.len() == 2
        ));
    }

    #[test]
    fn lowers_orphan_indented_list_before_shallower_list() {
        let evaluated = Evaluator::default().evaluate("= t\n\n  - x\n+ y");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(
            evaluated
                .content
                .elements
                .iter()
                .any(|node| matches!(node.element, Element::ListItem(_)))
        );
        assert!(
            evaluated
                .content
                .elements
                .iter()
                .any(|node| matches!(node.element, Element::EnumItem { .. }))
        );
    }

    #[test]
    fn reserves_asterisks_for_inline_strong() {
        let evaluated = Evaluator::default().evaluate("* item");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(
            !evaluated
                .content
                .elements
                .iter()
                .any(|node| matches!(node.element, Element::ListItem(_)))
        );

        let inline = Evaluator::default().evaluate("*strong*");
        assert!(matches!(
            inline.content.elements.as_slice(),
            [ElementNode {
                element: Element::Strong(_),
                ..
            }]
        ));
    }

    #[test]
    fn lowers_escaped_inline_punctuation_as_literal_text() {
        let evaluated = Evaluator::default().evaluate("\\*not strong\\* and \\|pipe\\|");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(
            !evaluated
                .content
                .elements
                .iter()
                .any(|node| matches!(node.element, Element::Strong(_)))
        );
        assert_eq!(
            evaluated
                .content
                .elements
                .iter()
                .filter_map(|node| match &node.element {
                    Element::Text(value) => Some(value.as_str()),
                    _ => None,
                })
                .collect::<String>(),
            "*not strong* and |pipe|"
        );
    }

    #[test]
    fn numbered_markdown_lists_remain_text() {
        let evaluated = Evaluator::default().evaluate("3) third\n  7) nested\n9) ninth");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(
            !evaluated
                .content
                .elements
                .iter()
                .any(|node| matches!(node.element, Element::EnumItem { .. }))
        );
    }

    #[test]
    fn lowers_inline_surface_sugar() {
        // Bare URLs and forced linebreaks are no longer first-class sugar:
        // they remain ordinary text (D0003 deferral).
        let evaluated =
            Evaluator::default().evaluate("*bold* _slanted_ https://example.test/page.");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            evaluated.content.elements[0].element,
            Element::Strong(_)
        ));
        assert!(matches!(
            evaluated.content.elements[1].element,
            Element::Text(_)
        ));
        assert!(matches!(
            evaluated.content.elements[2].element,
            Element::Emph(_)
        ));
        assert!(evaluated.content.elements.iter().any(|node| {
            matches!(
                &node.element,
                Element::Text(text) if text.contains("https://example.test/page.")
            )
        }));
    }

    #[test]
    fn markdown_image_syntax_remains_text() {
        let evaluated = Evaluator::default().evaluate("![Flow diagram](images/flow.png)");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(
            evaluated.content.elements.iter().any(
                |node| matches!(&node.element, Element::Text(text) if text.contains("flow.png"))
            )
        );
    }

    #[test]
    fn markdown_named_link_syntax_remains_text() {
        let evaluated = Evaluator::default().evaluate("[Notist](docs/index.html)");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(evaluated.content.elements.iter().any(
            |node| matches!(&node.element, Element::Text(text) if text.contains("docs/index.html"))
        ));
    }

    #[test]
    fn lowers_bare_email_addresses_as_plain_text() {
        // D0003 deferral: bare emails no longer produce mailto links.
        let evaluated = Evaluator::default().evaluate("Write hello+docs@example.test.");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(
            evaluated
                .content
                .elements
                .iter()
                .any(|node| matches!(&node.element, Element::Text(value) if value.contains("hello+docs@example.test")))
        );
    }

    #[test]
    fn evaluates_explicit_callout_function() {
        let evaluated =
            Evaluator::default().evaluate("#callout(kind=\"warning\")[*Check* the configuration]");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Callout { kind, body, .. }
                if kind == "warning" && matches!(body.elements[0].element, Element::Strong(_))
        ));
    }

    #[test]
    fn evaluates_explicit_details_function() {
        let evaluated =
            Evaluator::default().evaluate("#details(summary=[*More*])[Hidden _content_]");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Details { summary: Some(summary), open: false, body }
                if matches!(summary.elements[0].element, Element::Strong(_))
                    && body.elements.iter().any(|node| matches!(node.element, Element::Emph(_)))
        ));
    }

    #[test]
    fn lowers_strike_surface_sugar() {
        let evaluated = Evaluator::default().evaluate("Before ~~old *value*~~ after");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[1].element,
            Element::Strike(body)
                if body.elements.iter().any(|node| matches!(node.element, Element::Strong(_)))
        ));

        let unclosed = Evaluator::default().evaluate("keep ~~literal");
        assert!(matches!(
            &unclosed.content.elements[0].element,
            Element::Text(text) if text == "keep ~~literal"
        ));
        let empty = Evaluator::default().evaluate("keep ~~~~ literal");
        assert!(matches!(
            &empty.content.elements[0].element,
            Element::Text(text) if text == "keep ~~~~ literal"
        ));
    }

    #[test]
    fn lowers_heading_surface_sugar() {
        let evaluator = Evaluator::default();
        let evaluated = evaluator.evaluate("= Title\n== Subtitle");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            evaluated.content.elements[0].element,
            Element::Heading { level: 1, .. }
        ));
        assert!(matches!(
            evaluated.content.elements[1].element,
            Element::Heading { level: 2, .. }
        ));
        // D0003 boundary: a line of only `=` is an empty-body heading and a
        // line of only `-` (three or more) is a rule — Markdown setext
        // underlines do not survive as text.
        let setext = evaluator.evaluate("Main *title*\n==========\n\nSubtitle\n--------");
        assert!(setext.diagnostics.is_empty(), "{:?}", setext.diagnostics);
        assert!(
            setext
                .content
                .elements
                .iter()
                .any(|node| matches!(node.element, Element::Heading { level: 10, .. }))
        );
        assert!(
            setext
                .content
                .elements
                .iter()
                .any(|node| matches!(node.element, Element::Rule))
        );
    }

    #[test]
    fn does_not_lower_quote_marker_sugar() {
        // Quote is not part of the v1 language (R04): the marker stays text.
        let evaluated = Evaluator::default().evaluate("> > Nested *quotation*");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(
            evaluated.content.elements.iter().any(
                |node| matches!(&node.element, Element::Text(text) if text.contains("Nested"))
            )
        );
    }

    #[test]
    fn rule_sugar_lowers_dashes_but_star_breaks_stay_text() {
        // D0003: `---` is rule sugar; `***` and `___` have no sugar and stay
        // ordinary text.
        let evaluated = Evaluator::default().evaluate("---\n***\n___");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(
            evaluated
                .content
                .elements
                .iter()
                .any(|node| matches!(node.element, Element::Rule))
        );
        assert!(
            evaluated
                .content
                .elements
                .iter()
                .any(|node| matches!(&node.element, Element::Text(text) if text.contains("***")))
        );
        assert!(
            evaluated
                .content
                .elements
                .iter()
                .any(|node| matches!(&node.element, Element::Text(text) if text.contains("___")))
        );
    }

    #[test]
    fn lowers_pipe_table_sugar_to_table_element() {
        let evaluated = Evaluator::default()
            .evaluate("| Name | Value |\n| :--- | ---: |\n| one | 1 |\n| two | 2 |\n");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        let Element::Table {
            columns,
            header,
            alignments,
            cells,
            ..
        } = &evaluated.content.elements[0].element
        else {
            panic!("expected a table, got {:?}", evaluated.content.elements)
        };
        assert_eq!(*columns, 2);
        assert!(*header);
        assert_eq!(alignments, &[TableAlignment::Left, TableAlignment::Right]);
        assert_eq!(cells.len(), 6);
        assert!(matches!(
            &cells[2].element,
            Element::TableCell { body, .. }
                if matches!(&body.elements[0].element, Element::Text(text) if text == "one")
        ));

        let structured = structure(evaluated);
        assert!(matches!(
            structured.document.blocks.as_slice(),
            [Block::Element(ElementNode {
                element: Element::Table { .. },
                ..
            })]
        ));
    }

    #[test]
    fn evaluates_explicit_table_and_table_cell_constructors() {
        let evaluator = Evaluator::default();
        let evaluated = evaluator.evaluate(
            "#table(columns: 2, header: true, align: \"left, right\")[\n  #table-cell[Name] #table-cell[Value]\n  #table-cell[one] #table-cell[two]\n]",
        );
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Table {
                columns: 2,
                header: true,
                cells,
                ..
            } if cells.len() == 4
        ));

        let incomplete = evaluator.evaluate("#table(columns: 2)[#table-cell[A]]");
        assert!(
            incomplete
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("does not fill"))
        );
        let non_cell = evaluator.evaluate("#table(columns: 2)[#strong[A] #strong[B]]");
        assert!(
            non_cell
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("only table-cell"))
        );
    }

    #[test]
    fn figure_wraps_captioned_block_content_with_typst_style_kind() {
        let evaluator = Evaluator::default();
        let evaluated = evaluator.evaluate(
            "#figure(caption: [Cap], supplement: [Tab], kind: \"table\")[\n  #table(columns: 2)[#table-cell[A] #table-cell[B]]\n]",
        );
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        let Element::Figure {
            body,
            kind,
            supplement,
            caption,
        } = &evaluated.content.elements[0].element
        else {
            panic!("expected a figure, got {:?}", evaluated.content.elements)
        };
        assert_eq!(kind, "table");
        assert!(
            body.elements
                .iter()
                .any(|node| matches!(node.element, Element::Table { .. }))
        );
        assert!(matches!(
            supplement,
            Some(Content { elements }) if matches!(&elements[0].element, Element::Text(text) if text == "Tab")
        ));
        assert!(matches!(
            caption,
            Some(Content { elements }) if matches!(&elements[0].element, Element::Text(text) if text == "Cap")
        ));

        // Typst `kind: auto`: the wrapped block element decides the kind.
        let inferred = evaluator.evaluate("#figure[\n#table(columns: 1)[#table-cell[X]]\n]");
        assert!(
            inferred.diagnostics.is_empty(),
            "{:?}",
            inferred.diagnostics
        );
        assert!(matches!(
            &inferred.content.elements[0].element,
            Element::Figure { kind, .. } if kind == "table"
        ));

        let structured = structure(evaluated);
        assert!(matches!(
            structured.document.blocks.as_slice(),
            [Block::Element(ElementNode {
                element: Element::Figure { .. },
                ..
            })]
        ));
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
        // D0002 section grouping: the heading and its following content form
        // a Section node.
        assert_eq!(structured.document.blocks.len(), 3);
        assert!(matches!(
            &structured.document.blocks[0],
            Block::Element(node) if matches!(node.element, Element::Paragraph(_))
        ));
        assert!(matches!(
            &structured.document.blocks[1],
            Block::Element(ElementNode {
                element: Element::List { ordered: false, items },
                ..
            }) if items.len() == 2
        ));
        let Block::Section { level, body, .. } = &structured.document.blocks[2] else {
            panic!(
                "expected a section, got {:?}",
                structured.document.blocks[2]
            )
        };
        assert_eq!(*level, 1);
        assert!(matches!(
            body.as_slice(),
            [Block::Element(node)] if matches!(node.element, Element::Paragraph(_))
        ));
    }

    #[test]
    fn structuring_unifies_list_sugar_and_item_calls() {
        let evaluator = Evaluator::default();
        for source in ["- One\n- Two", "#item[One]#item[Two]"] {
            let structured = structure(evaluator.evaluate(source));
            assert!(matches!(
                structured.document.blocks.as_slice(),
                [Block::Element(ElementNode {
                    element: Element::List { ordered: false, items },
                    ..
                })] if items.len() == 2
            ));
        }
        for source in [
            "+ Three\n+ Four",
            "#item(ordered=true)[Three]#item(ordered=true)[Four]",
        ] {
            let structured = structure(evaluator.evaluate(source));
            assert!(matches!(
                structured.document.blocks.as_slice(),
                [Block::Element(ElementNode {
                    element: Element::List { ordered: true, items },
                    ..
                })] if items.len() == 2
            ));
        }
    }

    #[test]
    fn structuring_groups_ordered_items_separately() {
        let evaluation = Evaluator::default()
            .evaluate("#item(ordered=true)[First]\n#item(ordered=true)[Second]\n#item[Other]");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let structured = structure(evaluation);
        assert!(matches!(
            &structured.document.blocks[0],
            Block::Element(ElementNode {
                element: Element::List { ordered: true, items },
                ..
            }) if items.len() == 2
        ));
        assert!(matches!(
            &structured.document.blocks[1],
            Block::Element(ElementNode {
                element: Element::List { ordered: false, items },
                ..
            }) if items.len() == 1
        ));
    }

    #[test]
    fn registry_rejects_duplicate_function_names() {
        let mut registry = FunctionRegistry::new();
        registry.register(QuoteFunction).unwrap();
        let error = registry.register(QuoteFunction).unwrap_err();
        assert_eq!(error.name, "test::quote");
    }

    #[test]
    fn element_function_projects_schema_fields_and_trailing_content() {
        let signature = FunctionSignature {
            parameters: vec![
                Parameter {
                    name: "source".into(),
                    ty: Type::String,
                    default: None,
                },
                Parameter {
                    name: "width".into(),
                    ty: Type::Int,
                    default: Some(DefaultValue::Int(800)),
                },
                Parameter {
                    name: "body".into(),
                    ty: Type::Content,
                    default: None,
                },
            ],
            trailing_content: Some("body".into()),
            result: Type::Content,
        };
        let mut registry = FunctionRegistry::new();
        registry
            .register(ElementFunction::new(
                "demo::box",
                signature,
                true,
                FunctionOwner::Plugin("demo".into()),
            ))
            .unwrap();
        let evaluation = Evaluator::new(registry).evaluate("#demo::box(source: \"wgsl\")[Hi]");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let Element::Custom {
            name,
            body,
            block,
            fields,
        } = &evaluation.content.elements[0].element
        else {
            panic!(
                "expected custom element, got {:?}",
                evaluation.content.elements
            )
        };
        assert_eq!(name, "demo::box");
        assert!(*block);
        assert!(matches!(&body.elements[0].element, Element::Text(text) if text == "Hi"));
        assert!(fields.iter().any(|field| field.name == "source"
            && matches!(&field.value, notist_model::ElementValue::String(value) if value == "wgsl")));
        assert!(fields.iter().any(|field| field.name == "width"
            && matches!(&field.value, notist_model::ElementValue::Int(value) if *value == 800)));
    }

    #[test]
    fn element_function_without_trailing_content_stays_bodyless() {
        let signature = FunctionSignature {
            parameters: vec![Parameter {
                name: "label".into(),
                ty: Type::String,
                default: None,
            }],
            trailing_content: None,
            result: Type::Content,
        };
        let mut registry = FunctionRegistry::new();
        registry
            .register(ElementFunction::new(
                "demo::badge",
                signature,
                false,
                FunctionOwner::Plugin("demo".into()),
            ))
            .unwrap();
        let evaluation = Evaluator::new(registry).evaluate("#demo::badge(label: \"x\")");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let Element::Custom { body, fields, .. } = &evaluation.content.elements[0].element else {
            panic!("expected custom element")
        };
        assert!(body.elements.is_empty());
        assert_eq!(fields.len(), 1);
    }

    #[test]
    fn stream_reduction_preserves_siblings_around_failed_calls() {
        let evaluation =
            Evaluator::default().evaluate_stream("Before\n\n#heading(level: 0)[bad]\n\nAfter");
        assert!(
            evaluation
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("heading level")),
            "{:?}",
            evaluation.diagnostics
        );
        assert!(!evaluation.reduction_failed);
        assert!(
            evaluation.reduced.nodes.iter().any(
                |node| matches!(node, StreamNode::Leaf(leaf) if leaf.instance.is_core("text"))
            ),
            "{:#?}",
            evaluation.reduced
        );
        assert_eq!(evaluation.tree.roots.len(), 2);
    }

    #[test]
    fn parsed_stream_evaluation_accepts_import_seed_bindings() {
        let source = "#heading[#title]";
        let parse = notist_syntax::parse(source);
        let bindings = HashMap::from([("title".to_owned(), Value::String("Imported".to_owned()))]);
        let evaluation = Evaluator::default().evaluate_parsed_stream_with_bindings(
            source,
            &parse,
            bindings,
            ShapingRegistry::core(),
        );
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert_eq!(evaluation.tree.roots.len(), 1);
        let section = &evaluation.tree.roots[0].instance;
        assert!(section.is_core("section"));
        let heading = section
            .body
            .iter()
            .find(|node| node.instance.is_core("heading"))
            .expect("section contains its heading");
        assert!(heading.instance.body.iter().any(|node| {
            matches!(&node.instance.field("text"), Some(notist_model::FieldValue::String(text)) if text == "Imported")
        }));
    }

    #[test]
    fn native_functions_can_return_values_to_nested_expressions() {
        let mut registry = FunctionRegistry::with_builtins();
        registry.register(TwoFunction).unwrap();
        let evaluated = Evaluator::new(registry).evaluate("#heading(level=test::two())[Title]");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            evaluated.content.elements[0].element,
            Element::Heading { level: 2, .. }
        ));
    }

    struct SignatureFunction {
        signature: FunctionSignature,
    }

    impl Function for SignatureFunction {
        fn name(&self) -> &str {
            "test::custom"
        }

        fn signature(&self) -> FunctionSignature {
            self.signature.clone()
        }

        fn call(
            &self,
            _context: &FunctionContext<'_>,
            _input: FunctionInput<'_>,
        ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
            Ok(FunctionOutput::content(Content::default()))
        }
    }

    #[test]
    fn registry_validates_signatures_at_registration() {
        let mut registry = FunctionRegistry::new();

        let value_result = registry.register(SignatureFunction {
            signature: FunctionSignature {
                parameters: Vec::new(),
                trailing_content: None,
                result: Type::Int,
            },
        });
        assert!(value_result.is_ok());

        let mut registry = FunctionRegistry::new();
        let mismatched_default = registry.register(SignatureFunction {
            signature: FunctionSignature {
                parameters: vec![Parameter {
                    name: "level".into(),
                    ty: Type::Int,
                    default: Some(DefaultValue::String("one".into())),
                }],
                trailing_content: None,
                result: Type::Content,
            },
        });
        assert!(matches!(
            mismatched_default.unwrap_err().reason,
            RegistryErrorReason::InvalidSignature(message)
                if message.contains("parameter `level`")
        ));

        let mut registry = FunctionRegistry::new();
        let undeclared_trailing = registry.register(SignatureFunction {
            signature: FunctionSignature {
                parameters: Vec::new(),
                trailing_content: Some("body".into()),
                result: Type::Content,
            },
        });
        assert!(matches!(
            undeclared_trailing.unwrap_err().reason,
            RegistryErrorReason::InvalidSignature(message)
                if message.contains("trailing Content parameter `body`")
        ));
    }

    #[test]
    fn structuring_preserves_evaluation_diagnostics() {
        let evaluation = Evaluator::default().evaluate("#missing[body]");
        let structured = structure(evaluation);
        assert!(structured.diagnostics.is_empty());
    }

    #[test]
    fn evaluates_markup_with_string_and_content_interpolation() {
        let evaluation = Evaluator::default().evaluate("a[plain]#\"text\"#[content]z");

        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let texts: Vec<_> = evaluation
            .content
            .elements
            .iter()
            .filter_map(|node| match &node.element {
                Element::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["a[plain]", "text", "content", "z"]);
    }

    #[test]
    fn stringifies_scalar_values_in_markup_position() {
        // D0002 insertion rules: Int/Float/Bool become Text.
        let evaluation = Evaluator::default().evaluate("value: #42");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert!(
            evaluation
                .content
                .elements
                .iter()
                .any(|node| { matches!(&node.element, Element::Text(text) if text == "42") })
        );
    }

    #[test]
    fn ordinary_and_trailing_content_arguments_are_equivalent() {
        let evaluator = Evaluator::default();
        let ordinary = evaluator.evaluate("#details(body=[same])");
        let trailing = evaluator.evaluate("#details[same]");

        assert!(
            ordinary.diagnostics.is_empty(),
            "{:?}",
            ordinary.diagnostics
        );
        assert!(
            trailing.diagnostics.is_empty(),
            "{:?}",
            trailing.diagnostics
        );
        assert!(matches!(
            &ordinary.content.elements[0].element,
            Element::Details { body, .. } if body.elements.len() == 1 && matches!(
                &body.elements[0].element,
                Element::Text(text) if text == "same"
            )
        ));
        assert!(matches!(
            &trailing.content.elements[0].element,
            Element::Details { body, .. } if body.elements.len() == 1 && matches!(
                &body.elements[0].element,
                Element::Text(text) if text == "same"
            )
        ));
    }

    #[test]
    fn source_annotations_do_not_change_evaluation() {
        let evaluator = Evaluator::default();
        let plain = evaluator.evaluate("#[body]");
        let annotated = evaluator.evaluate("#[body]@id,#tag,.class,owner=\"Alice\"");

        assert!(
            annotated.diagnostics.is_empty(),
            "{:?}",
            annotated.diagnostics
        );
        assert_eq!(plain.content, annotated.content);
    }

    #[test]
    fn keeps_markup_comment_syntax_as_text_and_drops_code_trivia() {
        // E09: `//` and `/* ... */` are ordinary text in the Markup stream;
        // only Code contexts strip them as lexical trivia.
        let markup =
            Evaluator::default().evaluate("Visible // line comment\ntext /* outer block */ after");
        assert!(markup.diagnostics.is_empty(), "{:?}", markup.diagnostics);
        let visible = markup
            .content
            .elements
            .iter()
            .filter_map(|node| match &node.element {
                Element::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(
            visible,
            "Visible // line comment\ntext /* outer block */ after"
        );

        let code = Evaluator::default().evaluate("#(1 + /* nested /* block */ comment */ 2)");
        assert!(code.diagnostics.is_empty(), "{:?}", code.diagnostics);
        assert!(matches!(
            &code.content.elements[0].element,
            Element::Text(text) if text == "3"
        ));
    }

    #[test]
    fn block_annotations_bind_the_following_block_node() {
        // D0006: `@[...]` at line start binds the immediately following
        // block-level node (here a heading, then a paragraph).
        let evaluation = Evaluator::default().evaluate("@[wip]\n= Title\n\n@[install]\nabc");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert_eq!(evaluation.annotations.len(), 2);
        // "@[wip]\n" is 7 bytes; the heading spans [7, 14).
        assert_eq!(evaluation.annotations[0].range, TextRange::new(7, 14));
        assert!(evaluation.content.elements.iter().any(|node| {
            matches!(&node.element, Element::Heading { .. }) && node.range == TextRange::new(7, 14)
        }));
        // "@[install]\n" ends at 26; the paragraph's Text node is "\nabc"
        // and spans [26, 30).
        assert_eq!(evaluation.annotations[1].range, TextRange::new(26, 30));
    }

    #[test]
    fn module_annotations_become_module_attributes() {
        let evaluation =
            Evaluator::default().evaluate("@![#design, #wip, status = \"draft\"]\n\n= Title");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert_eq!(evaluation.module_attributes.len(), 1);
        let attributes = &evaluation.module_attributes[0];
        assert!(attributes.items.iter().any(|attribute| {
            matches!(attribute, notist_syntax::Attribute::Tag(name) if name.value == "design")
        }));
        assert!(attributes.items.iter().any(|attribute| {
            matches!(attribute, notist_syntax::Attribute::Tag(name) if name.value == "wip")
        }));
        assert!(attributes.items.iter().any(|attribute| {
            matches!(
                attribute,
                notist_syntax::Attribute::KeyValue { key, value, .. }
                    if key.value == "status" && value.raw == "\"draft\""
            )
        }));
    }

    #[test]
    fn dangling_block_annotations_produce_diagnostics() {
        let evaluation = Evaluator::default().evaluate("@[wip]");
        assert!(evaluation.annotations.is_empty());
        assert!(
            evaluation
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message.contains("not followed by a block") })
        );
    }

    #[test]
    fn bare_code_blocks_insert_their_join_value() {
        // D0006: a bare `{...}` block's join value enters the content stream.
        let evaluation = Evaluator::default().evaluate("before { let x = 1; x + 1 } after");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let visible = evaluation
            .content
            .elements
            .iter()
            .filter_map(|node| match &node.element {
                Element::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(visible, "before 2 after");
    }

    #[test]
    fn element_and_content_scopes_are_lexical_boundaries() {
        // D0002: heading, item, and Content literal bodies are value-level
        // scopes — `let` bindings inside never escape into the document.
        let heading = Evaluator::default().evaluate("= #let x = 1\n\n#x");
        assert!(
            heading
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message == "unresolved name `x`" })
        );
        let item = Evaluator::default().evaluate("- #let y = 2\n#y");
        assert!(
            item.diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message == "unresolved name `y`" })
        );
        let literal = Evaluator::default().evaluate("#[let z = 3]\n#z");
        assert!(
            literal
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message == "unresolved name `z`" })
        );

        // Document-level `let` remains visible inside element bodies (the
        // nested scope sees the chain).
        let visible = Evaluator::default().evaluate("#let accent = \"violet\"\n\n= #accent");
        assert!(visible.diagnostics.is_empty(), "{:?}", visible.diagnostics);
        assert!(visible.content.elements.iter().any(|node| {
            match &node.element {
                Element::Heading { body, .. } => matches!(
                    body.elements.as_slice(),
                    [notist_model::ElementNode { element: Element::Text(text), .. }]
                        if text == "violet"
                ),
                _ => false,
            }
        }));
    }

    #[test]
    fn escaped_closing_delimiters_remain_literal_inline_content() {
        let evaluated = Evaluator::default().evaluate("*left \\* middle* __left \\__ middle__");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Strong(body)
                if body.elements.iter().filter_map(|node| match &node.element {
                    Element::Text(text) => Some(text.as_str()),
                    _ => None,
                }).collect::<String>() == "left * middle"
        ));
        assert!(evaluated.content.elements.iter().any(|node| matches!(
            &node.element,
            Element::Underline(body)
                if body.elements.iter().filter_map(|node| match &node.element {
                    Element::Text(text) => Some(text.as_str()),
                    _ => None,
                }).collect::<String>() == "left __ middle"
        )));
    }

    #[test]
    fn evaluates_user_functions_with_defaults_and_nested_calls() {
        let evaluated = Evaluator::default().evaluate(
            "#let join(left: String, right: String = \"!\") -> String = left + right\n\
             #let greet(name: String = \"World\") -> String = join(\"Hello, \" + name)\n\
             #greet()",
        );
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(
            evaluated.content.elements.iter().any(
                |node| matches!(&node.element, Element::Text(text) if text == "Hello, World!")
            )
        );
    }

    #[test]
    fn evaluates_content_returning_user_functions_in_parameter_scope() {
        let evaluated = Evaluator::default().evaluate(
            "#let warning(title: String = \"Warning\", body: Content) -> Content = #callout(kind: \"note\")[\
             #heading(level=3)[#title]\n#body]\n\
             #warning[hello]",
        );
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(evaluated.content.elements.iter().any(|node| {
            matches!(&node.element, Element::Callout { body, .. }
                if body.elements.iter().any(|child| matches!(&child.element, Element::Heading { level: 3, .. })))
        }));
    }

    #[test]
    fn checks_user_function_results_again_at_runtime() {
        let evaluated =
            Evaluator::default().evaluate("#let broken() -> Int = \"wrong\"\n#broken()");
        assert!(evaluated.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("function `broken` returned String, expected Int")
        }));
    }
}
