#![allow(dead_code)]

use std::sync::Arc;

use notist_eval::{
    EvalDiagnostic, Function, FunctionContext, FunctionInput, FunctionRegistry, FunctionSignature,
    PluginContribution, RegistryError, ShapingRegistry, Type, Value,
};
use notist_model::{
    BodyMode, ElementName, ElementSchema, Node, NodeValue, ShapingKind, ShapingRole,
    TableAlignment, TableLayoutError, table_layout_nodes,
};

/// Builds an optional constructor argument from an optional Content value.
fn optional_content_arg(name: &str, value: Option<Vec<Node>>) -> Option<(String, NodeValue)> {
    value.map(|forest| (name.to_owned(), NodeValue::Stream(forest)))
}

/// The local constructor name of a node, ignoring the `core::` qualifier.
fn core_local(name: &str) -> &str {
    name.strip_prefix("core::").unwrap_or(name)
}

/// Whether the node is a whitespace-only `core::text` node.
fn is_whitespace_text(node: &Node) -> bool {
    core_local(&node.name) == "text"
        && matches!(node.get("text"), Some(NodeValue::String(text)) if text.trim().is_empty())
}

/// Whether the node is a `core::parbreak` node.
fn is_parbreak(node: &Node) -> bool {
    core_local(&node.name) == "parbreak"
}

/// Builds the semantic contribution provided by the native core package.
pub fn contribution() -> PluginContribution {
    let functions: Vec<Arc<dyn Function>> = vec![
        Arc::new(LinkFunction),
        Arc::new(HeadingFunction),
        Arc::new(RawFunction),
        Arc::new(CalloutFunction),
        Arc::new(DetailsFunction),
        Arc::new(ItemFunction),
        Arc::new(TableCellFunction),
        Arc::new(TableFunction),
        Arc::new(FigureFunction),
        Arc::new(StrongFunction),
        Arc::new(EmphFunction),
        Arc::new(StrikeFunction),
        Arc::new(UnderlineFunction),
        Arc::new(RuleFunction),
        Arc::new(TextFunction),
        Arc::new(ParbreakFunction),
    ];
    let signatures = functions
        .iter()
        .map(|function| (function.name().to_owned(), function.signature()))
        .collect();
    let aliases = [
        ("core::link", "link"),
        ("core::heading", "heading"),
        ("core::raw", "raw"),
        ("core::callout", "callout"),
        ("core::details", "details"),
        ("core::item", "item"),
        ("core::table-cell", "table-cell"),
        ("core::table", "table"),
        ("core::figure", "figure"),
        ("core::strong", "strong"),
        ("core::emph", "emph"),
        ("core::strike", "strike"),
        ("core::underline", "underline"),
        ("core::rule", "rule"),
        ("core::text", "text"),
        ("core::parbreak", "parbreak"),
    ]
    .into_iter()
    .map(|(alias, target)| (alias.into(), target.into()))
    .collect();
    PluginContribution {
        package: "core".into(),
        functions,
        signatures,
        elements: core_schemas(),
        aliases,
    }
}

/// Installs core functions, aliases, and shaping schemas atomically.
pub fn register_into(
    registry: &mut FunctionRegistry,
    shaping: &mut ShapingRegistry,
) -> Result<(), RegistryError> {
    registry.register_contribution(shaping, &contribution())
}

/// Creates registries containing the complete native core contribution.
pub fn registry() -> (FunctionRegistry, ShapingRegistry) {
    let mut functions = FunctionRegistry::new();
    let mut shaping = ShapingRegistry::new();
    register_into(&mut functions, &mut shaping).expect("core contribution must be valid");
    (functions, shaping)
}

