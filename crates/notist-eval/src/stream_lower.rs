//! Direct Markup → `Node` forest lowering.
//!
//! This pass walks the parse tree and emits the unified reduction IR before
//! any function is dispatched. Explicit calls and constructor sugar become
//! call nodes; text, wiki links, raw literals, heading/rule sugar, list
//! sugar, and table sugar lower directly into `core::*` nodes. A call
//! awaiting reduction and a terminal leaf share the single [`Node`] shape —
//! the fixpoint decides which names reduce.

use std::collections::{HashMap, VecDeque};

use notist_model::{Node, NodeValue, TextRange};
use notist_syntax::{
    Call, EmbeddedExpression, ExpressionKind, Markup, MarkupItem, UserFunctionDefinition,
};

use crate::lower;
use crate::type_system::{DictKey, Value};
use crate::{AnnotationEntry, EvalDiagnostic, FunctionRegistry};

/// A materialized attribute set: canonical `key = display` pairs, ready for
/// the property table and every query consumer.
pub type MaterializedAttributes = Vec<(String, String)>;

/// The result of the lowering pass.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lowered {
    /// Lowered call forest. Calls have not been reduced yet.
    pub nodes: Vec<Node>,
    /// Diagnostics collected while lowering.
    pub diagnostics: Vec<EvalDiagnostic>,
    /// Document-level bindings observed during lowering.
    pub bindings: HashMap<String, Value>,
    /// Side annotation table.
    pub annotations: Vec<AnnotationEntry>,
    /// Module-level attributes.
    pub module_attributes: Vec<MaterializedAttributes>,
}

/// Lowers a document Markup tree into a call forest with pre-seeded root
/// bindings.
///
/// The analysis layer uses this entry point to inject imported bindings before
/// evaluation.
pub fn lower_document_with_bindings(
    source: &str,
    markup: &Markup,
    base_offset: usize,
    registry: &FunctionRegistry,
    root_bindings: HashMap<String, Value>,
) -> Lowered {
    let user_functions = collect_functions(markup);
    let mut state = LowerState {
        source,
        base_offset,
        registry,
        variables: vec![root_bindings],
        user_functions,
        nodes: Vec::new(),
        diagnostics: Vec::new(),
        annotations: Vec::new(),
        module_attributes: Vec::new(),
        pending_annotations: Vec::new(),
        pending_block_start: None,
        pending_block_end: 0,
    };
    state.lower_markup(markup);
    state.finish_annotations();
    Lowered {
        nodes: state.nodes,
        diagnostics: state.diagnostics,
        bindings: state.variables.first().cloned().unwrap_or_default(),
        annotations: state.annotations,
        module_attributes: state.module_attributes,
    }
}

/// Lowers a nested Markup body (content literal, trailing body) into a call
/// forest under an inherited environment.
///
/// Used by the expression evaluator for `Content` values: the returned forest
/// is unreduced and re-enters the reduction fixpoint at the call site.
pub fn lower_body_with_environment(
    source: &str,
    markup: &Markup,
    base_offset: usize,
    registry: &FunctionRegistry,
    user_functions: &HashMap<String, UserFunctionDefinition>,
    variables: Vec<HashMap<String, Value>>,
) -> (Vec<Node>, Vec<EvalDiagnostic>) {
    let mut state = LowerState {
        source,
        base_offset,
        registry,
        variables,
        user_functions: user_functions.clone(),
        nodes: Vec::new(),
        diagnostics: Vec::new(),
        annotations: Vec::new(),
        module_attributes: Vec::new(),
        pending_annotations: Vec::new(),
        pending_block_start: None,
        pending_block_end: 0,
    };
    state.lower_markup(markup);
    state.finish_annotations();
    (state.nodes, state.diagnostics)
}

fn collect_functions(markup: &Markup) -> HashMap<String, UserFunctionDefinition> {
    let mut functions = HashMap::new();
    lower::collect_user_functions(markup, &mut functions);
    functions
}

