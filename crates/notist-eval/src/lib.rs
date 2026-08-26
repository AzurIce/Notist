//! Evaluation and structural normalization for Notist documents.

mod function;
mod leaf;
mod lower;
mod stream_lower;
mod type_system;

#[cfg(test)]
extern crate self as notist_eval;

#[cfg(test)]
#[path = "../../../plugins/core/lib.rs"]
mod test_core;

use std::collections::HashMap;

use notist_model::{Node, TextRange};
use notist_syntax::Parse;

pub use function::{
    ElementFunction, Function, FunctionContext, FunctionInput, FunctionOwner, FunctionRegistry,
    PluginContribution, RegistryError, RegistryErrorReason,
};
pub use notist_model::ElementSchema;

pub use leaf::node_engine::{
    NodeEvaluation, collect_names, evaluate_to_nodes, fully_reduced, nodes_to_element_tree,
    reduce_nodes, reduce_nodes_recovering,
};
pub use leaf::{
    ElementTree, ReduceFrame, ReduceLimits, ShapingRegistry, shape_flat, shape_flat_with,
};
pub use type_system::{
    BoundArguments, DefaultValue, FunctionImplementation, FunctionSignature, FunctionValue,
    Parameter, Type, Value, ValueOrigin,
};

/// The result of the full evaluation pipeline: lower → reduce → shape.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Evaluation {
    /// Lowering output before reduction: the call forest as written.
    pub lowered: Vec<Node>,
    /// The reduced call forest (fixpoint reached; unknown names survive).
    pub forest: Vec<Node>,
    /// The recursively shaped canonical tree.
    pub tree: ElementTree,
    /// Parse, lowering, and reduction diagnostics.
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

/// Evaluates Notist source using a configurable function registry.
pub struct Evaluator {
    registry: FunctionRegistry,
}

#[cfg(not(test))]
fn default_shaping() -> ShapingRegistry {
    ShapingRegistry::new()
}

#[cfg(test)]
fn default_shaping() -> ShapingRegistry {
    test_core::registry().1
}

impl Evaluator {
    /// Creates an evaluator using the provided function registry.
    pub fn new(registry: FunctionRegistry) -> Self {
        Self { registry }
    }

    /// Parses and evaluates a complete source file.
    pub fn evaluate(&self, source: &str) -> Evaluation {
        self.evaluate_with_shaping(source, &default_shaping())
    }

    /// Like [`Self::evaluate`] with a caller-provided shaping registry.
    ///
    /// Plugin packages contribute their element schemas through the snapshot
    /// shaping registry; this is the entry point that applies them while
    /// folding the reduced forest into the canonical tree.
    pub fn evaluate_with_shaping(&self, source: &str, shaping: &ShapingRegistry) -> Evaluation {
        let parse = notist_syntax::parse(source);
        self.evaluate_parsed_with_shaping(source, &parse, HashMap::new(), shaping)
    }

    /// Evaluates a parsed source file with a pre-seeded document scope: the
    /// analysis layer injects imported bindings before evaluation (D0004).
    pub fn evaluate_parsed_with_bindings(
        &self,
        source: &str,
        parse: &Parse,
        bindings: HashMap<String, Value>,
    ) -> Evaluation {
        self.evaluate_parsed_with_shaping(source, parse, bindings, &default_shaping())
    }

