//! Node reduction and shaping for the unified Call Reduction model.
//!
//! Lowering emits a [`Node`] call forest; reduction on the node engine
//! replaces every handled call with its output forest, then recursive shaping
//! folds the reduced forest into an [`ElementTree`]. Shaping consumes and
//! produces `Node` forests directly; no separate terminal-node type exists.

use std::collections::HashMap;

use notist_model::{
    BodyMode, ElementName, ElementSchema, Node, NodeValue, ShapingKind, ShapingRole, TextRange,
};

use crate::type_system::default_to_value;
use crate::{
    BoundArguments, EvalDiagnostic, FunctionContext, FunctionInput, FunctionRegistry, Type, Value,
};

/// The shaped, canonical node tree.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ElementTree {
    /// Top-level roots. No pending call or `core::parbreak` remains.
    pub roots: Vec<Node>,
}

/// Resource limits for one reduction run.
#[derive(Clone, Debug)]
pub struct ReduceLimits {
    /// Maximum nesting depth for generated call chains.
    pub max_depth: usize,
    /// Maximum number of dispatched calls.
    pub max_calls: usize,
}

impl Default for ReduceLimits {
    fn default() -> Self {
        Self {
            max_depth: 64,
            max_calls: 10_000,
        }
    }
}

/// Mutable reduction budget.
#[derive(Clone, Debug)]
pub struct ReduceFrame {
    /// Current call-chain depth.
    pub depth: usize,
    /// Remaining dispatch budget.
    pub remaining_calls: usize,
}

impl ReduceFrame {
    /// Creates a root frame with the given limits.
    pub fn root(limits: &ReduceLimits) -> Self {
        Self {
            depth: 0,
            remaining_calls: limits.max_calls,
        }
    }

    /// Runs the three dispatch checks for one entry by name and range.
    fn dispatch_entry(
        &mut self,
        limits: &ReduceLimits,
        _name: &str,
        range: TextRange,
    ) -> Result<(), Vec<EvalDiagnostic>> {
        if self.depth >= limits.max_depth {
            return Err(vec![EvalDiagnostic {
                message: format!(
                    "call reduction exceeded the maximum depth of {}",
                    limits.max_depth
                ),
                range,
            }]);
        }
        if self.remaining_calls == 0 {
            return Err(vec![EvalDiagnostic {
                message: format!(
                    "call reduction exceeded the maximum budget of {} calls",
                    limits.max_calls
                ),
                range,
            }]);
        }
        self.remaining_calls -= 1;
        self.depth += 1;
        Ok(())
    }
}

/// Declarative shaping rules contributed by core and plugin packages.
#[derive(Clone, Debug, Default)]
pub struct ShapingRegistry {
    schemas: HashMap<ElementName, ElementSchema>,
}

impl ShapingRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers or replaces one schema entry.
    pub fn insert(&mut self, schema: ElementSchema) -> Option<ElementSchema> {
        self.schemas.insert(schema.name.clone(), schema)
    }

    /// Looks up a package-specific schema entry, excluding core fallback.
    pub fn get(&self, name: &ElementName) -> Option<&ElementSchema> {
        self.schemas.get(name)
    }

    /// Iterates over schemas registered in this registry.
    pub fn schemas(&self) -> impl Iterator<Item = &ElementSchema> {
        self.schemas.values()
    }

    /// Resolves the schema supplied by the composition root.
    pub fn effective(&self, name: &ElementName) -> Option<&ElementSchema> {
        self.schemas.get(name)
    }

    /// Resolves the shaping kind, falling back to `Node.block`.
    pub fn kind_of(&self, node: &Node) -> ShapingKind {
        match self
            .effective(&ElementName::parse(&node.name))
            .map(|schema| schema.kind)
        {
            Some(ShapingKind::Unspecified) | None => {
                if node.block {
                    ShapingKind::Block
                } else {
                    ShapingKind::Inline
                }
            }
            Some(kind) => kind,
        }
    }

    /// Resolves the body mode, falling back to `Flow`.
    pub fn body_mode_of(&self, node: &Node) -> BodyMode {
        self.effective(&ElementName::parse(&node.name))
            .map_or(BodyMode::Flow, |schema| schema.body_mode)
    }

    /// Resolves the shaping role.
    pub fn role_of(&self, node: &Node) -> ShapingRole {
        self.effective(&ElementName::parse(&node.name))
            .map_or(ShapingRole::None, |schema| schema.role)
    }
}