fn core_schemas() -> Vec<ElementSchema> {
    let mut registry = ShapingRegistry::new();
    let core = |registry: &mut ShapingRegistry, local: &str, kind, body, role| {
        registry.insert(ElementSchema::new(
            ElementName::core(local),
            kind,
            body,
            role,
        ));
    };
    core(
        &mut registry,
        "text",
        ShapingKind::Inline,
        BodyMode::None,
        ShapingRole::None,
    );
    core(
        &mut registry,
        "reference",
        ShapingKind::Inline,
        BodyMode::None,
        ShapingRole::None,
    );
    core(
        &mut registry,
        "parbreak",
        ShapingKind::Separator,
        BodyMode::None,
        ShapingRole::None,
    );
    core(
        &mut registry,
        "heading",
        ShapingKind::Block,
        BodyMode::Inline,
        ShapingRole::Heading,
    );
    core(
        &mut registry,
        "rule",
        ShapingKind::Block,
        BodyMode::None,
        ShapingRole::None,
    );
    core(
        &mut registry,
        "item",
        ShapingKind::Block,
        BodyMode::Flow,
        ShapingRole::Item,
    );
    core(
        &mut registry,
        "table-cell",
        ShapingKind::Block,
        BodyMode::Flow,
        ShapingRole::None,
    );
    core(
        &mut registry,
        "table",
        ShapingKind::Block,
        BodyMode::Cells,
        ShapingRole::None,
    );
    core(
        &mut registry,
        "figure",
        ShapingKind::Block,
        BodyMode::Flow,
        ShapingRole::None,
    );
    core(
        &mut registry,
        "callout",
        ShapingKind::Block,
        BodyMode::Flow,
        ShapingRole::None,
    );
    core(
        &mut registry,
        "details",
        ShapingKind::Block,
        BodyMode::Flow,
        ShapingRole::None,
    );
    core(
        &mut registry,
        "raw",
        ShapingKind::Unspecified,
        BodyMode::None,
        ShapingRole::None,
    );
    for local in ["strong", "emph", "underline", "strike"] {
        core(
            &mut registry,
            local,
            ShapingKind::Inline,
            BodyMode::Inline,
            ShapingRole::None,
        );
    }
    for local in ["paragraph", "list", "section"] {
        core(
            &mut registry,
            local,
            ShapingKind::Block,
            BodyMode::Shaped,
            ShapingRole::None,
        );
    }
    core(
        &mut registry,
        "unresolved-call",
        ShapingKind::Unspecified,
        BodyMode::Flow,
        ShapingRole::None,
    );
    registry.schemas().cloned().collect()
}

struct TextFunction;