    /// Unified-node variant of the pre-parsed bindings entry point with a
    /// caller-provided shaping registry.
    #[tracing::instrument(
        target = "notist_eval",
        name = "evaluate_pass",
        skip_all,
        fields(roots = parse.root.items.len())
    )]
    pub fn evaluate_parsed_with_shaping(
        &self,
        source: &str,
        parse: &Parse,
        bindings: HashMap<String, Value>,
        shaping: &ShapingRegistry,
    ) -> Evaluation {
        let lowered = stream_lower::lower_document_with_bindings(
            source,
            &parse.root,
            0,
            &self.registry,
            bindings,
        );
        let mut evaluation =
            leaf::node_engine::evaluate_to_nodes(lowered.nodes.clone(), &self.registry, shaping);
        let mut diagnostics = Vec::new();
        diagnostics.extend(parse.errors.iter().cloned().map(|error| EvalDiagnostic {
            message: error.message,
            range: error.range,
        }));
        diagnostics.extend(lowered.diagnostics);
        diagnostics.append(&mut evaluation.diagnostics);
        tracing::debug!(
            target: "notist_eval",
            diagnostics = diagnostics.len(),
            forest = evaluation.forest.len(),
            "evaluate pass complete"
        );
        Evaluation {
            lowered: lowered.nodes,
            forest: evaluation.forest,
            tree: evaluation.tree,
            diagnostics,
            bindings: lowered.bindings,
            annotations: lowered.annotations,
            module_attributes: lowered.module_attributes,
        }
    }

    /// Returns the function registry used by this evaluator.
    pub fn registry(&self) -> &FunctionRegistry {
        &self.registry
    }
}

#[cfg(not(test))]
impl Default for Evaluator {
    fn default() -> Self {
        Self::new(FunctionRegistry::new())
    }
}

#[cfg(test)]
impl Default for Evaluator {
    fn default() -> Self {
        Self::new(test_core::registry().0)
    }
}

/// Parses and evaluates a source fragment as nested Notist content, honoring
/// the host-provided base offset for source ranges.
pub(crate) fn evaluate_fragment(
    source: &str,
    base_offset: usize,
    registry: &FunctionRegistry,
) -> Evaluation {
    let parse = notist_syntax::parse(source);
    let lowered = stream_lower::lower_document_with_bindings(
        source,
        &parse.root,
        base_offset,
        registry,
        HashMap::new(),
    );
    let mut evaluation =
        leaf::node_engine::evaluate_to_nodes(lowered.nodes.clone(), registry, &default_shaping());
    let mut diagnostics = Vec::new();
    diagnostics.extend(parse.errors.iter().cloned().map(|error| EvalDiagnostic {
        message: error.message,
        range: error.range,
    }));
    diagnostics.extend(lowered.diagnostics);
    diagnostics.append(&mut evaluation.diagnostics);
    Evaluation {
        lowered: lowered.nodes,
        forest: evaluation.forest,
        tree: evaluation.tree,
        diagnostics,
        bindings: lowered.bindings,
        annotations: lowered.annotations,
        module_attributes: lowered.module_attributes,
    }
}

#[cfg(test)]
mod tests {
    use notist_model::{Node, NodeValue, TextRange};

    use super::*;

    /// Zeroes source ranges recursively so two forests can be compared by
    /// shape alone.
    fn normalized(node: &Node) -> Node {
        let mut node = node.clone();
        node.range = TextRange::new(0, 0);
        for (_, value) in &mut node.args {
            if let NodeValue::Stream(nodes) = value {
                *nodes = nodes.iter().map(normalized).collect();
            }
        }
        node.children = node.children.iter().map(normalized).collect();
        node
    }

    fn normalized_forest(forest: &[Node]) -> Vec<Node> {
        forest.iter().map(normalized).collect()
    }

    /// The text payload of a `core::text` node.
    fn text(node: &Node) -> Option<&str> {
        if node.is_core("text")
            && let Some(NodeValue::String(value)) = node.get("text")
        {
            return Some(value);
        }
        None
    }

    fn texts(forest: &[Node]) -> Vec<&str> {
        forest.iter().filter_map(text).collect()
    }

