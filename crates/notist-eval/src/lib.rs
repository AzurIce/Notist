//! Evaluation and structural normalization for Notist documents.

mod builtin;
mod function;
mod lower;
mod structure;
mod type_system;

use notist_model::{Content, TextRange};
use notist_syntax::Parse;

pub use function::{
    Function, FunctionContext, FunctionInput, FunctionOutput, FunctionRegistry, RegistryError,
    RegistryErrorReason,
};
pub use structure::structure;
pub use type_system::{
    BoundArguments, DefaultValue, FunctionSignature, Parameter, Type, Value, ValueOrigin,
};

/// The result of lowering syntax and evaluating values inserted into Markup.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Evaluation {
    /// Evaluated elements in source order.
    pub content: Content,
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

        assert_eq!(content.diagnostics.len(), 1);
        assert_eq!(content.diagnostics[0].message, "unknown function `missing`");
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
    fn plain_markup_and_text_function_share_the_text_element() {
        let evaluator = Evaluator::default();
        let plain = evaluator.evaluate("plain text");
        let explicit = evaluator.evaluate("#text(\"plain text\")");
        assert!(plain.diagnostics.is_empty());
        assert!(explicit.diagnostics.is_empty());
        assert_eq!(
            plain.content.elements[0].element,
            explicit.content.elements[0].element
        );
    }

    #[test]
    fn structuring_unifies_plain_and_explicit_paragraphs() {
        let evaluator = Evaluator::default();
        for source in ["plain *content*", "#paragraph[plain *content*]"] {
            let structured = structure(evaluator.evaluate(source));
            assert!(matches!(
                structured.document.blocks.as_slice(),
                [Block::Element(node)]
                    if matches!(&node.element, Element::Paragraph(body)
                        if body.elements.iter().any(|child| matches!(child.element, Element::Strong(_))))
            ));
        }
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

        let explicit_inline = evaluator.evaluate("#code(\"cargo test\")");
        assert_eq!(
            explicit_inline.content.elements[0].element,
            inline.content.elements[1].element
        );
        let explicit_block =
            evaluator.evaluate("#code(\"fn main() {}\", lang=\"rust\", block=true)");
        assert_eq!(
            explicit_block.content.elements[0].element,
            fenced.content.elements[0].element
        );

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
    fn lowers_list_and_table_surface_sugar() {
        let evaluator = Evaluator::default();
        let lists = evaluator.evaluate("- first\n- second\n+ third\n+ fourth");
        assert!(lists.diagnostics.is_empty(), "{:?}", lists.diagnostics);
        assert!(matches!(
            lists.content.elements[0].element,
            Element::ListItem(_)
        ));
        assert!(matches!(
            lists.content.elements[1].element,
            Element::ListItem(_)
        ));
        assert!(matches!(
            lists.content.elements[2].element,
            Element::EnumItem { .. }
        ));
        assert!(matches!(
            lists.content.elements[3].element,
            Element::EnumItem { .. }
        ));
        assert!(matches!(
            lists.content.elements[3].element,
            Element::EnumItem { value: None, .. }
        ));

        let table = evaluator.evaluate("| A | B |\n| C | D |\n");
        assert!(table.diagnostics.is_empty(), "{:?}", table.diagnostics);
        assert!(matches!(
            &table.content.elements[0].element,
            Element::Table { columns: 2, header: false, cells, .. } if cells.len() == 4
        ));

        let header_table = evaluator.evaluate("| Name | Value |\n| :--- | ---: |\n| one | two |");
        assert!(
            header_table.diagnostics.is_empty(),
            "{:?}",
            header_table.diagnostics
        );
        assert!(matches!(
            &header_table.content.elements[0].element,
            Element::Table { columns: 2, header: true, alignments, cells, .. }
                if cells.len() == 4
                    && alignments == &[TableAlignment::Left, TableAlignment::Right]
        ));

        let caption_table = evaluator.evaluate("| A | B |\n| C | D |\n: *Inventory*");
        assert!(
            caption_table.diagnostics.is_empty(),
            "{:?}",
            caption_table.diagnostics
        );
        assert!(matches!(
            &caption_table.content.elements[0].element,
            Element::Table { caption: Some(caption), .. }
                if matches!(&caption.elements[0].element, Element::Strong(_))
        ));

        let escaped_pipe = evaluator.evaluate("| A \\| B | C |");
        assert!(
            escaped_pipe.diagnostics.is_empty(),
            "{:?}",
            escaped_pipe.diagnostics
        );
        assert!(matches!(
            &escaped_pipe.content.elements[0].element,
            Element::Table { columns: 2, cells, .. }
                if matches!(&cells[0].element, Element::TableCell { body, .. }
                    if body.elements.iter().map(|node| match &node.element {
                        Element::Text(value) => value.as_str(),
                        _ => "",
                    }).collect::<String>() == "A | B")
        ));

        let uneven = evaluator.evaluate("| a | b | c |\n| one | two |");
        assert!(uneven.diagnostics.is_empty(), "{:?}", uneven.diagnostics);
        assert!(matches!(
            &uneven.content.elements[0].element,
            Element::Table { columns: 3, cells, .. }
                if cells.len() == 6
                    && matches!(&cells[5].element, Element::TableCell { body, .. } if body.is_empty())
        ));
    }

    #[test]
    fn lowers_rich_content_inside_pipe_table_cells() {
        let evaluated = Evaluator::default().evaluate(
            "| Code | Reference | Content |\n| --- | --- | --- |\n| `a|b` | [[guide]] | #strong[x | y] |",
        );
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Table { columns: 3, header: true, cells, .. }
                if matches!(&cells[3].element, Element::TableCell { body, .. }
                    if matches!(&body.elements[0].element, Element::Raw { text, block: false, .. } if text == "a|b"))
                && matches!(&cells[4].element, Element::TableCell { body, .. }
                    if matches!(body.elements[0].element, Element::Reference(_)))
                && matches!(&cells[5].element, Element::TableCell { body, .. }
                    if matches!(body.elements[0].element, Element::Strong(_)))
        ));
    }

    #[test]
    fn lowers_headings_and_tables_inside_long_form_markup() {
        let evaluated = Evaluator::default()
            .evaluate("= Title\n\nIntro\n\n| A | B |\n| --- | --- |\n| one | two |\n\nOutro");
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
                .any(|node| matches!(node.element, Element::Table { header: true, .. }))
        );
        assert!(
            evaluated
                .content
                .elements
                .iter()
                .any(|node| matches!(&node.element, Element::Text(text) if text.contains("Outro")))
        );
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
    fn outdented_list_after_an_indented_span_does_not_recurse() {
        let evaluated = Evaluator::default().evaluate("  - child\n- parent");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            evaluated.content.elements.as_slice(),
            [
                ElementNode {
                    element: Element::ListItem(_),
                    ..
                },
                ElementNode {
                    element: Element::ListItem(_),
                    ..
                }
            ]
        ));
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

        let boundary = Evaluator::default().evaluate("#kbd[A] - B");
        assert!(
            boundary.diagnostics.is_empty(),
            "{:?}",
            boundary.diagnostics
        );
        assert!(matches!(
            boundary.content.elements[0].element,
            Element::Keyboard(_)
        ));
        assert!(matches!(
            &boundary.content.elements[1].element,
            Element::Text(text) if text == " - B"
        ));

        let adjacent = Evaluator::default().evaluate("#raw(text=\"<u>\")*word*");
        assert!(
            adjacent.diagnostics.is_empty(),
            "{:?}",
            adjacent.diagnostics
        );
        assert!(matches!(
            adjacent.content.elements.as_slice(),
            [
                ElementNode {
                    element: Element::Raw { .. },
                    ..
                },
                ElementNode {
                    element: Element::Strong(_),
                    ..
                }
            ]
        ));

        let parenthesized = Evaluator::default().evaluate("#raw(text=(\"a\" + \"b\"))");
        assert!(
            parenthesized.diagnostics.is_empty(),
            "{:?}",
            parenthesized.diagnostics
        );
        assert!(matches!(
            &parenthesized.content.elements[0].element,
            Element::Raw { text, .. } if text == "ab"
        ));
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
    fn lowers_indented_nested_task_lists() {
        let evaluated = Evaluator::default().evaluate("- [ ] Parent\n  - [x] Child\n- [x] Sibling");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            evaluated.content.elements.as_slice(),
            [ElementNode { element: Element::TaskItem { body: parent_body, .. }, .. },
             ElementNode { element: Element::TaskItem { checked: true, .. }, .. }]
                if matches!(parent_body.elements[1].element, Element::TaskItem { checked: true, .. })
        ));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn lowers_indented_nested_definition_lists() {
        let evaluated =
            Evaluator::default().evaluate("/ API: Interface\n  / HTTP: Transport\n/ URL: Address");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            evaluated.content.elements.as_slice(),
            [ElementNode { element: Element::TermItem { description: parent_description, .. }, .. },
             ElementNode { element: Element::TermItem { .. }, .. }]
                if matches!(parent_description.elements[1].element, Element::TermItem { .. })
        ));
    }

    #[test]
    fn lowers_inline_surface_sugar() {
        let evaluated =
            Evaluator::default().evaluate("*bold* _slanted_ https://example.test/page.\\\nnext");
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
        assert!(matches!(
            evaluated.content.elements[3].element,
            Element::Text(_)
        ));
        assert!(matches!(
            evaluated.content.elements[4].element,
            Element::Link { .. }
        ));
        assert!(
            evaluated
                .content
                .elements
                .iter()
                .any(|node| matches!(node.element, Element::Linebreak))
        );
        assert!(
            evaluated
                .content
                .elements
                .iter()
                .any(|node| matches!(&node.element, Element::Text(text) if text == "next"))
        );
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
            !evaluated
                .content
                .elements
                .iter()
                .any(|node| matches!(node.element, Element::Image { .. } | Element::Figure { .. }))
        );
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn lowers_video_surface_sugar() {
        let evaluated = Evaluator::default().evaluate("!video(media/demo.mp4)");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Video { source, poster: None, controls: true }
                if source == "media/demo.mp4"
        ));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn lowers_audio_surface_sugar() {
        let evaluated = Evaluator::default().evaluate("!audio(media/theme.ogg)");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Audio { source, controls: true, looping: false } if source == "media/theme.ogg"
        ));
    }

    #[test]
    fn markdown_named_link_syntax_remains_text() {
        let evaluated = Evaluator::default().evaluate("[Notist](docs/index.html)");
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
                .any(|node| matches!(node.element, Element::Link { .. }))
        );
    }

    #[test]
    fn lowers_bare_email_addresses_as_mailto_links() {
        let evaluated = Evaluator::default().evaluate("Write hello+docs@example.test.");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[1].element,
            Element::Link { destination, body, .. }
                if destination == "mailto:hello+docs@example.test"
                    && matches!(&body.elements[0].element, Element::Text(value) if value == "hello+docs@example.test")
        ));

        let malformed = Evaluator::default().evaluate("not-an-address@invalid");
        assert!(matches!(
            &malformed.content.elements[0].element,
            Element::Text(value) if value == "not-an-address@invalid"
        ));
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
    #[ignore = "legacy feature moved to plugin"]
    fn lowers_insert_surface_sugar() {
        let evaluated = Evaluator::default().evaluate("Before ++new *value*++ after");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[1].element,
            Element::Insert(body)
                if body.elements.iter().any(|node| matches!(node.element, Element::Strong(_)))
        ));

        let unclosed = Evaluator::default().evaluate("keep ++literal");
        assert!(matches!(
            &unclosed.content.elements[0].element,
            Element::Text(text) if text == "keep ++literal"
        ));
        let empty = Evaluator::default().evaluate("keep ++++ literal");
        assert!(matches!(
            &empty.content.elements[0].element,
            Element::Text(text) if text == "keep ++++ literal"
        ));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn lowers_spoiler_surface_sugar() {
        let evaluated = Evaluator::default().evaluate("Reveal >!hidden *ending*!< later");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[1].element,
            Element::Spoiler(body)
                if body.elements.iter().any(|node| matches!(node.element, Element::Strong(_)))
        ));

        let unclosed = Evaluator::default().evaluate("keep >!hidden");
        assert!(matches!(
            &unclosed.content.elements[0].element,
            Element::Text(text) if text == "keep >!hidden"
        ));
        let empty = Evaluator::default().evaluate("keep >!!< literal");
        assert!(matches!(
            &empty.content.elements[0].element,
            Element::Text(text) if text == "keep >!!< literal"
        ));
    }

    #[test]
    fn lowers_heading_and_quote_surface_sugar() {
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
        let quoted = evaluator.evaluate("> Quoted");
        assert!(quoted.diagnostics.is_empty(), "{:?}", quoted.diagnostics);
        assert!(
            !quoted
                .content
                .elements
                .iter()
                .any(|node| matches!(node.element, Element::Quote { .. }))
        );

        let setext = evaluator.evaluate("Main *title*\n==========\n\nSubtitle\n--------");
        assert!(setext.diagnostics.is_empty(), "{:?}", setext.diagnostics);
        assert!(
            !setext
                .content
                .elements
                .iter()
                .any(|node| matches!(node.element, Element::Heading { .. }))
        );
    }

    #[test]
    fn evaluates_quote_attribution_function() {
        let evaluated = Evaluator::default()
            .evaluate("#quote(attribution=[Francis Bacon])[Knowledge is power]");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Quote { attribution: Some(attribution), body }
                if matches!(&attribution.elements[0].element, Element::Text(text) if text == "Francis Bacon")
                    && matches!(&body.elements[0].element, Element::Text(text) if text == "Knowledge is power")
        ));
    }

    #[test]
    fn does_not_lower_quote_marker_sugar() {
        let evaluated = Evaluator::default().evaluate("> > Nested *quotation*");
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
                .any(|node| matches!(node.element, Element::Quote { .. }))
        );
    }

    #[test]
    fn markdown_thematic_breaks_remain_text() {
        let evaluated = Evaluator::default().evaluate("---\n***\n___");
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
                .any(|node| matches!(node.element, Element::Rule))
        );
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn lowers_and_groups_definition_list_sugar() {
        let evaluated = Evaluator::default()
            .evaluate("/ *API*: Application interface\n/ URL: https://example.test");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::TermItem { term, description }
                if matches!(term.elements[0].element, Element::Strong(_))
                && matches!(description.elements[0].element, Element::Text(_))
        ));
        assert!(matches!(
            &evaluated.content.elements[1].element,
            Element::TermItem { description, .. }
                if matches!(description.elements[0].element, Element::Link { .. })
        ));
        let structured = structure(evaluated);
        assert!(matches!(
            &structured.document.blocks[0],
            Block::Element(ElementNode {
                element: Element::Terms { items },
                ..
            }) if items.len() == 2
        ));
        let explicit = structure(Evaluator::default().evaluate(
            "#terms[#terms::item(term=[API])[Interface]#terms::item(term=[URL])[Address]]",
        ));
        assert!(matches!(
            explicit.document.blocks.as_slice(),
            [Block::Element(ElementNode {
                element: Element::Terms { items },
                ..
            })] if items.len() == 2
        ));
    }

    #[test]
    fn lowers_and_groups_task_list_sugar() {
        let evaluated = Evaluator::default()
            .evaluate("- [ ] *Write* tests\n- [x] Run workspace checks\n- [X] Ship");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::TaskItem { checked: false, body }
                if matches!(body.elements[0].element, Element::Strong(_))
        ));
        assert!(matches!(
            evaluated.content.elements[1].element,
            Element::TaskItem { checked: true, .. }
        ));
        assert!(matches!(
            evaluated.content.elements[2].element,
            Element::TaskItem { checked: true, .. }
        ));
        let structured = structure(evaluated);
        assert!(matches!(
            &structured.document.blocks[0],
            Block::Element(ElementNode {
                element: Element::Tasks { items },
                ..
            }) if items.len() == 3
        ));
        let explicit = structure(
            Evaluator::default()
                .evaluate("#task[#task::item[First]#task::item(checked=true)[Second]]"),
        );
        assert!(matches!(
            explicit.document.blocks.as_slice(),
            [Block::Element(ElementNode {
                element: Element::Tasks { items },
                ..
            })] if items.len() == 2
        ));
    }

    #[test]
    fn lowers_inline_content_inside_block_sugar() {
        let evaluator = Evaluator::default();

        let heading = evaluator.evaluate("= *Important _note_* ");
        assert!(matches!(
            &heading.content.elements[0].element,
            Element::Heading { body, .. }
                if matches!(&body.elements[0].element, Element::Strong(strong)
                    if strong.elements.iter().any(|node| matches!(node.element, Element::Emph(_))))
        ));

        let list = evaluator.evaluate("- *bold*\n- _slanted_");
        assert!(matches!(
            &list.content.elements[0].element,
            Element::ListItem(body) if matches!(body.elements[0].element, Element::Strong(_))
        ));
        assert!(matches!(
            &list.content.elements[1].element,
            Element::ListItem(body) if matches!(body.elements[0].element, Element::Emph(_))
        ));

        let table = evaluator.evaluate("| *Name* | https://example.test |");
        assert!(matches!(
            &table.content.elements[0].element,
            Element::Table { cells, .. }
                if matches!(&cells[0].element, Element::TableCell { body, .. }
                    if matches!(body.elements[0].element, Element::Strong(_)))
                && matches!(&cells[1].element, Element::TableCell { body, .. }
                    if matches!(body.elements[0].element, Element::Link { .. }))
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
        assert_eq!(structured.document.blocks.len(), 4);
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
        assert!(matches!(structured.document.blocks[2], Block::Element(_)));
        assert!(matches!(
            &structured.document.blocks[3],
            Block::Element(node) if matches!(node.element, Element::Paragraph(_))
        ));
    }

    #[test]
    fn structuring_unifies_list_sugar_and_container_functions() {
        let evaluator = Evaluator::default();
        for source in ["- One\n- Two", "#list[#list::item[One]#list::item[Two]]"] {
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
            "#enum[#enum::item(value=3)[Three]#enum::item(value=4)[Four]]",
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
            .evaluate("#enum::item[First]\n#enum::item[Second]\n#list::item[Other]");
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
        assert_eq!(structured.diagnostics.len(), 1);
        assert_eq!(
            structured.diagnostics[0].message,
            "unknown function `missing`"
        );
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
    fn rejects_non_content_values_in_markup_position() {
        let evaluation = Evaluator::default().evaluate("value: #42");

        assert!(
            evaluation
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message.contains("cannot insert Int into Markup") })
        );
    }

    #[test]
    fn ordinary_and_trailing_content_arguments_are_equivalent() {
        let evaluator = Evaluator::default();
        let ordinary = evaluator.evaluate("#quote(body=[same])");
        let trailing = evaluator.evaluate("#quote[same]");

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
            Element::Quote { body, .. } if body.elements.len() == 1 && matches!(
                &body.elements[0].element,
                Element::Text(text) if text == "same"
            )
        ));
        assert!(matches!(
            &trailing.content.elements[0].element,
            Element::Quote { body, .. } if body.elements.len() == 1 && matches!(
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
    #[ignore = "legacy feature moved to plugin"]
    fn lowers_additional_inline_sugar_with_delimiter_precedence() {
        let evaluation = Evaluator::default().evaluate("==*marked*==__under__^2^~i~");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert!(matches!(
            evaluation.content.elements.as_slice(),
            [
                ElementNode { element: Element::Highlight(marked), .. },
                ElementNode { element: Element::Underline(_), .. },
                ElementNode { element: Element::Super(_), .. },
                ElementNode { element: Element::Sub(_), .. },
            ] if matches!(marked.elements[0].element, Element::Strong(_))
        ));

        let literal = Evaluator::default().evaluate("==open __open ^open ~open");
        assert!(matches!(
            &literal.content.elements[0].element,
            Element::Text(text) if text == "==open __open ^open ~open"
        ));
    }

    #[test]
    fn evaluates_keyboard_input_as_a_semantic_inline_element() {
        let evaluation = Evaluator::default().evaluate("Press #kbd[Ctrl + *S*] to save.");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert!(matches!(
            &evaluation.content.elements[1].element,
            Element::Keyboard(body) if matches!(body.elements[1].element, Element::Strong(_))
        ));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn evaluates_sample_output_as_a_semantic_inline_element() {
        let evaluation = Evaluator::default().evaluate("Output: #samp[Saved *3* files]");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert!(matches!(
            &evaluation.content.elements[1].element,
            Element::Sample(body) if body.elements.iter().any(|node| matches!(node.element, Element::Strong(_)))
        ));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn evaluates_machine_readable_time_content() {
        let evaluation =
            Evaluator::default().evaluate("Published #time(\"2026-07-21\")[July *21*].");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert!(matches!(
            &evaluation.content.elements[1].element,
            Element::Time { datetime, body }
                if datetime == "2026-07-21"
                    && body.elements.iter().any(|node| matches!(node.element, Element::Strong(_)))
        ));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn lowers_inline_footnote_sugar() {
        let evaluated = Evaluator::default().evaluate("Claim^[Source with *detail*].");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[1].element,
            Element::Footnote(body)
                if body.elements.iter().any(|node| matches!(node.element, Element::Strong(_)))
        ));

        let unclosed = Evaluator::default().evaluate("Claim^[source");
        assert!(matches!(
            &unclosed.content.elements[0].element,
            Element::Text(text) if text == "Claim^[source"
        ));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn lowers_comment_sugar() {
        let evaluated = Evaluator::default().evaluate("Visible %%author *note*%% text");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[1].element,
            Element::Comment(body)
                if body.elements.iter().any(|node| matches!(node.element, Element::Strong(_)))
        ));
        let unclosed = Evaluator::default().evaluate("%%author note");
        assert!(
            matches!(&unclosed.content.elements[0].element, Element::Text(text) if text == "%%author note")
        );
    }

    #[test]
    fn lowers_math_sugar() {
        let evaluated = Evaluator::default().evaluate("Inline $x + y$ and $$a < b$$");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(evaluated.content.elements.iter().any(
            |node| matches!(&node.element, Element::Math { text, block: false } if text == "x + y")
        ));
        assert!(evaluated.content.elements.iter().any(
            |node| matches!(&node.element, Element::Math { text, block: true } if text == "a < b")
        ));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn lowers_abbreviation_sugar() {
        let evaluated = Evaluator::default().evaluate("*[HTML]: HyperText Markup Language");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Abbr { term, expansion }
                if term == "HTML" && expansion == "HyperText Markup Language"
        ));

        let invalid = Evaluator::default().evaluate("*[HTML]: ");
        assert!(matches!(
            &invalid.content.elements[0].element,
            Element::Text(_)
        ));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn lowers_citation_sugar() {
        let evaluated = Evaluator::default().evaluate("See [@doe2024, pp. 17-19] and [@roe2025].");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(evaluated.content.elements.iter().any(|node| {
            matches!(
                &node.element,
                Element::Citation { key, locator }
                    if key == "doe2024" && locator.as_deref() == Some("pp. 17-19")
            )
        }));
        assert!(evaluated.content.elements.iter().any(|node| {
            matches!(
                &node.element,
                Element::Citation { key, locator }
                    if key == "roe2025" && locator.is_none()
            )
        }));

        let invalid = Evaluator::default().evaluate("Keep [@two words] literal");
        assert!(matches!(
            &invalid.content.elements[0].element,
            Element::Text(text) if text == "Keep [@two words] literal"
        ));
    }

    #[test]
    fn omits_source_comments_from_evaluated_content() {
        let evaluated = Evaluator::default()
            .evaluate("Visible // line comment\ntext /* outer /* nested */ block */ after");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        let visible = evaluated
            .content
            .elements
            .iter()
            .filter_map(|node| match &node.element {
                Element::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(visible, "Visible \ntext  after");
    }

    #[test]
    fn keeps_rich_content_inside_surface_lists_and_tasks() {
        let evaluated = Evaluator::default().evaluate(
            "- open `config.toml`\n- see [[vault::grammar]]\n- press #kbd[Ctrl + S]\n\n- [ ] check [[target]]",
        );
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        let list_items = evaluated
            .content
            .elements
            .iter()
            .filter_map(|node| match &node.element {
                Element::ListItem(body) => Some(body),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(list_items.len(), 3);
        assert!(
            list_items[0]
                .elements
                .iter()
                .any(|node| matches!(node.element, Element::Raw { .. }))
        );
        assert!(
            list_items[1]
                .elements
                .iter()
                .any(|node| matches!(node.element, Element::Reference(_)))
        );
        assert!(
            list_items[2]
                .elements
                .iter()
                .any(|node| matches!(node.element, Element::Keyboard(_)))
        );
        assert!(evaluated.content.elements.iter().any(|node| {
            matches!(&node.element, Element::TaskItem { body, .. }
                if body.elements.iter().any(|child| matches!(child.element, Element::Reference(_))))
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
            "#let warning(title: String = \"Warning\", body: Content) -> Content = #quote[\
             #heading(level=3)[#title]\n#body]\n\
             #warning[hello]",
        );
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(evaluated.content.elements.iter().any(|node| {
            matches!(&node.element, Element::Quote { body, .. }
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