fn bind_validated_arguments(
    signature: &notist_model::FunctionSignature,
    provided: &HashMap<String, Value>,
    call_name: &str,
    range: TextRange,
    diagnostics: &mut Vec<EvalDiagnostic>,
) -> HashMap<String, Value> {
    let mut values = HashMap::new();
    for parameter in &signature.parameters {
        let Some(argument) = provided.get(&parameter.name) else {
            if let Some(default) = &parameter.default {
                values.insert(parameter.name.clone(), default_to_value(default));
            } else {
                diagnostics.push(EvalDiagnostic {
                    message: format!(
                        "missing argument `{}` for function `{call_name}`",
                        parameter.name
                    ),
                    range,
                });
            }
            continue;
        };

        let mut value = argument.clone();

        if !parameter.ty.accepts(&value.ty()) {
            diagnostics.push(EvalDiagnostic {
                message: format!(
                    "type mismatch for argument `{}`: expected {}, found {}",
                    parameter.name,
                    parameter.ty,
                    value.ty()
                ),
                range,
            });
            continue;
        }
        if parameter.ty == Type::Float
            && let Value::Int(integer) = value
        {
            value = Value::Float(integer as f64);
        }
        values.insert(parameter.name.clone(), value);
    }
    values
}

/// Recursively shapes a reduced node forest using an empty schema registry.
pub fn shape_flat(nodes: &[Node]) -> ElementTree {
    shape_flat_with(nodes, &ShapingRegistry::new())
}

/// Recursively shapes a reduced node forest using a caller-provided schema registry.
pub fn shape_flat_with(nodes: &[Node], registry: &ShapingRegistry) -> ElementTree {
    ElementTree {
        roots: shape_flow(nodes, registry),
    }
}

fn shape_node(node: &Node, registry: &ShapingRegistry) -> Node {
    let children = match registry.body_mode_of(node) {
        BodyMode::Shaped | BodyMode::None => node.children.clone(),
        BodyMode::Inline => shape_inline(&node.children, registry),
        BodyMode::Flow => shape_flow(&node.children, registry),
        BodyMode::Cells => node.children.clone(),
    };
    Node {
        args: shape_args(&node.args, registry),
        children,
        ..node.clone()
    }
}

fn shape_args(
    args: &[(String, NodeValue)],
    registry: &ShapingRegistry,
) -> Vec<(String, NodeValue)> {
    args.iter()
        .map(|(name, value)| (name.clone(), shape_node_value(value, registry)))
        .collect()
}

fn shape_node_value(value: &NodeValue, registry: &ShapingRegistry) -> NodeValue {
    match value {
        NodeValue::Stream(nodes) => NodeValue::Stream(shape_inline(nodes, registry)),
        NodeValue::Array(values) => NodeValue::Array(
            values
                .iter()
                .map(|value| shape_node_value(value, registry))
                .collect(),
        ),
        value => value.clone(),
    }
}

fn shape_inline(nodes: &[Node], registry: &ShapingRegistry) -> Vec<Node> {
    nodes
        .iter()
        .filter(|node| registry.kind_of(node) != ShapingKind::Separator)
        .cloned()
        .collect()
}

fn shape_flow(nodes: &[Node], registry: &ShapingRegistry) -> Vec<Node> {
    let mut blocks = Vec::new();
    let mut paragraph = Vec::new();

    for node in nodes {
        match registry.kind_of(node) {
            ShapingKind::Separator => flush_paragraph(&mut paragraph, &mut blocks),
            ShapingKind::Inline | ShapingKind::Unspecified if !node.block => {
                if paragraph.is_empty() && is_framing_whitespace(node) {
                    continue;
                }
                paragraph.push(node.clone());
            }
            ShapingKind::Inline | ShapingKind::Block | ShapingKind::Unspecified => {
                if registry.role_of(node) == ShapingRole::Item {
                    flush_paragraph(&mut paragraph, &mut blocks);
                    push_list_item(&mut blocks, node, registry);
                } else {
                    flush_paragraph(&mut paragraph, &mut blocks);
                    blocks.push(shape_node(node, registry));
                }
            }
        }
    }
    flush_paragraph(&mut paragraph, &mut blocks);
    group_sections(blocks, registry)
}

fn is_framing_whitespace(node: &Node) -> bool {
    node.is_core("text")
        && node
            .get("text")
            .is_some_and(|value| matches!(value, NodeValue::String(text) if text.trim().is_empty()))
}

fn flush_paragraph(paragraph: &mut Vec<Node>, blocks: &mut Vec<Node>) {
    if paragraph.is_empty() {
        return;
    }
    let range = TextRange::new(
        paragraph.first().unwrap().range.start,
        paragraph.last().unwrap().range.end,
    );
    let mut node = Node::block_call("core::paragraph", range);
    node.children = std::mem::take(paragraph);
    blocks.push(node);
}