/// One parsed list row carried through lowering.
struct ListSugarRow {
    indent: usize,
    ordered: bool,
    body: Vec<Node>,
    range: TextRange,
}

struct LowerState<'a> {
    source: &'a str,
    base_offset: usize,
    registry: &'a FunctionRegistry,
    variables: Vec<HashMap<String, Value>>,
    user_functions: HashMap<String, UserFunctionDefinition>,
    nodes: Vec<Node>,
    diagnostics: Vec<EvalDiagnostic>,
    annotations: Vec<AnnotationEntry>,
    module_attributes: Vec<MaterializedAttributes>,
    pending_annotations: Vec<(MaterializedAttributes, TextRange)>,
    pending_block_start: Option<TextRange>,
    pending_block_end: usize,
}

impl LowerState<'_> {
    fn lower_markup(&mut self, markup: &Markup) {
        for item in &markup.items {
            match item {
                MarkupItem::Annotation(annotation) => {
                    self.lower_annotation(annotation);
                }
                MarkupItem::Embedded(embedded) => self.lower_embedded(embedded),
                MarkupItem::Heading(sugar) => {
                    let body = self.lower_markup_body(&sugar.body);
                    let mut node =
                        Node::block_call("heading", sugar.range.shifted(self.base_offset))
                            .arg("level", sugar.level as i64);
                    node.children = body;
                    self.push_node(node);
                }
                MarkupItem::Rule(range) => {
                    self.push_node(Node::block_call("rule", range.shifted(self.base_offset)));
                }
                MarkupItem::Text(text) => {
                    for node in lower_inline_text(text, self.base_offset) {
                        self.push_node(node);
                    }
                }
                MarkupItem::Raw(raw) => self.lower_raw(raw),
                MarkupItem::List(sugar) => self.lower_list_sugar(sugar),
                MarkupItem::Table(sugar) => self.lower_table_sugar(sugar),
            }
        }
    }

    fn lower_list_sugar(&mut self, sugar: &notist_syntax::ListSugar) {
        let mut rows = VecDeque::new();
        for row in &sugar.rows {
            rows.push_back(ListSugarRow {
                indent: row.indent,
                ordered: row.ordered,
                body: self.lower_markup_body(&row.body),
                range: row.range.shifted(self.base_offset),
            });
        }
        while let Some(indent) = rows.front().map(|row| row.indent) {
            let nodes = self.lower_list_rows(&mut rows, indent);
            for node in nodes {
                self.push_node(node);
            }
        }
    }

    fn lower_list_rows(&mut self, rows: &mut VecDeque<ListSugarRow>, indent: usize) -> Vec<Node> {
        let mut nodes: Vec<Node> = Vec::new();
        while let Some(front_indent) = rows.front().map(|row| row.indent) {
            if front_indent < indent {
                break;
            }
            if front_indent > indent {
                let child_indent = front_indent;
                let children = self.lower_list_rows(rows, child_indent);
                let Some(parent) = nodes.last_mut() else {
                    self.diagnostics.push(EvalDiagnostic {
                        message: "nested list item is missing its parent item".into(),
                        range: children
                            .first()
                            .map(|node| node.range)
                            .unwrap_or(TextRange::new(0, 0)),
                    });
                    break;
                };
                parent.children.extend(children);
                continue;
            }

            let row = rows.pop_front().unwrap();
            let mut node = Node::block_call("item", row.range).arg("ordered", row.ordered);
            node.children = row.body;
            nodes.push(node);
        }
        nodes
    }

    fn lower_table_sugar(&mut self, sugar: &notist_syntax::TableSugar) {
        let mut body = Vec::new();
        for cell in sugar.header.iter().chain(sugar.rows.iter().flatten()) {
            let mut node = Node::block_call("table-cell", cell.range.shifted(self.base_offset));
            node.children = self.lower_markup_body(&cell.body);
            body.push(node);
        }

        let alignments = sugar
            .alignments
            .iter()
            .map(|alignment| match alignment {
                notist_model::TableAlignment::Default => "default",
                notist_model::TableAlignment::Left => "left",
                notist_model::TableAlignment::Center => "center",
                notist_model::TableAlignment::Right => "right",
            })
            .collect::<Vec<_>>()
            .join(",");
        let mut node = Node::block_call("table", sugar.range.shifted(self.base_offset))
            .arg("columns", sugar.header.len() as i64)
            .arg("header", true)
            .arg("align", alignments);
        node.children = body;
        self.push_node(node);
    }

    fn push_node(&mut self, mut node: Node) {
        let parbreak = node.is_core("parbreak");
        let blank = node.is_core("text")
            && node.get("text").is_some_and(
                |value| matches!(value, NodeValue::String(text) if text.trim().is_empty()),
            );
        let plain_text = node.is_core("text");
        // A pending annotation binds forward to the next block, so a block
        // start with pending annotations is that annotation's target. Plain
        // text only accumulates into a paragraph: its span stays honest and
        // the annotation is carried by the entry interval instead.
        let binds_pending = !self.pending_annotations.is_empty()
            && self.pending_block_start.is_none()
            && !parbreak
            && !blank
            && !plain_text;
        let leading_start = if binds_pending {
            self.pending_annotations
                .iter()
                .map(|(_, range)| range.start)
                .min()
        } else {
            None
        };
        let inline = !node.block && !parbreak;
        // Block spans are content-honest: a block starts at its declaring
        // annotation (or its first token) and ends at the line break that
        // terminates it. Inter-block blank lines belong to no node — spans
        // are facts, and window policies (region display, point-query
        // fallbacks) compose their own coverage over them (2026-09-02
        // ruling).
        if node.block {
            if let Some(&b'\n') = self.source.as_bytes().get(node.range.end) {
                node.range.end += 1;
            }
        }
        self.track_pending_annotation(node.range, inline, parbreak, blank, plain_text);
        if let Some(leading) = leading_start {
            // Leading annotations belong to the block they declare
            // (2026-09-01 ruling): the block's span starts at the annotation,
            // so the declaring bytes are governed by what they declare.
            node.range.start = node.range.start.min(leading);
        }
        self.nodes.push(node);
    }

    fn track_pending_annotation(
        &mut self,
        range: TextRange,
        inline: bool,
        parbreak: bool,
        blank: bool,
        plain_text: bool,
    ) {
        if self.pending_annotations.is_empty() {
            return;
        }
        if self.pending_block_start.is_none() {
            if parbreak || blank {
                return;
            }
            self.pending_block_start = Some(range);
            self.pending_block_end = range.end;
            // An inline element (scope, call node) is bound immediately; only
            // plain text accumulates into a paragraph target.
            if !inline || !plain_text {
                let end = self.pending_block_end;
                self.flush_pending_annotations(end);
            }
            return;
        }
        if parbreak || !inline {
            let end = self.pending_block_end;
            self.flush_pending_annotations(end);
        } else {
            self.pending_block_end = range.end;
        }
    }

    fn flush_pending_annotations(&mut self, block_end: usize) {
        let Some(start) = self.pending_block_start.take() else {
            return;
        };
        let mut remaining = Vec::new();
        for (attributes, annotation_range) in self.pending_annotations.drain(..) {
            if annotation_range.start < start.start {
                // A leading annotation governs from its own bytes on: the
                // entry starts at the annotation, not at the block it binds to.
                self.annotations.push(AnnotationEntry {
                    range: TextRange::new(annotation_range.start, block_end),
                    attributes,
                });
            } else {
                remaining.push((attributes, annotation_range));
            }
        }
        self.pending_annotations = remaining;
    }

    fn finish_annotations(&mut self) {
        if self.pending_block_start.is_some() {
            let end = self.pending_block_end;
            self.flush_pending_annotations(end);
        }
        for (_, range) in self.pending_annotations.drain(..) {
            self.diagnostics.push(EvalDiagnostic {
                message: "annotation `@(...)` is not followed by an Item".into(),
                range,
            });
        }
    }

    fn lower_raw(&mut self, raw: &notist_syntax::RawLiteral) {
        let range = raw.range.shifted(self.base_offset);
        let source_start = raw.payload_range.start.saturating_sub(self.base_offset);
        let source_end = raw.payload_range.end.saturating_sub(self.base_offset);
        let source = self
            .source
            .get(source_start..source_end)
            .unwrap_or_default()
            .to_owned();
        let block = matches!(raw.form, notist_syntax::RawLiteralForm::Fenced);
        let language = raw
            .tag
            .as_ref()
            .map(|tag| NodeValue::String(tag.value.clone()))
            .unwrap_or(NodeValue::None);
        let mut node = Node::call("core::raw", range)
            .arg("source", source)
            .arg("lang", language)
            .arg("block", block);
        node.block = block;
        self.push_node(node);
    }

    fn lower_markup_body(&mut self, markup: &Markup) -> Vec<Node> {
        let mut nested = LowerState {
            source: self.source,
            base_offset: self.base_offset,
            registry: self.registry,
            variables: self.variables.clone(),
            user_functions: self.user_functions.clone(),
            nodes: Vec::new(),
            diagnostics: Vec::new(),
            annotations: Vec::new(),
            module_attributes: Vec::new(),
            pending_annotations: Vec::new(),
            pending_block_start: None,
            pending_block_end: 0,
        };
        nested.lower_markup(markup);
        nested.finish_annotations();
        self.diagnostics.append(&mut nested.diagnostics);
        self.annotations.append(&mut nested.annotations);
        nested.nodes
    }

    /// Evaluates an annotation payload and queues the materialized Dict on
    /// the following Item (or the module root for `@!`). There is no
    /// fallback: a payload that does not evaluate to a Dict is a diagnostic
    /// and the annotation is dropped.
    fn lower_annotation(&mut self, annotation: &notist_syntax::Annotation) {
        let (value, mut payload_diagnostics) = lower::evaluate_expression_fragment(
            self.source,
            &annotation.expression,
            self.base_offset,
            self.registry,
            0,
            &self.user_functions,
            self.variables.clone(),
        );
        self.diagnostics.append(&mut payload_diagnostics);
        let Value::Dict(entries) = value else {
            self.diagnostics.push(EvalDiagnostic {
                message: format!("annotation must evaluate to a Dict, got {}", value.ty()),
                range: annotation.range.shifted(self.base_offset),
            });
            return;
        };
        let materialized =
            materialize_attributes(&entries, self.base_offset, &mut self.diagnostics);
        if annotation.module {
            self.module_attributes.push(materialized);
        } else {
            self.pending_annotations
                .push((materialized, annotation.range.shifted(self.base_offset)));
        }
    }

    fn lower_embedded(&mut self, embedded: &EmbeddedExpression) {
        match &embedded.expression.kind {
            ExpressionKind::Let { name, value, .. } => {
                let (value, mut diagnostics) = lower::evaluate_expression_fragment(
                    self.source,
                    value,
                    self.base_offset,
                    self.registry,
                    0,
                    &self.user_functions,
                    self.variables.clone(),
                );
                self.diagnostics.append(&mut diagnostics);
                self.bind(name.value.clone(), value);
            }
            ExpressionKind::LetFunction(definition) => {
                let function = lower::user_function_value(definition, &self.variables);
                self.bind(
                    definition.name.value.clone(),
                    Value::Function(Box::new(function)),
                );
            }
            ExpressionKind::Call(call)
                if !self.variables.iter().rev().any(|scope| {
                    scope
                        .get(&call.name.value)
                        .is_some_and(|value| matches!(value, Value::Function(_)))
                }) =>
            {
                if self.registry.get(&call.name.value).is_some() {
                    self.lower_registry_call(call, embedded);
                } else {
                    // Unknown/data-only name: emit a pending call node. The
                    // reducer decides — handler dispatch or terminal leaf.
                    self.lower_unknown_call(call);
                }
            }
            ExpressionKind::Content(block) => {
                // 手动 scope `#[...]` at markup position: its product is a
                // `scope` call — the ScopeItem keeps its content as children
                // in the Item tree (model.not). Content in code position
                // (`#let x = #[...]`, branches, arguments) stays a plain
                // Item-literal forest.
                let (value, mut diagnostics) = lower::evaluate_expression_fragment(
                    self.source,
                    &embedded.expression,
                    self.base_offset,
                    self.registry,
                    0,
                    &self.user_functions,
                    self.variables.clone(),
                );
                self.diagnostics.append(&mut diagnostics);
                let forest = match value {
                    Value::Content(forest) => forest,
                    other => {
                        return self
                            .insert_value(other, embedded.scope_range.shifted(self.base_offset))
                    }
                };
                let mut node = Node::block_call("scope", block.range.shifted(self.base_offset));
                // Inline one-liner scopes join the surrounding text flow;
                // scopes spanning blocks interrupt it.
                node.block = forest
                    .iter()
                    .any(|child| child.block || child.is_core("parbreak"));
                node.children = forest;
                self.push_node(node);
            }
            _ => {
                let (value, mut diagnostics) = lower::evaluate_expression_fragment(
                    self.source,
                    &embedded.expression,
                    self.base_offset,
                    self.registry,
                    0,
                    &self.user_functions,
                    self.variables.clone(),
                );
                self.diagnostics.append(&mut diagnostics);
                self.insert_value(value, embedded.scope_range.shifted(self.base_offset));
            }
        }
    }

    /// Lowers a call whose name has no registered handler.
    ///
    /// Named arguments keep their names; unnamed scalar arguments receive
    /// synthetic positional names; content arguments fold into the body.
    fn lower_unknown_call(&mut self, call: &Call) {
        let range = call.range.shifted(self.base_offset);
        let mut node = Node::call(call.name.value.clone(), range);
        let mut body: Vec<Node> = Vec::new();
        for (positional, argument) in call.arguments.iter().enumerate() {
            match &argument.expression.kind {
                ExpressionKind::Content(block) => {
                    body.extend(self.lower_markup_body(&block.markup));
                }
                _ => {
                    let (value, mut diagnostics) = lower::evaluate_expression_fragment(
                        self.source,
                        &argument.expression,
                        self.base_offset,
                        self.registry,
                        0,
                        &self.user_functions,
                        self.variables.clone(),
                    );
                    self.diagnostics.append(&mut diagnostics);
                    let name = match &argument.name {
                        Some(name) => name.value.clone(),
                        None => format!("arg{positional}"),
                    };
                    self.push_argument(&mut node.args, name, value, range);
                }
            }
        }
        // Unhandled names are data: the trailing body stays as the node's
        // children so tooling and projection still see it.
        for block in &call.trailing {
            body.extend(self.lower_markup_body(&block.markup));
        }
        node.children = body;
        self.push_node(node);
    }

    fn lower_registry_call(&mut self, call: &Call, embedded: &EmbeddedExpression) {
        let range = embedded.scope_range.shifted(self.base_offset);
        let signature = self
            .registry
            .get(&call.name.value)
            .expect("caller checked the registry")
            .signature();
        // Positional arguments bind parameters in declaration order, including
        // parameters with defaults; only the trailing Content slot is excluded.
        let positional = signature
            .parameters
            .iter()
            .filter(|parameter| {
                Some(parameter.name.as_str()) != signature.trailing_content.as_deref()
            })
            .map(|parameter| parameter.name.clone())
            .collect::<Vec<_>>();
        let mut positional_index = 0usize;

        let mut node = Node::call(call.name.value.clone(), range);
        for argument in &call.arguments {
            match &argument.expression.kind {
                ExpressionKind::Content(block) => {
                    let name = match &argument.name {
                        Some(name) => name.value.clone(),
                        None => signature
                            .trailing_content
                            .clone()
                            .unwrap_or_else(|| "body".into()),
                    };
                    let body = self.lower_markup_body(&block.markup);
                    node.args.push((name, NodeValue::Stream(body)));
                }
                _ => {
                    let (value, mut diagnostics) = lower::evaluate_expression_fragment(
                        self.source,
                        &argument.expression,
                        self.base_offset,
                        self.registry,
                        0,
                        &self.user_functions,
                        self.variables.clone(),
                    );
                    self.diagnostics.append(&mut diagnostics);
                    let name = match &argument.name {
                        Some(name) => name.value.clone(),
                        None => {
                            let name = positional
                                .get(positional_index)
                                .cloned()
                                .unwrap_or_else(|| format!("arg{positional_index}"));
                            positional_index += 1;
                            name
                        }
                    };
                    self.push_argument(&mut node.args, name, value, range);
                }
            }
        }

        if !call.trailing.is_empty() {
            for block in &call.trailing {
                node.children.extend(self.lower_markup_body(&block.markup));
            }
        }

        self.push_node(node);
    }

    /// Appends one evaluated argument value to a pending call node.
    ///
    /// Function values cannot live on content nodes: they belong to evaluator
    /// internals, surface as a diagnostic, and the argument is dropped while
    /// the rest of the forest keeps lowering.
    fn push_argument(
        &mut self,
        args: &mut Vec<(String, NodeValue)>,
        name: String,
        value: Value,
        range: TextRange,
    ) {
        let value = match value {
            Value::Unit => NodeValue::None,
            Value::Bool(value) => NodeValue::Bool(value),
            Value::Int(value) => NodeValue::Int(value),
            Value::Float(value) => NodeValue::Float(value),
            Value::String(value) => NodeValue::String(value),
            Value::Content(forest) => NodeValue::Stream(forest),
            Value::Target(reference) => NodeValue::Target(reference),
            Value::Function(_) => {
                self.diagnostics.push(EvalDiagnostic {
                    message: "function values cannot live on content nodes".into(),
                    range,
                });
                return;
            }
            Value::Array(_) | Value::Dict(_) => {
                self.diagnostics.push(EvalDiagnostic {
                    message: "collection values cannot live on content nodes".into(),
                    range,
                });
                return;
            }
        };
        args.push((name, value));
    }

    fn bind(&mut self, name: String, value: Value) {
        if self.variables.is_empty() {
            self.variables.push(HashMap::new());
        }
        if let Some(scope) = self.variables.last_mut() {
            scope.insert(name, value);
        }
    }

    fn insert_value(&mut self, value: Value, range: TextRange) {
        match value {
            Value::Content(forest) => {
                for node in forest {
                    self.push_node(node);
                }
            }
            Value::String(text) => {
                self.push_node(Node::call("core::text", range).arg("text", text));
            }
            Value::Target(reference) => {
                self.push_node(
                    Node::call("core::reference", range)
                        .arg("target", NodeValue::Target(reference)),
                );
            }
            Value::Int(value) => self.push_text_leaf(value.to_string(), range),
            Value::Float(value) => self.push_text_leaf(value.to_string(), range),
            Value::Bool(value) => self.push_text_leaf(value.to_string(), range),
            Value::Unit => {}
            other => self.diagnostics.push(EvalDiagnostic {
                message: format!("cannot insert {} into Markup", other.ty()),
                range,
            }),
        }
    }

    fn push_text_leaf(&mut self, text: String, range: TextRange) {
        self.push_node(Node::call("core::text", range).arg("text", text));
    }
}