impl Function for TextFunction {
    fn name(&self) -> &str {
        "text"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::FunctionSignature {
            parameters: vec![notist_model::Parameter {
                name: "text".into(),
                ty: Type::String,
                default: None,
            }],
            trailing_content: None,
            result: Type::Content,
        }
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<Value, Vec<EvalDiagnostic>> {
        Ok(Value::Content(vec![
            Node::call("core::text", input.range)
                .arg("text", input.arguments.string("text").to_owned()),
        ]))
    }
}

struct ParbreakFunction;

impl Function for ParbreakFunction {
    fn name(&self) -> &str {
        "parbreak"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::empty_content_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<Value, Vec<EvalDiagnostic>> {
        Ok(Value::Content(vec![Node::call(
            "core::parbreak",
            input.range,
        )]))
    }
}

struct LinkFunction;

impl Function for LinkFunction {
    fn name(&self) -> &str {
        "link"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::link_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<Value, Vec<EvalDiagnostic>> {
        let Some(value) = input.arguments.get("target") else {
            return Ok(Value::Content(Vec::new()));
        };
        match value {
            Value::Target(reference) => Ok(Value::Content(vec![Node::call(
                "core::reference",
                input.range,
            )
            .arg("target", NodeValue::Target(reference.clone()))])),
            Value::String(url) => {
                // The String branch is exclusively for external urls.
                match notist_syntax::parse_wiki_reference(url) {
                    Ok(reference)
                        if matches!(
                            reference.module,
                            notist_model::ModuleReference::External(_)
                        ) =>
                    {
                        Ok(Value::Content(vec![Node::call(
                            "core::reference",
                            input.range,
                        )
                        .arg("target", NodeValue::String(url.clone()))]))
                    }
                    Ok(_) => Err(vec![EvalDiagnostic {
                        message:
                            "internal targets must use a `<...>` target literal, not a String"
                                .into(),
                        range: input.range,
                    }]),
                    Err(message) => Err(vec![EvalDiagnostic { message, range: input.range }]),
                }
            }
            other => Err(vec![EvalDiagnostic {
                message: format!(
                    "link target must be a Target or an external url String, found {}",
                    other.ty()
                ),
                range: input.range,
            }]),
        }
    }
}

struct HeadingFunction;

impl Function for HeadingFunction {
    fn name(&self) -> &str {
        "heading"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::heading_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<Value, Vec<EvalDiagnostic>> {
        let level = input.arguments.int("level");
        if level < 1 {
            return Err(vec![EvalDiagnostic {
                message: "heading level must be at least 1".into(),
                range: input.range,
            }]);
        }
        let body = input.arguments.take_content("body");
        let mut node = Node::block_call("core::heading", input.range).arg("level", level);
        node.children = body;
        Ok(Value::Content(vec![node]))
    }
}

struct RawFunction;

impl Function for RawFunction {
    fn name(&self) -> &str {
        "raw"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::raw_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<Value, Vec<EvalDiagnostic>> {
        let language = input.arguments.optional_string("lang").map(str::to_owned);
        let source = input.arguments.string("source").to_owned();
        let block = input.arguments.bool("block");
        if !block && source.contains('\n') {
            return Err(vec![EvalDiagnostic {
                message: "inline raw source must not contain line breaks".into(),
                range: input.range,
            }]);
        }
        Ok(Value::Content(vec![raw_node(
            source,
            block,
            language,
            input.range,
        )]))
    }
}

struct CalloutFunction;

impl Function for CalloutFunction {
    fn name(&self) -> &str {
        "callout"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::callout_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<Value, Vec<EvalDiagnostic>> {
        let kind = input.arguments.string("kind").trim().to_owned();
        if kind.is_empty() {
            return Err(vec![EvalDiagnostic {
                message: "callout kind cannot be empty".into(),
                range: input.range,
            }]);
        }
        let title = input.arguments.take_optional_content("title");
        let body = input.arguments.take_content("body");
        let mut node = Node::block_call("core::callout", input.range).arg("kind", kind);
        node.args.extend(optional_content_arg("title", title));
        node.children = body;
        Ok(Value::Content(vec![node]))
    }
}

struct DetailsFunction;

impl Function for DetailsFunction {
    fn name(&self) -> &str {
        "details"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::details_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<Value, Vec<EvalDiagnostic>> {
        let summary = input.arguments.take_optional_content("summary");
        let open = input.arguments.bool("open");
        let body = input.arguments.take_content("body");
        let mut node = Node::block_call("core::details", input.range);
        node.args.extend(optional_content_arg("summary", summary));
        node.args.push(("open".into(), NodeValue::Bool(open)));
        node.children = body;
        Ok(Value::Content(vec![node]))
    }
}

struct ItemFunction;

impl Function for ItemFunction {
    fn name(&self) -> &str {
        "item"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::item_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<Value, Vec<EvalDiagnostic>> {
        let ordered = input.arguments.bool("ordered");
        let body = input.arguments.take_content("body");
        let mut node = Node::block_call("core::item", input.range).arg("ordered", ordered);
        node.children = body;
        Ok(Value::Content(vec![node]))
    }
}

struct StrongFunction;

impl Function for StrongFunction {
    fn name(&self) -> &str {
        "strong"
    }
    fn signature(&self) -> FunctionSignature {
        notist_model::inline_body_signature()
    }
    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<Value, Vec<EvalDiagnostic>> {
        let mut node = Node::call("core::strong", input.range);
        node.children = input.arguments.take_content("body");
        Ok(Value::Content(vec![node]))
    }
}

struct EmphFunction;

impl Function for EmphFunction {
    fn name(&self) -> &str {
        "emph"
    }
    fn signature(&self) -> FunctionSignature {
        notist_model::inline_body_signature()
    }
    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<Value, Vec<EvalDiagnostic>> {
        let mut node = Node::call("core::emph", input.range);
        node.children = input.arguments.take_content("body");
        Ok(Value::Content(vec![node]))
    }
}

struct StrikeFunction;

impl Function for StrikeFunction {
    fn name(&self) -> &str {
        "strike"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::inline_body_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<Value, Vec<EvalDiagnostic>> {
        let mut node = Node::call("core::strike", input.range);
        node.children = input.arguments.take_content("body");
        Ok(Value::Content(vec![node]))
    }
}

macro_rules! inline_wrapper_function {
    ($function:ident, $name:literal, $variant:literal) => {
        struct $function;

        impl Function for $function {
            fn name(&self) -> &str {
                $name
            }

            fn signature(&self) -> FunctionSignature {
                notist_model::inline_body_signature()
            }

            fn call(
                &self,
                _context: &FunctionContext<'_>,
                mut input: FunctionInput<'_>,
            ) -> Result<Value, Vec<EvalDiagnostic>> {
                let mut node = Node::call($variant, input.range);
                node.children = input.arguments.take_content("body");
                Ok(Value::Content(vec![node]))
            }
        }
    };
}

inline_wrapper_function!(UnderlineFunction, "underline", "core::underline");

fn image_dimension(
    value: Option<i64>,
    name: &str,
    range: notist_model::TextRange,
) -> Result<Option<u32>, Vec<EvalDiagnostic>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if !(1..=u32::MAX as i64).contains(&value) {
        return Err(vec![EvalDiagnostic {
            message: format!("image {name} must be between 1 and {}", u32::MAX),
            range,
        }]);
    }
    Ok(Some(value as u32))
}

struct TableCellFunction;

impl Function for TableCellFunction {
    fn name(&self) -> &str {
        "table-cell"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::table_cell_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<Value, Vec<EvalDiagnostic>> {
        let colspan = input.arguments.int("colspan");
        let rowspan = input.arguments.int("rowspan");
        if !(1..=u16::MAX as i64).contains(&colspan) {
            return Err(vec![EvalDiagnostic {
                message: "table cell colspan must be between 1 and 65535".into(),
                range: input.range,
            }]);
        }
        if !(1..=u16::MAX as i64).contains(&rowspan) {
            return Err(vec![EvalDiagnostic {
                message: "table cell rowspan must be between 1 and 65535".into(),
                range: input.range,
            }]);
        }
        let mut node = Node::block_call("core::table-cell", input.range)
            .arg("colspan", colspan)
            .arg("rowspan", rowspan);
        node.children = input.arguments.take_content("body");
        Ok(Value::Content(vec![node]))
    }
}

struct TableFunction;

impl Function for TableFunction {
    fn name(&self) -> &str {
        "table"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::table_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<Value, Vec<EvalDiagnostic>> {
        let columns = input.arguments.int("columns");
        let header = input.arguments.bool("header");
        if !(1..=u16::MAX as i64).contains(&columns) {
            return Err(vec![EvalDiagnostic {
                message: "table columns must be between 1 and 65535".into(),
                range: input.range,
            }]);
        }
        let alignments =
            match table_alignments(input.arguments.optional_string("align"), columns as usize) {
                Ok(alignments) => alignments,
                Err(message) => {
                    return Err(vec![EvalDiagnostic {
                        message,
                        range: input.range,
                    }]);
                }
            };
        let body = input.arguments.take_content("body");
        let mut cells = Vec::new();
        for node in body {
            // Source formatting between cells is not table content.
            if is_whitespace_text(&node) || is_parbreak(&node) {
                continue;
            }
            if core_local(&node.name) != "table-cell" {
                return Err(vec![EvalDiagnostic {
                    message: "table body may contain only table-cell elements".into(),
                    range: input.range,
                }]);
            }
            cells.push(node);
        }
        if cells.is_empty() {
            return Err(vec![EvalDiagnostic {
                message: "table requires at least one table-cell".into(),
                range: input.range,
            }]);
        }
        // The layout checker reads cell spans directly from the unified nodes.
        if let Err(error) = table_layout_nodes(columns as u16, &cells) {
            return Err(vec![EvalDiagnostic {
                message: table_layout_message(error, columns as u16),
                range: input.range,
            }]);
        }
        let align = alignments
            .iter()
            .map(alignment_name)
            .collect::<Vec<_>>()
            .join(",");
        let mut node = Node::block_call("core::table", input.range)
            .arg("columns", columns)
            .arg("header", header)
            .arg("align", align);
        node.children = cells;
        Ok(Value::Content(vec![node]))
    }
}

fn alignment_name(alignment: &TableAlignment) -> &'static str {
    match alignment {
        TableAlignment::Default => "default",
        TableAlignment::Left => "left",
        TableAlignment::Center => "center",
        TableAlignment::Right => "right",
    }
}

struct FigureFunction;

impl Function for FigureFunction {
    fn name(&self) -> &str {
        "figure"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::figure_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<Value, Vec<EvalDiagnostic>> {
        let body = input.arguments.take_content("body");
        let caption = input.arguments.take_optional_content("caption");
        let supplement = input.arguments.take_optional_content("supplement");
        let kind = match input.arguments.optional_string("kind") {
            Some(kind) => {
                let kind = kind.trim().to_owned();
                if kind.is_empty() {
                    return Err(vec![EvalDiagnostic {
                        message: "figure kind cannot be empty".into(),
                        range: input.range,
                    }]);
                }
                kind
            }
            None => infer_figure_kind(&body),
        };
        let mut node = Node::block_call("core::figure", input.range).arg("kind", kind);
        node.args
            .extend(optional_content_arg("supplement", supplement));
        node.args.extend(optional_content_arg("caption", caption));
        node.children = body;
        Ok(Value::Content(vec![node]))
    }
}

/// Resolves the Typst-style `kind: auto` default from the wrapped body: the
/// first meaningful block element wins; unrecognized bodies use `"figure"`.
fn infer_figure_kind(body: &[Node]) -> String {
    for node in body {
        if is_whitespace_text(node) || is_parbreak(node) {
            continue;
        }
        match core_local(&node.name) {
            "table" => return "table".into(),
            "raw" => return "raw".into(),
            _ if !node.name.starts_with("core::") => return node.name.clone(),
            _ => break,
        }
    }
    "figure".into()
}

fn table_alignments(source: Option<&str>, columns: usize) -> Result<Vec<TableAlignment>, String> {
    let Some(source) = source else {
        return Ok(vec![TableAlignment::Default; columns]);
    };
    let alignments: Result<Vec<_>, _> = source
        .split(',')
        .map(|value| match value.trim() {
            "default" | "" => Ok(TableAlignment::Default),
            "left" => Ok(TableAlignment::Left),
            "center" => Ok(TableAlignment::Center),
            "right" => Ok(TableAlignment::Right),
            value => Err(format!("unknown table alignment `{value}`")),
        })
        .collect();
    let alignments = alignments?;
    if alignments.len() != columns {
        return Err(format!(
            "table align specifies {} columns, expected {columns}",
            alignments.len()
        ));
    }
    Ok(alignments)
}

fn table_layout_message(error: TableLayoutError, columns: u16) -> String {
    match error {
        TableLayoutError::NonCell { cell } => {
            format!("table cell {cell} is not a table-cell element")
        }
        TableLayoutError::CellDoesNotFit {
            row,
            cell,
            column,
            colspan,
        } => format!(
            "table cell {cell} with colspan {colspan} does not fit row {row} at column {} of {columns}",
            column + 1
        ),
        TableLayoutError::IncompleteRow { row } => {
            format!("table row {row} does not fill all {columns} columns")
        }
        TableLayoutError::FullyCoveredRow { row } => {
            format!("table row {row} is fully covered by rowspans and cannot contain the next cell")
        }
        TableLayoutError::RowspanBeyondTable => {
            "table rowspan extends beyond the final explicit row".into()
        }
    }
}

struct RuleFunction;

impl Function for RuleFunction {
    fn name(&self) -> &str {
        "rule"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::empty_content_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<Value, Vec<EvalDiagnostic>> {
        Ok(Value::Content(vec![Node::block_call(
            "core::rule",
            input.range,
        )]))
    }
}

/// Builds a `core::raw` node from validated parts.
pub(crate) fn raw_node(
    source: String,
    block: bool,
    language: Option<String>,
    range: notist_model::TextRange,
) -> Node {
    let mut node = Node::call("core::raw", range)
        .arg("source", source)
        .arg("lang", language.map_or(NodeValue::None, NodeValue::String))
        .arg("block", block);
    node.block = block;
    node
}

#[cfg(test)]
mod tests {
    use notist_eval::{Evaluation, Evaluator, ShapingRegistry};
    use notist_model::{Node, NodeValue};