fn push_list_item(blocks: &mut Vec<Node>, node: &Node, registry: &ShapingRegistry) {
    let ordered = node
        .get("ordered")
        .is_some_and(|value| matches!(value, NodeValue::Bool(true)));
    if let Some(last) = blocks.last_mut()
        && last.is_core("list")
        && last.get("ordered").is_some_and(
            |value| matches!(value, NodeValue::Bool(ordered_value) if *ordered_value == ordered),
        )
    {
        last.range.end = node.range.end;
        last.children.push(shape_node(node, registry));
        return;
    }

    let mut list = Node::block_call("core::list", node.range).arg("ordered", ordered);
    list.children.push(shape_node(node, registry));
    blocks.push(list);
}

fn group_sections(blocks: Vec<Node>, registry: &ShapingRegistry) -> Vec<Node> {
    #[derive(Clone)]
    struct OpenSection {
        level: i64,
        heading: Node,
        body: Vec<Node>,
    }

    fn push_section(output: &mut Vec<Node>, open: &mut [OpenSection], section: OpenSection) {
        let range = TextRange::new(
            section.heading.range.start,
            section
                .body
                .last()
                .map_or(section.heading.range.end, |node| node.range.end),
        );
        let mut node = Node::block_call("core::section", range).arg("level", section.level);
        node.children.push(section.heading);
        node.children.extend(section.body);
        match open.last_mut() {
            Some(parent) => parent.body.push(node),
            None => output.push(node),
        }
    }

    let mut output = Vec::new();
    let mut open: Vec<OpenSection> = Vec::new();
    for block in blocks {
        let heading_level = (registry.role_of(&block) == ShapingRole::Heading)
            .then(|| {
                block.get("level").and_then(|value| match value {
                    NodeValue::Int(level) => Some(*level),
                    _ => None,
                })
            })
            .flatten();
        if let Some(level) = heading_level {
            while open.last().is_some_and(|section| section.level >= level) {
                let section = open.pop().unwrap();
                push_section(&mut output, &mut open, section);
            }
            open.push(OpenSection {
                level,
                heading: block,
                body: Vec::new(),
            });
        } else if let Some(parent) = open.last_mut() {
            parent.body.push(block);
        } else {
            output.push(block);
        }
    }
    while let Some(section) = open.pop() {
        push_section(&mut output, &mut open, section);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Evaluator;
    use crate::leaf::node_engine::reduce_nodes_recovering;

    fn text(value: &str) -> Node {
        Node::call("core::text", TextRange::new(0, 0)).arg("text", value)
    }

    #[test]
    fn source_evaluates_to_shaped_node_tree() {
        let evaluator = Evaluator::default();
        let evaluation = evaluator.evaluate("hello\n\nworld");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let tree = evaluation.tree;

        assert_eq!(tree.roots.len(), 2);
        assert!(tree.roots.iter().all(|node| node.is_core("paragraph")));
        assert_eq!(
            tree.roots[0].children.len(),
            1,
            "first paragraph contains one text node"
        );
        assert!(tree.roots[0].children[0].is_core("text"));
    }

    #[test]
    fn node_reduction_composes_details_raw_and_text() {
        let registry = FunctionRegistry::with_builtins();
        let limits = ReduceLimits::default();
        let mut frame = ReduceFrame::root(&limits);

        let call = Node::block_call("details", TextRange::new(0, 0))
            .arg("summary", NodeValue::Stream(vec![text("Shader")]))
            .arg("open", false)
            .child(
                Node::block_call("raw", TextRange::new(0, 0))
                    .arg("source", "fn main() {}")
                    .arg("lang", "wgsl")
                    .arg("block", true),
            )
            .child(text("fallback"));

        let (output, errors) = reduce_nodes_recovering(vec![call], &registry, &limits, &mut frame);
        assert!(errors.is_empty(), "{errors:?}");
        let tree = shape_flat(&output);

        assert_eq!(tree.roots.len(), 1);
        assert!(tree.roots[0].is_core("details"));
        assert!(
            tree.roots[0]
                .children
                .iter()
                .any(|node| node.is_core("raw"))
        );
        assert!(
            tree.roots[0]
                .children
                .iter()
                .any(|node| node.is_core("paragraph") && node.children[0].is_core("text"))
        );
    }

    #[test]
    fn qualified_core_aliases_reduce_in_the_node_engine() {
        let registry = FunctionRegistry::with_builtins();
        let limits = ReduceLimits::default();
        let mut frame = ReduceFrame::root(&limits);
        let call = Node::block_call("core::details", TextRange::new(0, 0)).child(
            Node::block_call("core::raw", TextRange::new(0, 0))
                .arg("source", "fn main() {}")
                .arg("block", true),
        );

        let (output, errors) = reduce_nodes_recovering(vec![call], &registry, &limits, &mut frame);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(output.len(), 1);
        assert!(output[0].is_core("details"));
        assert!(output[0].children.iter().any(|node| node.is_core("raw")));
    }

    #[test]
    fn evaluation_shapes_sections_recursively() {
        let evaluator = Evaluator::default();
        let evaluation = evaluator.evaluate(
            "= Title

Body text",
        );

        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert_eq!(evaluation.tree.roots.len(), 1);
        assert!(evaluation.tree.roots[0].is_core("section"));
        assert!(evaluation.tree.roots[0].children[0].is_core("heading"));
        assert!(
            evaluation.tree.roots[0]
                .children
                .iter()
                .any(|node| node.is_core("paragraph"))
        );
    }

    #[test]
    fn stream_pipeline_binds_positional_arguments() {
        let evaluator = Evaluator::default();
        let evaluation = evaluator.evaluate("#raw(\"fn main() {}\")\n");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let Some(raw) = evaluation.forest.iter().find(|node| node.is_core("raw")) else {
            panic!("expected raw leaf");
        };
        assert!(raw.is_core("raw"));
    }

    #[test]
    fn stream_pipeline_binds_positional_arguments_past_optional_parameters() {
        let evaluator = Evaluator::default();
        let evaluation = evaluator.evaluate("#raw(\"fn\", \"rust\", true)");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let Some(raw) = evaluation.forest.first() else {
            panic!("expected raw leaf, got {:#?}", evaluation.forest)
        };
        assert_eq!(
            raw.get("lang"),
            Some(&notist_model::NodeValue::String("rust".into()))
        );
        assert_eq!(raw.get("block"), Some(&notist_model::NodeValue::Bool(true)));
    }

    #[test]
    fn shaping_registry_drives_plugin_node_body_mode() {
        use notist_model::{ElementName, ElementSchema, ShapingKind, ShapingRole};

        let (_, mut registry) = crate::test_core::registry();
        assert_eq!(
            registry
                .get(&ElementName::core("parbreak"))
                .map(|schema| schema.kind),
            Some(ShapingKind::Separator)
        );
        registry.insert(ElementSchema::new(
            ElementName::plugin("demo", "box"),
            ShapingKind::Block,
            notist_model::BodyMode::Flow,
            ShapingRole::None,
        ));
        let mut leaf = Node::block_call("demo::box", TextRange::new(0, 0));
        leaf.children = vec![
            text("first"),
            Node::call("core::parbreak", TextRange::new(0, 0)),
            text("second"),
        ];
        let tree = shape_flat_with(&[leaf], &registry);

        assert_eq!(tree.roots.len(), 1);
        assert!(!tree.roots[0].is_core("box"));
        assert_eq!(tree.roots[0].children.len(), 2);
        assert!(
            tree.roots[0]
                .children
                .iter()
                .all(|node| node.is_core("paragraph"))
        );
    }

    #[test]
    fn stream_tree_groups_sections_lists_and_tables() {
        let source = "= Title\n\nBefore after.\n\n- one\n  - nested\n- two\n\n| a | b |\n|---|---|\n| 1 | 2 |";
        let evaluator = Evaluator::default();
        let stream = evaluator.evaluate(source);
        assert!(stream.diagnostics.is_empty(), "{:?}", stream.diagnostics);
        assert_eq!(stream.tree.roots.len(), 1);
        let section = &stream.tree.roots[0];
        assert!(section.is_core("section"));
        assert!(section.children.iter().any(|node| node.is_core("list")));
        assert!(section.children.iter().any(|node| node.is_core("table")));
    }

    #[test]
    fn stream_pipeline_lowers_list_and_table_sugar_directly() {
        let evaluator = Evaluator::default();
        let list = evaluator.evaluate("- a\n  - b\n- c");
        assert!(list.diagnostics.is_empty(), "{:?}", list.diagnostics);
        assert!(
            list.lowered.iter().any(|node| node.name == "item"),
            "{:#?}",
            list.lowered
        );
        assert_eq!(list.tree.roots.len(), 1);
        assert!(list.tree.roots[0].is_core("list"));
        assert!(
            list.tree.roots[0].children[0]
                .children
                .iter()
                .any(|node| node.is_core("list"))
        );

        let table = evaluator.evaluate("| a | b |\n|---|---|\n| 1 | 2 |");
        assert!(table.diagnostics.is_empty(), "{:?}", table.diagnostics);
        assert!(
            table.lowered.iter().any(|node| node.name == "table"),
            "{:#?}",
            table.lowered
        );
        assert_eq!(table.tree.roots.len(), 1);
        assert!(table.tree.roots[0].is_core("table"));
    }

    #[test]
    fn stream_pipeline_lowers_calls_before_reducing() {
        let evaluator = Evaluator::default();
        let evaluation = evaluator.evaluate("= Title\n\nHello");

        assert!(
            evaluation.lowered.iter().any(|node| node.name == "heading"),
            "{:#?}",
            evaluation.lowered
        );
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert_eq!(evaluation.tree.roots.len(), 1);
        assert!(evaluation.tree.roots[0].is_core("section"));
    }

    #[test]
    fn stream_pipeline_reduces_explicit_calls() {
        let evaluator = Evaluator::default();
        let evaluation = evaluator.evaluate("#details[hello]");

        assert!(
            evaluation.lowered.iter().any(|node| node.name == "details"),
            "{:#?}",
            evaluation.lowered
        );
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert_eq!(evaluation.forest.len(), 1);
        let details = &evaluation.forest[0];
        assert!(details.is_core("details"));
        assert_eq!(evaluation.tree.roots.len(), 1);
        assert!(evaluation.tree.roots[0].is_core("details"));
    }

    #[test]
    fn reduction_enforces_depth_budget_for_recursive_plugins() {
        use crate::{Function, FunctionContext, FunctionInput, FunctionSignature};

        struct Recursive;

        impl Function for Recursive {
            fn name(&self) -> &str {
                "recursive"
            }

            fn signature(&self) -> FunctionSignature {
                FunctionSignature {
                    parameters: Vec::new(),
                    trailing_content: None,
                    result: crate::Type::Content,
                }
            }

            fn call(
                &self,
                _context: &FunctionContext<'_>,
                input: FunctionInput<'_>,
            ) -> Result<Value, Vec<EvalDiagnostic>> {
                // Non-stationary self-return: each dispatch produces two new
                // self-addressed calls, so the budget is the only thing that
                // stops the expansion.
                Ok(Value::Content(vec![
                    Node::call("recursive", input.range),
                    Node::call("recursive", input.range),
                ]))
            }
        }

        let mut registry = FunctionRegistry::new();
        registry.register(Recursive).unwrap();
        let limits = ReduceLimits {
            max_depth: 4,
            max_calls: 100,
        };
        let mut frame = ReduceFrame::root(&limits);
        let call = Node::call("recursive", TextRange::new(0, 0));

        let (_, errors) = reduce_nodes_recovering(vec![call], &registry, &limits, &mut frame);
        assert!(
            errors
                .iter()
                .any(|diagnostic| diagnostic.message.contains("maximum depth")),
            "{errors:?}"
        );
    }

    #[test]
    fn node_reduction_reports_invalid_arguments_instead_of_panicking() {
        let registry = FunctionRegistry::with_builtins();
        let limits = ReduceLimits::default();
        let mut frame = ReduceFrame::root(&limits);
        let call = Node::block_call("heading", TextRange::new(0, 0))
            .arg("level", "not an int")
            .child(text("title"));

        let (_, errors) = reduce_nodes_recovering(vec![call], &registry, &limits, &mut frame);
        assert!(
            errors
                .iter()
                .any(|diagnostic| diagnostic.message.contains("type mismatch")),
            "{errors:?}"
        );
    }
}

/// The unified-node reduction engine.
///
/// Operates on `notist_model::Node` — the single representation where a call
/// awaiting dispatch and a terminal leaf are the same shape. Reduction is a
/// fixpoint iteration: a registered handler replaces the node with its
/// output; a node nobody handles *is* a leaf, and its children and pending
/// argument streams still reduce.
pub mod node_engine {
    use super::*;

    /// Maps a node value onto the evaluator value domain.
    ///
    /// Streams were already reduced by the caller before dispatch, so they
    /// map straight onto [`Value::Content`]. Arrays have no eval-level value
    /// yet and stay invisible to handlers.
    fn value_from_node_value(value: &NodeValue) -> Option<Value> {
        match value {
            NodeValue::None => Some(Value::Unit),
            NodeValue::Bool(value) => Some(Value::Bool(*value)),
            NodeValue::Int(value) => Some(Value::Int(*value)),
            NodeValue::Float(value) => Some(Value::Float(*value)),
            NodeValue::String(value) => Some(Value::String(value.clone())),
            NodeValue::Stream(forest) => Some(Value::Content(forest.clone())),
            NodeValue::Array(_) => None,
            NodeValue::Target(reference) => Some(Value::Target(reference.clone())),
        }
    }

    /// Reduces a unified node stream while preserving successful siblings.
    pub fn reduce_nodes_recovering(
        nodes: Vec<Node>,
        registry: &FunctionRegistry,
        limits: &ReduceLimits,
        frame: &mut ReduceFrame,
    ) -> (Vec<Node>, Vec<EvalDiagnostic>) {
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();
        for node in nodes {
            match reduce_node(node, registry, limits, frame) {
                Ok(mut reduced) => output.append(&mut reduced),
                Err(mut errors) => diagnostics.append(&mut errors),
            }
        }
        (output, diagnostics)
    }

    /// Strict variant: any diagnostic aborts the whole stream.
    pub fn reduce_nodes(
        nodes: Vec<Node>,
        registry: &FunctionRegistry,
        limits: &ReduceLimits,
        frame: &mut ReduceFrame,
    ) -> Result<Vec<Node>, Vec<EvalDiagnostic>> {
        let (output, diagnostics) = reduce_nodes_recovering(nodes, registry, limits, frame);
        if diagnostics.is_empty() {
            Ok(output)
        } else {
            Err(diagnostics)
        }
    }

    /// Reduces one node to its output.
    ///
    /// Children and pending argument streams reduce first; then a registered
    /// handler consumes the node, or it survives as a leaf.
    fn reduce_node(
        mut node: Node,
        registry: &FunctionRegistry,
        limits: &ReduceLimits,
        frame: &mut ReduceFrame,
    ) -> Result<Vec<Node>, Vec<EvalDiagnostic>> {
        // Descend first: pending argument streams and children always reduce,
        // independent of whether this node itself has a handler.
        for (_, value) in node.args.iter_mut() {
            if let NodeValue::Stream(stream) = value {
                let (reduced, errors) =
                    reduce_nodes_recovering(std::mem::take(stream), registry, limits, frame);
                *stream = reduced;
                if !errors.is_empty() {
                    return Err(errors);
                }
            }
        }
        if !node.children.is_empty() {
            let (reduced, errors) = reduce_nodes_recovering(
                std::mem::take(&mut node.children),
                registry,
                limits,
                frame,
            );
            node.children = reduced;
            if !errors.is_empty() {
                return Err(errors);
            }
        }

        // Fixpoint rule: nobody handles this name → it already is a leaf.
        let Some(function) = registry.get(&node.name) else {
            return Ok(vec![node]);
        };

        frame.dispatch_entry(limits, &node.name, node.range)?;
        tracing::trace!(
            target: "notist_eval",
            element = %node.name,
            depth = frame.depth,
            budget_left = frame.remaining_calls,
            "dispatch"
        );
        let result = dispatch_handler(node, function, registry, limits, frame);
        frame.depth -= 1;
        result
    }

    fn dispatch_handler(
        node: Node,
        function: &dyn crate::Function,
        registry: &FunctionRegistry,
        limits: &ReduceLimits,
        frame: &mut ReduceFrame,
    ) -> Result<Vec<Node>, Vec<EvalDiagnostic>> {
        let signature = function.signature();
        let mut provided: HashMap<String, Value> = HashMap::new();
        for (name, value) in &node.args {
            let Some(value) = value_from_node_value(value) else {
                continue;
            };
            provided.insert(name.clone(), value);
        }

        // Trailing body binds under the signature's parameter name. Children
        // were already reduced by the descend-first pass, so the bound
        // Content forest is the input-side (fully reduced) forest. A named
        // Content argument for the same parameter wins; trailing children on
        // top of it are a duplicate, not an override.
        if let Some(trailing) = signature.trailing_content.as_deref() {
            if provided.contains_key(trailing) {
                if !node.children.is_empty() {
                    return Err(vec![EvalDiagnostic {
                        message: format!("argument `{trailing}` was provided more than once"),
                        range: node.range,
                    }]);
                }
            } else {
                provided.insert(trailing.to_owned(), Value::Content(node.children.clone()));
            }
        } else if !node.children.is_empty() {
            return Err(vec![EvalDiagnostic {
                message: format!("function `{}` does not accept trailing content", node.name),
                range: node.range,
            }]);
        }

        let mut diagnostics = Vec::new();
        let values = bind_validated_arguments(
            &signature,
            &provided,
            &node.name,
            node.range,
            &mut diagnostics,
        );
        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }

        let input = FunctionInput {
            name: &node.name,
            arguments: BoundArguments::from_values(values),
            range: node.range,
        };
        match function.call(
            &FunctionContext {
                registry,
                depth: frame.depth,
            },
            input,
        ) {
            Ok(Value::Content(forest)) => {
                // The output forest re-enters the fixpoint. A handler that
                // returns its own call node unchanged declares identity — the
                // replacement changed nothing, so the fixpoint is reached.
                // Anything else reduces again; a handler that keeps producing
                // fresh self-addressed calls is an author bug caught by the
                // depth/call budget.
                if forest.len() == 1 && forest.first() == Some(&node) {
                    return Ok(forest);
                }
                let (reduced, errors) = reduce_nodes_recovering(forest, registry, limits, frame);
                if errors.is_empty() {
                    Ok(reduced)
                } else {
                    Err(errors)
                }
            }
            Ok(value) => Err(vec![EvalDiagnostic {
                message: format!(
                    "function `{}` returned {}, expected Content",
                    node.name,
                    value.ty()
                ),
                range: node.range,
            }]),
            Err(errors) => Err(errors),
        }
    }

    /// Builds the shaped canonical tree from a reduced node forest.
    pub fn nodes_to_element_tree(nodes: &[Node], shaping: &ShapingRegistry) -> ElementTree {
        shape_flat_with(nodes, shaping)
    }

    /// Full-pipeline result over the unified node representation.
    #[derive(Clone, Debug, Default)]
    pub struct NodeEvaluation {
        /// Reduced unified-node forest.
        pub forest: Vec<Node>,
        /// The recursively shaped canonical tree.
        pub tree: ElementTree,
        /// Parse, lower, and reduction diagnostics.
        pub diagnostics: Vec<EvalDiagnostic>,
    }

    /// Runs a lowered call forest through the node engine and shapes the
    /// result.
    pub fn evaluate_to_nodes(
        nodes: Vec<Node>,
        registry: &FunctionRegistry,
        shaping: &ShapingRegistry,
    ) -> NodeEvaluation {
        let limits = ReduceLimits::default();
        let mut frame = ReduceFrame::root(&limits);
        let (forest, diagnostics) = reduce_nodes_recovering(nodes, registry, &limits, &mut frame);
        let tree = shape_flat_with(&forest, shaping);
        NodeEvaluation {
            forest,
            tree,
            diagnostics,
        }
    }

    /// Collects every node name in depth-first order.
    pub fn collect_names(nodes: &[Node]) -> Vec<String> {
        let mut names = Vec::new();
        fn walk(node: &Node, names: &mut Vec<String>) {
            names.push(node.name.clone());
            for value in &node.args {
                if let NodeValue::Stream(stream) = &value.1 {
                    walk_stream(stream, names);
                }
            }
            for child in &node.children {
                walk(child, names);
            }
        }
        fn walk_stream(nodes: &[Node], names: &mut Vec<String>) {
            for node in nodes {
                walk(node, names);
            }
        }
        walk_stream(nodes, &mut names);
        names
    }

    /// Whether every argument stream in the forest is empty (fully reduced).
    pub fn fully_reduced(nodes: &[Node]) -> bool {
        fn walk(node: &Node) -> bool {
            node.args.iter().all(|(_, v)| match v {
                NodeValue::Stream(stream) => stream.is_empty(),
                _ => true,
            }) && node.children.iter().all(walk)
        }
        nodes.iter().all(walk)
    }
}