/// Lowers one Markup text run into inline `core::*` nodes, splitting blank
/// lines into `core::parbreak` separators and scanning the inline sugar
/// (`*strong*`, `_emph_`, `__underline__`, `~~strike~~`, escapes).
fn lower_inline_text(text: &notist_syntax::SpannedText, base_offset: usize) -> Vec<Node> {
    let mut lowerer = InlineTextLowerer {
        text,
        base_offset,
        nodes: Vec::new(),
    };
    lowerer.push_text_with_parbreaks();
    lowerer.nodes
}

struct InlineTextLowerer<'a> {
    text: &'a notist_syntax::SpannedText,
    base_offset: usize,
    nodes: Vec<Node>,
}

impl InlineTextLowerer<'_> {
    fn push_text_with_parbreaks(&mut self) {
        let bytes = self.text.value.as_bytes();
        let mut segment_start = 0usize;
        let mut cursor = 0usize;

        while cursor < bytes.len() {
            let Some(mut after) = newline_end(bytes, cursor) else {
                cursor += 1;
                continue;
            };
            let mut count = 1usize;
            while let Some(next) = newline_end(bytes, after) {
                count += 1;
                after = next;
            }
            if count == 1 {
                // A soft break is a separator within the paragraph, not
                // content: flush the segment before it and skip the newline
                // so it never reaches rendered output.
                self.push_inline_text(segment_start, cursor);
                segment_start = after;
                cursor = after;
                continue;
            }
            self.push_inline_text(segment_start, cursor);
            self.nodes
                .push(Node::call("core::parbreak", self.text_range(cursor, after)));
            segment_start = after;
            cursor = after;
        }

        self.push_inline_text(segment_start, bytes.len());
    }

    fn push_inline_text(&mut self, start: usize, end: usize) {
        let bytes = self.text.value.as_bytes();
        let mut plain_start = start;
        let mut cursor = start;
        while cursor < end {
            if bytes[cursor..end].starts_with(b"~~")
                && (cursor == start || bytes[cursor - 1] != b'\\')
                && let Some(closing) = find_unescaped_sequence(bytes, cursor + 2, end, b"~~")
                && closing > cursor + 2
            {
                self.push_plain_text(plain_start, cursor);
                let body = self.inline_content(cursor + 2, closing);
                let mut node = Node::call("core::strike", self.text_range(cursor, closing + 2));
                node.children = body;
                self.nodes.push(node);
                cursor = closing + 2;
                plain_start = cursor;
                continue;
            }

            if bytes[cursor..end].starts_with(b"__")
                && (cursor == start || bytes[cursor - 1] != b'\\')
                && let Some(closing) = find_unescaped_sequence(bytes, cursor + 2, end, b"__")
                && closing > cursor + 2
            {
                self.push_plain_text(plain_start, cursor);
                let body = self.inline_content(cursor + 2, closing);
                let mut node = Node::call("core::underline", self.text_range(cursor, closing + 2));
                node.children = body;
                self.nodes.push(node);
                cursor = closing + 2;
                plain_start = cursor;
                continue;
            }

            if bytes[cursor] == b'\\'
                && let Some(&escaped) = bytes.get(cursor + 1)
                && escaped.is_ascii_punctuation()
            {
                self.push_plain_text(plain_start, cursor);
                self.nodes.push(
                    Node::call("core::text", self.text_range(cursor, cursor + 2))
                        .arg("text", (escaped as char).to_string()),
                );
                cursor += 2;
                plain_start = cursor;
                continue;
            }

            if matches!(bytes[cursor], b'*' | b'_')
                && (cursor == start || bytes[cursor - 1] != b'\\')
                && bytes
                    .get(cursor + 1)
                    .is_some_and(|&next| !next.is_ascii_whitespace() && next != bytes[cursor])
            {
                let delimiter = bytes[cursor];
                if let Some(closing) = find_unescaped_sequence(bytes, cursor + 1, end, &[delimiter])
                    && closing > cursor + 1
                    && !bytes[closing - 1].is_ascii_whitespace()
                {
                    self.push_plain_text(plain_start, cursor);
                    let body = self.inline_content(cursor + 1, closing);
                    let name = if delimiter == b'*' {
                        "core::strong"
                    } else {
                        "core::emph"
                    };
                    let mut node = Node::call(name, self.text_range(cursor, closing + 1));
                    node.children = body;
                    self.nodes.push(node);
                    cursor = closing + 1;
                    plain_start = cursor;
                    continue;
                }
            }

            cursor += 1;
        }
        self.push_plain_text(plain_start, end);
    }

    fn push_plain_text(&mut self, start: usize, end: usize) {
        if start < end {
            self.nodes.push(
                Node::call("core::text", self.text_range(start, end))
                    .arg("text", self.text.value[start..end].to_owned()),
            );
        }
    }

    fn inline_content(&self, start: usize, end: usize) -> Vec<Node> {
        let mut nested = InlineTextLowerer {
            text: self.text,
            base_offset: self.base_offset,
            nodes: Vec::new(),
        };
        nested.push_inline_text(start, end);
        nested.nodes
    }

    fn text_range(&self, start: usize, end: usize) -> TextRange {
        TextRange::new(
            self.base_offset + self.text.range.start + start,
            self.base_offset + self.text.range.start + end,
        )
    }
}