    struct CoreEvaluator {
        evaluator: Evaluator,
        shaping: ShapingRegistry,
    }

    impl CoreEvaluator {
        fn evaluate(&self, source: &str) -> Evaluation {
            self.evaluator.evaluate_with_shaping(source, &self.shaping)
        }
    }

    fn evaluator() -> CoreEvaluator {
        let (registry, shaping) = super::registry();
        CoreEvaluator {
            evaluator: Evaluator::new(registry),
            shaping,
        }
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

    fn texts(nodes: &[Node]) -> Vec<&str> {
        nodes.iter().filter_map(text).collect()
    }

    #[test]
    fn excludes_plugin_candidates_from_core_registry() {
        let evaluator = evaluator();
        for name in [
            "outline",
            "terms",
            "insert",
            "spoiler",
            "highlight",
            "samp",
            "super",
            "sub",
            "footnote",
            "comment",
            "abbr",
            "time",
            "cite",
            "video",
            "audio",
        ] {
            // Fixpoint rule: unhandled names stay as unresolved calls — the
            // check layer owns unknown-name diagnostics, not the evaluator.
            let evaluation = evaluator.evaluate(&format!("#{name}[]"));
            assert!(
                evaluation.diagnostics.is_empty(),
                "{name}: {:?}",
                evaluation.diagnostics
            );
            assert_eq!(evaluation.forest[0].name, name);
        }
    }

    #[test]
    fn evaluates_link_function_and_target_sugar_into_one_reference_node() {
        let evaluator = evaluator();
        let explicit = evaluator.evaluate("#link(<vault::guide::intro/overview>)");
        let sugar = evaluator.evaluate("#<vault::guide::intro/overview>");
        assert!(
            explicit.diagnostics.is_empty(),
            "{:?}",
            explicit.diagnostics
        );
        assert!(sugar.diagnostics.is_empty(), "{:?}", sugar.diagnostics);
        let stripped = |node: &Node| {
            let mut node = node.clone();
            node.range = notist_model::TextRange::new(0, 0);
            node
        };
        assert_eq!(stripped(&explicit.forest[0]), stripped(&sugar.forest[0]));
        assert!(explicit.forest[0].is_core("reference"));

        let invalid = evaluator.evaluate("#link(\"vault::::guide\")");
        assert!(
            invalid
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message.contains("empty segment") })
        );
    }