    fn joined_texts(forest: &[Node]) -> String {
        texts(forest).into_iter().collect()
    }

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
            .forest
            .iter()
            .find(|node| node.is_core("heading"))
        else {
            panic!("expected a heading, got {:?}", evaluation.forest)
        };
        assert_eq!(texts(&heading.children), ["violet"]);
        assert_eq!(
            evaluation.bindings.get("accent"),
            Some(&Value::String("violet".into()))
        );
    }

    #[test]
    fn if_expression_selects_branches_and_omitting_else_yields_none() {
        let yes = Evaluator::default().evaluate("#if true [yes] else [no]");
        assert!(yes.diagnostics.is_empty(), "{:?}", yes.diagnostics);
        assert_eq!(texts(&yes.forest), ["yes"]);
        let no = Evaluator::default().evaluate("#if false [yes] else [no]");
        assert!(no.diagnostics.is_empty(), "{:?}", no.diagnostics);
        assert_eq!(texts(&no.forest), ["no"]);
        let missing = Evaluator::default().evaluate("#if false [yes]");
        assert!(missing.diagnostics.is_empty(), "{:?}", missing.diagnostics);
        assert!(missing.forest.is_empty());
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
        assert!(
            evaluation
                .forest
                .iter()
                .any(|node| { node.is_core("heading") && texts(&node.children) == ["标题"] })
        );
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
                .forest
                .iter()
                .any(|node| text(node) == Some("42"))
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
        assert_eq!(content.len(), 2);
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
        ) -> Result<Value, Vec<EvalDiagnostic>> {
            let body = input.arguments.take_content("body");
            let mut node = notist_model::Node::block_call("quote", input.range);
            node.children = body;
            Ok(Value::Content(vec![node]))
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
        ) -> Result<Value, Vec<EvalDiagnostic>> {
            Ok(Value::Int(2))
        }
    }

    #[test]
    fn lowers_transparent_scopes_references_and_parbreaks() {
        let source = "Hello #<self::target>@concept,#important\n\nAfter";
        let evaluation = Evaluator::default().evaluate(source);

        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert_eq!(evaluation.forest.len(), 4);
        assert_eq!(text(&evaluation.forest[0]), Some("Hello "));
        assert!(evaluation.forest[1].is_core("reference"));
        assert!(evaluation.forest[2].is_core("parbreak"));
    }

    #[test]
    fn preserves_unknown_calls_with_optional_trailing_content() {
        // Unregistered names stay in the forest as data: reduction is total
        // and never rewrites a call nobody handles.
        let content = Evaluator::default().evaluate("#missing(x=1)[#<self::target>]");
        let bodyless = Evaluator::default().evaluate("#missing(x=1)");

        assert!(content.diagnostics.is_empty(), "{:?}", content.diagnostics);
        assert_eq!(content.forest.len(), 1);
        let call = &content.forest[0];
        assert_eq!(call.name, "missing");
        assert_eq!(call.get("x"), Some(&NodeValue::Int(1)));
        assert!(
            call.children.iter().any(|node| node.is_core("reference")),
            "{call:#?}"
        );
        let call = &bodyless.forest[0];
        assert_eq!(call.name, "missing");
        assert!(call.children.is_empty());
    }

    #[test]
    fn content_calls_receive_lowered_notist_content() {
        let mut registry = FunctionRegistry::new();
        registry.register(QuoteFunction).unwrap();
        let evaluator = Evaluator::new(registry);
        let evaluation =
            evaluator.evaluate("Before\n\n#test::quote[Inside [[self::target]].]\n\nAfter");

        assert!(evaluation.diagnostics.is_empty());
        assert_eq!(evaluation.tree.roots.len(), 3);
        assert!(evaluation.tree.roots[0].is_core("paragraph"));
        assert_eq!(evaluation.tree.roots[1].name, "quote");
        assert!(evaluation.tree.roots[2].is_core("paragraph"));
    }

    #[test]
    fn plain_markup_produces_text_elements() {
        let evaluator = Evaluator::default();
        let plain = evaluator.evaluate("plain text");
        assert!(plain.diagnostics.is_empty());
        assert_eq!(text(&plain.forest[0]), Some("plain text"));
    }

    #[test]
    fn structuring_groups_plain_paragraphs() {
        let evaluator = Evaluator::default();
        let tree = evaluator.evaluate("plain *content*").tree;
        assert!(
            matches!(tree.roots.as_slice(), [node] if node.is_core("paragraph")
                && node.children.iter().any(|child| child.is_core("strong")))
        );
    }

    #[test]
    fn lowers_backtick_and_fenced_raw_sugar() {
        let evaluator = Evaluator::default();

        let inline = evaluator.evaluate("Before `cargo test` after");
        assert!(inline.diagnostics.is_empty(), "{:?}", inline.diagnostics);
        let raw = &inline.forest[1];
        assert!(raw.is_core("raw"));
        assert_eq!(
            raw.get("source"),
            Some(&NodeValue::String("cargo test".into()))
        );
        assert_eq!(raw.get("block"), Some(&NodeValue::Bool(false)));
        assert_eq!(raw.get("lang"), Some(&NodeValue::None));

        let fenced = evaluator.evaluate("```rust\nfn main() {}\n```");
        assert!(fenced.diagnostics.is_empty(), "{:?}", fenced.diagnostics);
        let raw = &fenced.forest[0];
        assert!(raw.is_core("raw"));
        assert_eq!(
            raw.get("source"),
            Some(&NodeValue::String("fn main() {}".into()))
        );
        assert_eq!(raw.get("lang"), Some(&NodeValue::String("rust".into())));
        assert_eq!(raw.get("block"), Some(&NodeValue::Bool(true)));

        let explicit = evaluator.evaluate("#raw(r#\"cargo test\"#)");
        assert!(
            explicit.diagnostics.is_empty(),
            "{:?}",
            explicit.diagnostics
        );
        assert_eq!(
            normalized(&inline.forest[1]),
            normalized(&explicit.forest[0])
        );

        let without_builtins = Evaluator::new(FunctionRegistry::new()).evaluate("`core raw`");
        assert!(
            without_builtins.diagnostics.is_empty(),
            "{:?}",
            without_builtins.diagnostics
        );
        let raw = &without_builtins.forest[0];
        assert!(raw.is_core("raw"));
        assert_eq!(
            raw.get("source"),
            Some(&NodeValue::String("core raw".into()))
        );
    }

    #[test]
    fn lowers_headings_inside_long_form_markup() {
        let evaluated = Evaluator::default().evaluate("= Title\n\nIntro\n\nOutro");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        let heading = &evaluated.forest[0];
        assert!(heading.is_core("heading"));
        assert_eq!(heading.get("level"), Some(&NodeValue::Int(1)));
        assert!(
            evaluated
                .forest
                .iter()
                .any(|node| text(node).is_some_and(|value| value.contains("Outro")))
        );
    }

    #[test]
    fn lowers_fenced_raw_block_inside_list_item_body() {
        // An indented fenced raw block belongs to the row body: it lowers
        // into the item content instead of escaping as a sibling.
        let evaluated = Evaluator::default().evaluate("- item\n  ```not\n  x\n  ```\n- next");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(
            matches!(evaluated.forest.as_slice(), [first, second]
                if first.is_core("item") && second.is_core("item")
                    && first.children.iter().any(|node| node.is_core("raw")
                        && node.get("block") == Some(&NodeValue::Bool(true)))),
            "{:#?}",
            evaluated.forest
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
        assert!(
            matches!(evaluated.forest.as_slice(), [parent, _]
                if parent.is_core("item")
                    && parent.children[1].is_core("item")
                    && parent.children[1].get("ordered") == Some(&NodeValue::Bool(true))
                    && parent.children[2].is_core("item")
                    && parent.children[2].get("ordered") == Some(&NodeValue::Bool(true))),
            "{:#?}",
            evaluated.forest
        );
        let tree = evaluated.tree;
        assert!(
            matches!(tree.roots.as_slice(), [list]
                if list.is_core("list")
                    && list.get("ordered") == Some(&NodeValue::Bool(false))
                    && list.children.len() == 2),
            "{:#?}",
            tree.roots
        );
    }

    #[test]
    fn lowers_orphan_indented_list_before_shallower_list() {
        let evaluated = Evaluator::default().evaluate("= t\n\n  - x\n+ y");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(evaluated.forest.iter().any(
            |node| node.is_core("item") && node.get("ordered") == Some(&NodeValue::Bool(false))
        ));
        assert!(evaluated.forest.iter().any(
            |node| node.is_core("item") && node.get("ordered") == Some(&NodeValue::Bool(true))
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
        assert!(!evaluated.forest.iter().any(|node| node.is_core("item")));

        let inline = Evaluator::default().evaluate("*strong*");
        assert!(
            matches!(inline.forest.as_slice(), [node] if node.is_core("strong")),
            "{:#?}",
            inline.forest
        );
    }

    #[test]
    fn lowers_escaped_inline_punctuation_as_literal_text() {
        let evaluated = Evaluator::default().evaluate("\\*not strong\\* and \\|pipe\\|");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(!evaluated.forest.iter().any(|node| node.is_core("strong")));
        assert_eq!(joined_texts(&evaluated.forest), "*not strong* and |pipe|");
    }

    #[test]
    fn numbered_markdown_lists_remain_text() {
        let evaluated = Evaluator::default().evaluate("3) third\n  7) nested\n9) ninth");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(!evaluated.forest.iter().any(
            |node| node.is_core("item") && node.get("ordered") == Some(&NodeValue::Bool(true))
        ));
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
        assert!(evaluated.forest[0].is_core("strong"));
        assert!(evaluated.forest[1].is_core("text"));
        assert!(evaluated.forest[2].is_core("emph"));
        assert!(evaluated.forest.iter().any(|node| {
            text(node).is_some_and(|value| value.contains("https://example.test/page."))
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
            evaluated
                .forest
                .iter()
                .any(|node| text(node).is_some_and(|value| value.contains("flow.png")))
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
        assert!(
            evaluated
                .forest
                .iter()
                .any(|node| text(node).is_some_and(|value| value.contains("docs/index.html")))
        );
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
                .forest
                .iter()
                .any(|node| text(node)
                    .is_some_and(|value| value.contains("hello+docs@example.test")))
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
        let callout = &evaluated.forest[0];
        assert!(callout.is_core("callout"));
        assert_eq!(
            callout.get("kind"),
            Some(&NodeValue::String("warning".into()))
        );
        assert!(
            callout.children[0].is_core("strong"),
            "{:#?}",
            callout.children
        );
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
        let details = &evaluated.forest[0];
        assert!(details.is_core("details"));
        assert_eq!(details.get("open"), Some(&NodeValue::Bool(false)));
        let Some(NodeValue::Stream(summary)) = details.get("summary") else {
            panic!("expected a summary stream, got {:#?}", details.args)
        };
        assert!(summary[0].is_core("strong"));
        assert!(details.children.iter().any(|node| node.is_core("emph")));
    }

    #[test]
    fn lowers_strike_surface_sugar() {
        let evaluated = Evaluator::default().evaluate("Before ~~old *value*~~ after");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        let strike = &evaluated.forest[1];
        assert!(strike.is_core("strike"));
        assert!(strike.children.iter().any(|node| node.is_core("strong")));

        let unclosed = Evaluator::default().evaluate("keep ~~literal");
        assert_eq!(text(&unclosed.forest[0]), Some("keep ~~literal"));
        let empty = Evaluator::default().evaluate("keep ~~~~ literal");
        assert_eq!(text(&empty.forest[0]), Some("keep ~~~~ literal"));
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
        assert!(evaluated.forest[0].is_core("heading"));
        assert_eq!(evaluated.forest[0].get("level"), Some(&NodeValue::Int(1)));
        assert!(evaluated.forest[1].is_core("heading"));
        assert_eq!(evaluated.forest[1].get("level"), Some(&NodeValue::Int(2)));
        // D0003 boundary: a line of only `=` is an empty-body heading and a
        // line of only `-` (three or more) is a rule — Markdown setext
        // underlines do not survive as text.
        let setext = evaluator.evaluate("Main *title*\n==========\n\nSubtitle\n--------");
        assert!(setext.diagnostics.is_empty(), "{:?}", setext.diagnostics);
        assert!(
            setext
                .forest
                .iter()
                .any(|node| node.is_core("heading")
                    && node.get("level") == Some(&NodeValue::Int(10)))
        );
        assert!(setext.forest.iter().any(|node| node.is_core("rule")));
    }

    #[test]
    fn does_not_lower_quote_marker_sugar() {
        // Quote is not part of the current language (R04): the marker stays text.
        let evaluated = Evaluator::default().evaluate("> > Nested *quotation*");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(
            evaluated
                .forest
                .iter()
                .any(|node| text(node).is_some_and(|value| value.contains("Nested")))
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
        assert!(evaluated.forest.iter().any(|node| node.is_core("rule")));
        assert!(
            evaluated
                .forest
                .iter()
                .any(|node| text(node).is_some_and(|value| value.contains("***")))
        );
        assert!(
            evaluated
                .forest
                .iter()
                .any(|node| text(node).is_some_and(|value| value.contains("___")))
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
        let table = &evaluated.forest[0];
        assert!(table.is_core("table"), "{:#?}", evaluated.forest);
        assert_eq!(table.get("columns"), Some(&NodeValue::Int(2)));
        assert_eq!(table.get("header"), Some(&NodeValue::Bool(true)));
        assert_eq!(
            table.get("align"),
            Some(&NodeValue::String("left,right".into()))
        );
        assert_eq!(table.children.len(), 6);
        let cell = &table.children[2];
        assert!(cell.is_core("table-cell"));
        assert_eq!(texts(&cell.children), ["one"]);

        assert!(
            matches!(evaluated.tree.roots.as_slice(), [node] if node.is_core("table")),
            "{:#?}",
            evaluated.tree.roots
        );
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
        let table = &evaluated.forest[0];
        assert!(table.is_core("table"));
        assert_eq!(table.get("columns"), Some(&NodeValue::Int(2)));
        assert_eq!(table.get("header"), Some(&NodeValue::Bool(true)));
        assert_eq!(table.children.len(), 4);

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
        let figure = &evaluated.forest[0];
        assert!(figure.is_core("figure"), "{:#?}", evaluated.forest);
        assert_eq!(figure.get("kind"), Some(&NodeValue::String("table".into())));
        assert!(figure.children.iter().any(|node| node.is_core("table")));
        let Some(NodeValue::Stream(supplement)) = figure.get("supplement") else {
            panic!("expected a supplement stream")
        };
        assert_eq!(texts(supplement), ["Tab"]);
        let Some(NodeValue::Stream(caption)) = figure.get("caption") else {
            panic!("expected a caption stream")
        };
        assert_eq!(texts(caption), ["Cap"]);

        // Typst `kind: auto`: the wrapped block element decides the kind.
        let inferred = evaluator.evaluate("#figure[\n#table(columns: 1)[#table-cell[X]]\n]");
        assert!(
            inferred.diagnostics.is_empty(),
            "{:?}",
            inferred.diagnostics
        );
        assert_eq!(
            inferred.forest[0].get("kind"),
            Some(&NodeValue::String("table".into()))
        );

        assert!(
            matches!(evaluated.tree.roots.as_slice(), [node] if node.is_core("figure")),
            "{:#?}",
            evaluated.tree.roots
        );
    }

    #[test]
    fn shaping_groups_paragraphs_and_adjacent_list_items() {
        let range = TextRange::new(0, 1);
        let item = || {
            let mut node = Node::block_call("core::item", range).arg("ordered", false);
            node.children = vec![Node::call("core::text", range).arg("text", "item")];
            node
        };
        let mut heading = Node::block_call("core::heading", range).arg("level", 1_i64);
        heading.children = vec![Node::call("core::text", range).arg("text", "title")];
        let forest = vec![
            Node::call("core::text", range).arg("text", "intro"),
            Node::call("core::parbreak", range),
            item(),
            item(),
            heading,
            Node::call("core::text", range).arg("text", "tail"),
        ];

        let (_, shaping) = test_core::registry();
        let tree = shape_flat_with(&forest, &shaping);
        // D0002 section grouping: the heading and its following content form
        // a Section node.
        assert_eq!(tree.roots.len(), 3);
        assert!(tree.roots[0].is_core("paragraph"));
        assert!(
            tree.roots[1].is_core("list")
                && tree.roots[1].get("ordered") == Some(&NodeValue::Bool(false))
                && tree.roots[1].children.len() == 2
        );
        let section = &tree.roots[2];
        assert!(section.is_core("section"), "{:#?}", tree.roots);
        assert_eq!(section.get("level"), Some(&NodeValue::Int(1)));
        assert!(section.children[0].is_core("heading"));
        assert!(
            section.children[1..]
                .iter()
                .any(|node| node.is_core("paragraph"))
        );
    }

    #[test]
    fn structuring_unifies_list_sugar_and_item_calls() {
        let evaluator = Evaluator::default();
        for source in ["- One\n- Two", "#item[One]#item[Two]"] {
            let tree = evaluator.evaluate(source).tree;
            assert!(
                matches!(tree.roots.as_slice(), [list]
                    if list.is_core("list")
                        && list.get("ordered") == Some(&NodeValue::Bool(false))
                        && list.children.len() == 2),
                "{:#?}",
                tree.roots
            );
        }
        for source in [
            "+ Three\n+ Four",
            "#item(ordered=true)[Three]#item(ordered=true)[Four]",
        ] {
            let tree = evaluator.evaluate(source).tree;
            assert!(
                matches!(tree.roots.as_slice(), [list]
                    if list.is_core("list")
                        && list.get("ordered") == Some(&NodeValue::Bool(true))
                        && list.children.len() == 2),
                "{:#?}",
                tree.roots
            );
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
        let tree = evaluation.tree;
        assert!(
            matches!(&tree.roots[0], list
                if list.is_core("list")
                    && list.get("ordered") == Some(&NodeValue::Bool(true))
                    && list.children.len() == 2),
            "{:#?}",
            tree.roots
        );
        assert!(
            matches!(&tree.roots[1], list
                if list.is_core("list")
                    && list.get("ordered") == Some(&NodeValue::Bool(false))
                    && list.children.len() == 1),
            "{:#?}",
            tree.roots
        );
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
                FunctionOwner::Package("demo".into()),
            ))
            .unwrap();
        let evaluation = Evaluator::new(registry).evaluate("#demo::box(source: \"wgsl\")[Hi]");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let node = &evaluation.forest[0];
        assert_eq!(node.name, "demo::box");
        assert!(node.block);
        assert_eq!(texts(&node.children), ["Hi"]);
        assert_eq!(node.get("source"), Some(&NodeValue::String("wgsl".into())));
        assert_eq!(node.get("width"), Some(&NodeValue::Int(800)));
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
                FunctionOwner::Package("demo".into()),
            ))
            .unwrap();
        let evaluation = Evaluator::new(registry).evaluate("#demo::badge(label: \"x\")");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let node = &evaluation.forest[0];
        assert_eq!(node.name, "demo::badge");
        assert!(node.children.is_empty());
        assert_eq!(node.args.len(), 1);
    }

    #[test]
    fn reduction_preserves_siblings_around_failed_calls() {
        let evaluation =
            Evaluator::default().evaluate("Before\n\n#heading(level: 0)[bad]\n\nAfter");
        assert!(
            evaluation
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("heading level")),
            "{:?}",
            evaluation.diagnostics
        );
        assert!(
            evaluation.forest.iter().any(|node| node.is_core("text")),
            "{:#?}",
            evaluation.forest
        );
        assert_eq!(evaluation.tree.roots.len(), 2);
    }

    #[test]
    fn parsed_evaluation_accepts_import_seed_bindings() {
        let source = "#heading[#title]";
        let parse = notist_syntax::parse(source);
        let bindings = HashMap::from([("title".to_owned(), Value::String("Imported".to_owned()))]);
        let evaluation =
            Evaluator::default().evaluate_parsed_with_bindings(source, &parse, bindings);
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert_eq!(evaluation.tree.roots.len(), 1);
        let section = &evaluation.tree.roots[0];
        assert!(section.is_core("section"));
        let heading = section
            .children
            .iter()
            .find(|node| node.is_core("heading"))
            .expect("section contains its heading");
        assert_eq!(texts(&heading.children), ["Imported"]);
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
        assert_eq!(evaluated.forest[0].get("level"), Some(&NodeValue::Int(2)));
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
        ) -> Result<Value, Vec<EvalDiagnostic>> {
            Ok(Value::Content(Vec::new()))
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
    fn evaluation_preserves_reduction_diagnostics() {
        // Diagnostics never masquerade as success: a failed call surfaces its
        // diagnostic while the rest of the forest evaluates.
        let evaluation = Evaluator::default().evaluate("#heading(level: 0)[bad]");
        assert!(
            evaluation
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("heading level"))
        );
        // Unknown names stay silent at the reduction layer (check owns them).
        let unknown = Evaluator::default().evaluate("#missing[body]");
        assert!(unknown.diagnostics.is_empty());
    }

    #[test]
    fn evaluates_markup_with_string_and_content_interpolation() {
        let evaluation = Evaluator::default().evaluate("a[plain]#\"text\"#[content]z");

        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert_eq!(
            texts(&evaluation.forest),
            ["a[plain]", "text", "content", "z"]
        );
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
                .forest
                .iter()
                .any(|node| text(node) == Some("42"))
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
        for evaluation in [&ordinary, &trailing] {
            let details = &evaluation.forest[0];
            assert!(details.is_core("details"));
            assert_eq!(texts(&details.children), ["same"]);
        }
        assert_eq!(
            normalized_forest(&ordinary.forest),
            normalized_forest(&trailing.forest)
        );
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
        assert_eq!(plain.forest, annotated.forest);
    }

    #[test]
    fn keeps_markup_comment_syntax_as_text_and_drops_code_trivia() {
        // E09: `//` and `/* ... */` are ordinary text in the Markup stream;
        // only Code contexts strip them as lexical trivia.
        let markup =
            Evaluator::default().evaluate("Visible // line comment\ntext /* outer block */ after");
        assert!(markup.diagnostics.is_empty(), "{:?}", markup.diagnostics);
        assert_eq!(
            joined_texts(&markup.forest),
            "Visible // line commenttext /* outer block */ after"
        );

        let code = Evaluator::default().evaluate("#(1 + /* nested /* block */ comment */ 2)");
        assert!(code.diagnostics.is_empty(), "{:?}", code.diagnostics);
        assert_eq!(text(&code.forest[0]), Some("3"));
    }

    #[test]
    fn soft_break_joins_lines_without_space() {
        // A single soft break inside a paragraph is a separator, not content:
        // the lowered Text nodes must not retain the `\n`.
        let evaluation = Evaluator::default().evaluate("第一段。\n第二段。");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert_eq!(joined_texts(&evaluation.forest), "第一段。第二段。");
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
        assert!(
            evaluation
                .forest
                .iter()
                .any(|node| { node.is_core("heading") && node.range == TextRange::new(7, 14) })
        );
        // "@[install]\n" ends at 26; the soft break is a separator, so the
        // paragraph's Text node is "abc" and spans [27, 30).
        assert_eq!(evaluation.annotations[1].range, TextRange::new(27, 30));
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
        assert_eq!(joined_texts(&evaluation.forest), "before 2 after");
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
        assert!(
            visible
                .forest
                .iter()
                .any(|node| { node.is_core("heading") && texts(&node.children) == ["violet"] })
        );
    }

    #[test]
    fn escaped_closing_delimiters_remain_literal_inline_content() {
        let evaluated = Evaluator::default().evaluate("*left \\* middle* __left \\__ middle__");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        let strong = &evaluated.forest[0];
        assert!(strong.is_core("strong"));
        assert_eq!(joined_texts(&strong.children), "left * middle");
        assert!(evaluated.forest.iter().any(|node| {
            node.is_core("underline") && joined_texts(&node.children) == "left __ middle"
        }));
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
            evaluated
                .forest
                .iter()
                .any(|node| text(node) == Some("Hello, World!"))
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
        assert!(evaluated.forest.iter().any(|node| {
            node.is_core("callout")
                && node.children.iter().any(|child| {
                    child.is_core("heading") && child.get("level") == Some(&NodeValue::Int(3))
                })
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