fn newline_end(bytes: &[u8], cursor: usize) -> Option<usize> {
    match bytes.get(cursor) {
        Some(b'\r') if bytes.get(cursor + 1) == Some(&b'\n') => Some(cursor + 2),
        Some(b'\r' | b'\n') => Some(cursor + 1),
        _ => None,
    }
}

fn preceding_backslashes(bytes: &[u8], start: usize, cursor: usize) -> usize {
    bytes[start..cursor]
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\\')
        .count()
}

fn find_unescaped_sequence(
    bytes: &[u8],
    start: usize,
    end: usize,
    delimiter: &[u8],
) -> Option<usize> {
    if delimiter.is_empty() || start >= end || delimiter.len() > end - start {
        return None;
    }
    (start..=end - delimiter.len()).find(|&cursor| {
        bytes[cursor..].starts_with(delimiter)
            && preceding_backslashes(bytes, 0, cursor).is_multiple_of(2)
    })
}

/// Flattens an evaluated Dict into canonical `key = display` pairs. Values
/// display without quotes (matching the attrs surface); nested collections
/// render in source-like form.
fn materialize_attributes(
    entries: &[(DictKey, Value)],
    base_offset: usize,
    diagnostics: &mut Vec<EvalDiagnostic>,
) -> MaterializedAttributes {
    entries
        .iter()
        .map(|(key, value)| {
            (
                key.to_string(),
                value_display(value, base_offset, diagnostics),
            )
        })
        .collect()
}

fn value_display(
    value: &Value,
    base_offset: usize,
    diagnostics: &mut Vec<EvalDiagnostic>,
) -> String {
    match value {
        Value::Unit => "()".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Int(value) => value.to_string(),
        Value::Float(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(elements) => {
            let inner = elements
                .iter()
                .map(|element| value_display(element, base_offset, diagnostics))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner},)")
        }
        Value::Dict(entries) => {
            let inner = entries
                .iter()
                .map(|(key, value)| {
                    format!("{key}: {}", value_display(value, base_offset, diagnostics))
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner})")
        }
        Value::Content(_) | Value::Function(_) | Value::Target(_) => {
            diagnostics.push(EvalDiagnostic {
                message: "annotation values must be literal Dict entries".into(),
                range: TextRange::new(base_offset, base_offset),
            });
            "()".to_owned()
        }
    }
}