    #[test]
    fn evaluates_heading_and_raw() {
        let evaluator = evaluator();

        let heading = evaluator.evaluate("#heading(level=2)[Title]");
        assert!(heading.diagnostics.is_empty(), "{:?}", heading.diagnostics);
        let node = &heading.forest[0];
        assert!(node.is_core("heading"));
        assert_eq!(node.get("level"), Some(&NodeValue::Int(2)));

        let raw = evaluator.evaluate("#raw(r#\"fn main() {}\"#, lang=\"rust\")");
        assert!(raw.diagnostics.is_empty(), "{:?}", raw.diagnostics);
        let node = &raw.forest[0];
        assert!(node.is_core("raw"));
        assert_eq!(
            node.get("source"),
            Some(&NodeValue::String("fn main() {}".into()))
        );
        assert_eq!(node.get("lang"), Some(&NodeValue::String("rust".into())));
        assert_eq!(node.get("block"), Some(&NodeValue::Bool(false)));
    }

    #[test]
    fn evaluates_ordered_and_unordered_items() {
        let evaluator = evaluator();
        let unordered = evaluator.evaluate("#item[One]");
        assert!(
            unordered.diagnostics.is_empty(),
            "{:?}",
            unordered.diagnostics
        );
        let node = &unordered.forest[0];
        assert!(node.is_core("item"));
        assert_eq!(node.get("ordered"), Some(&NodeValue::Bool(false)));

        let ordered = evaluator.evaluate("#item(ordered=true)[First]");
        assert!(ordered.diagnostics.is_empty(), "{:?}", ordered.diagnostics);
        let node = &ordered.forest[0];
        assert!(node.is_core("item"));
        assert_eq!(node.get("ordered"), Some(&NodeValue::Bool(true)));
        assert_eq!(node.get("value"), None);
    }