#[cfg(test)]
mod engine_tests {
    use super::node_engine::{collect_names, fully_reduced, reduce_nodes_recovering};
    use crate::leaf::{FunctionRegistry, ReduceFrame, ReduceLimits};
    use notist_model::{Node, NodeValue, TextRange};
    use std::collections::HashMap;

    #[test]
    fn plain_text_flows_through_the_node_engine() {
        let registry = FunctionRegistry::with_builtins();
        let parse = notist_syntax::parse("hello");
        let lowered = crate::stream_lower::lower_document_with_bindings(
            "hello",
            &parse.root,
            0,
            &registry,
            HashMap::new(),
        );
        let (_, shaping) = crate::test_core::registry();
        let evaluation = super::node_engine::evaluate_to_nodes(lowered.nodes, &registry, &shaping);
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let names = collect_names(&evaluation.forest);
        assert!(names.iter().any(|name| name.ends_with("text")), "{names:?}");
        assert!(fully_reduced(&evaluation.forest));
    }

    #[test]
    fn unknown_names_survive_as_leaves_with_reduced_children() {
        let registry = FunctionRegistry::with_builtins();
        let limits = ReduceLimits::default();
        let mut frame = ReduceFrame::root(&limits);

        let mystery = Node::block_call("mystery::widget", TextRange::new(0, 3))
            .child(Node::call("core::text", TextRange::new(4, 6)).arg("text", "kept"));
        let (output, errors) =
            reduce_nodes_recovering(vec![mystery], &registry, &limits, &mut frame);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].name, "mystery::widget");
        assert_eq!(output[0].children[0].name, "core::text");
        assert!(matches!(
            output[0].children[0].get("text"),
            Some(NodeValue::String(value)) if value == "kept"
        ));
    }

    #[test]
    fn handler_dispatch_runs_on_the_node_path() {
        let registry = FunctionRegistry::with_builtins();
        let limits = ReduceLimits::default();
        let mut frame = ReduceFrame::root(&limits);

        // `rule` is a registered builtin with no arguments.
        assert!(
            registry.get("rule").is_some(),
            "rule must be a registered builtin"
        );
        let call = Node::block_call("rule", TextRange::new(0, 6));
        let (output, errors) = reduce_nodes_recovering(vec![call], &registry, &limits, &mut frame);
        assert!(errors.is_empty(), "{errors:?}");
        let names = collect_names(&output);
        assert!(names.iter().any(|name| name.contains("rule")), "{names:?}");
    }

    #[test]
    fn trailing_body_binds_and_children_reduce() {
        let registry = FunctionRegistry::with_builtins();
        let limits = ReduceLimits::default();
        let mut frame = ReduceFrame::root(&limits);

        let call = Node::block_call("details", TextRange::new(0, 9))
            .arg("open", true)
            .child(Node::call("core::text", TextRange::new(10, 14)).arg("text", "body"));
        let (output, errors) = reduce_nodes_recovering(vec![call], &registry, &limits, &mut frame);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(fully_reduced(&output));
    }

    #[test]
    fn handler_forests_reenter_the_fixpoint() {
        use crate::{EvalDiagnostic, Function, FunctionContext, FunctionInput, FunctionSignature};

        /// Macro-style handler: expands into a fresh `core::raw` call plus a
        /// terminal data leaf; the fixpoint reduces the call and keeps the
        /// leaf.
        struct Wrapper;

        impl Function for Wrapper {
            fn name(&self) -> &str {
                "test::wrapper"
            }

            fn signature(&self) -> FunctionSignature {
                FunctionSignature {
                    parameters: Vec::new(),
                    trailing_content: None,
                    result: crate::Type::Content,
                }
            }

            fn call(
                &self,
                _context: &FunctionContext<'_>,
                input: FunctionInput<'_>,
            ) -> Result<crate::Value, Vec<EvalDiagnostic>> {
                Ok(crate::Value::Content(vec![
                    Node::block_call("core::raw", input.range)
                        .arg("source", "fn main() {}")
                        .arg("block", true),
                    Node::call("test::canvas", input.range).arg("source", "wgsl"),
                ]))
            }
        }

        let mut registry = FunctionRegistry::with_builtins();
        registry.register(Wrapper).unwrap();
        let limits = ReduceLimits::default();
        let mut frame = ReduceFrame::root(&limits);

        let call = Node::call("test::wrapper", TextRange::new(0, 0));
        let (output, errors) = reduce_nodes_recovering(vec![call], &registry, &limits, &mut frame);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(output.len(), 2);
        // The `core::raw` call was reduced by its handler; the identity
        // rule keeps the result terminal instead of looping on the alias.
        assert_eq!(output[0].name, "core::raw");
        assert!(output[0].block);
        assert_eq!(output[1].name, "test::canvas");
        assert!(fully_reduced(&output));
    }

    #[test]
    fn identity_returns_terminate_validator_handlers() {
        // A handler that returns its own call node unchanged declares
        // identity: the fixpoint treats the replacement as "no change" and
        // stops without consuming the budget.
        let registry = FunctionRegistry::with_builtins();
        let limits = ReduceLimits {
            max_depth: 4,
            max_calls: 8,
        };
        let mut frame = ReduceFrame::root(&limits);

        let call = Node::call("core::text", TextRange::new(0, 4)).arg("text", "kept");
        let (output, errors) = reduce_nodes_recovering(vec![call], &registry, &limits, &mut frame);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].name, "core::text");
        assert!(matches!(
            output[0].get("text"),
            Some(NodeValue::String(value)) if value == "kept"
        ));
    }
}