    #[test]
    fn evaluates_callouts() {
        let evaluator = evaluator();
        let default = evaluator.evaluate("#callout[Remember this]");
        assert!(default.diagnostics.is_empty(), "{:?}", default.diagnostics);
        let node = &default.forest[0];
        assert!(node.is_core("callout"));
        assert_eq!(node.get("kind"), Some(&NodeValue::String("note".into())));
        assert!(node.get("title").is_none());
        assert_eq!(texts(&node.children), ["Remember this"]);

        let titled = evaluator.evaluate("#callout(kind=\"warning\", title=[Risk])[Body]");
        assert!(titled.diagnostics.is_empty(), "{:?}", titled.diagnostics);
        let node = &titled.forest[0];
        assert!(node.is_core("callout"));
        let Some(NodeValue::Stream(title)) = node.get("title") else {
            panic!("expected a title stream, got {:#?}", node.args)
        };
        assert_eq!(texts(title), ["Risk"]);

        let empty = evaluator.evaluate("#callout(kind=\"\")[Body]");
        assert!(
            empty
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("kind cannot be empty"))
        );
    }

    #[test]
    fn evaluates_details() {
        let evaluated =
            evaluator().evaluate("#details(summary=[More], open=true)[Hidden *content*]");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        let node = &evaluated.forest[0];
        assert!(node.is_core("details"));
        assert_eq!(node.get("open"), Some(&NodeValue::Bool(true)));
        let Some(NodeValue::Stream(summary)) = node.get("summary") else {
            panic!("expected a summary stream, got {:#?}", node.args)
        };
        assert_eq!(texts(summary), ["More"]);
        assert!(node.children.iter().any(|child| child.is_core("strong")));

        let without_summary = evaluator().evaluate("#details[Hidden]");
        assert!(
            without_summary.diagnostics.is_empty(),
            "{:?}",
            without_summary.diagnostics
        );
        let node = &without_summary.forest[0];
        assert!(node.is_core("details"));
        assert!(node.get("summary").is_none());
        assert_eq!(node.get("open"), Some(&NodeValue::Bool(false)));
    }

    #[test]
    fn evaluates_strike_function() {
        let evaluated = evaluator().evaluate("#strike[obsolete]");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        let node = &evaluated.forest[0];
        assert!(node.is_core("strike"));
        assert_eq!(texts(&node.children), ["obsolete"]);
    }

    #[test]
    fn evaluates_rule_function() {
        let evaluated = evaluator().evaluate("#rule()");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(evaluated.forest[0].is_core("rule"));
    }

    #[test]
    fn block_raw_excludes_delimiter_line_breaks() {
        let evaluated = evaluator()
            .evaluate("#raw(r#\"\"\"\nline one\nline two\n\"\"\"#, lang=\"text\", block=true)");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        let node = &evaluated.forest[0];
        assert!(node.is_core("raw"));
        assert_eq!(node.get("block"), Some(&NodeValue::Bool(true)));
        assert_eq!(
            node.get("source"),
            Some(&NodeValue::String("line one\nline two".into()))
        );

        // D0003 constructor validation: an inline raw source must not contain
        // line breaks; block: true is the opt-in for multi-line sources.
        let invalid = evaluator().evaluate("#raw(\"line one\\nline two\")");
        assert!(
            invalid
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message.contains("must not contain line breaks") }),
            "{:?} {:#?}",
            invalid.diagnostics,
            invalid.forest
        );
    }

    #[test]
    fn raw_triple_quotes_without_an_opening_line_break_stay_inline() {
        let evaluated = evaluator().evaluate(r####"#raw(r#"""quoted"""#)"####);
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        let node = &evaluated.forest[0];
        assert!(node.is_core("raw"));
        assert_eq!(node.get("block"), Some(&NodeValue::Bool(false)));
        assert_eq!(
            node.get("source"),
            Some(&NodeValue::String("\"\"quoted\"\"".into()))
        );
    }

    #[test]
    fn reports_signature_and_trailing_content_errors_before_calling_builtins() {
        let evaluator = evaluator();
        let wrong_level = evaluator.evaluate("#heading(level=\"two\")[Title]");
        assert!(
            wrong_level.diagnostics[0]
                .message
                .contains("expected Int, found String")
        );

        let wrong_body = evaluator.evaluate("#raw[parsed]");
        assert_eq!(
            wrong_body.diagnostics[0].message,
            "function `raw` does not accept trailing content"
        );

        // Unknown arguments are reported by the check layer against the
        // SignatureSet; reduction dispatch intentionally stays silent.
        let unknown = evaluator.evaluate("#details(source=\"book\")[text]");
        assert!(unknown.diagnostics.is_empty(), "{:?}", unknown.diagnostics);
    }
}
