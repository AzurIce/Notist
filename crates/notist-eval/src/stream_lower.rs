//! Direct Markup → `Stream<Call | Leaf>` lowering.
//!
//! This pass walks the parse tree and emits the uniform reduction IR before
//! any function is dispatched. Explicit calls and constructor sugar become
//! `StreamNode::Call`; text, wiki links, raw literals, heading/rule sugar,
//! list sugar, and table sugar are lowered directly into the reduction IR.
//! Text still uses the legacy inline-sugar parser as a leaf factory until that
//! scanner is ported onto `StreamNode::Call` wrappers.

use std::collections::{HashMap, VecDeque};

use notist_model::{ElementInstance, ElementName, FieldValue, InstanceNode, TextRange};
use notist_syntax::{
    Attributes, Call, EmbeddedExpression, ExpressionKind, Markup, MarkupItem,
    UserFunctionDefinition,
};

use crate::leaf::{FlatContent, StreamArgument, StreamCall, StreamNode, StreamValue};
use crate::lower;
use crate::type_system::Value;
use crate::{AnnotationEntry, EvalDiagnostic, FunctionRegistry};

/// The result of the Stream lowering pass.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamLowered {
    /// Lowered stream. Calls have not been reduced yet.
    pub flat: FlatContent,
    /// Diagnostics collected while lowering.
    pub diagnostics: Vec<EvalDiagnostic>,
    /// Document-level bindings observed during lowering.
    pub bindings: HashMap<String, Value>,
    /// Side annotation table.
    pub annotations: Vec<AnnotationEntry>,
    /// Module-level attributes.
    pub module_attributes: Vec<Attributes>,
}

/// Lowers a document Markup tree into `Stream<Call | Leaf>`.
/// Lowers a document Markup tree with pre-seeded root bindings.
///
/// The analysis layer uses this entry point to inject imported bindings before
/// evaluation, exactly like the legacy evaluator's seeded scope.
pub fn lower_markup_stream_with_bindings(
    source: &str,
    markup: &Markup,
    base_offset: usize,
    registry: &FunctionRegistry,
    root_bindings: HashMap<String, Value>,
) -> StreamLowered {
    let user_functions = collect_functions(markup);
    let mut state = StreamLowerState {
        source,
        base_offset,
        registry,
        variables: vec![root_bindings],
        user_functions,
        flat: FlatContent::new(),
        diagnostics: Vec::new(),
        annotations: Vec::new(),
        module_attributes: Vec::new(),
        pending_annotations: Vec::new(),
        pending_block_start: None,
        pending_block_end: 0,
    };
    state.lower_markup(markup);
    state.finish_annotations();
    StreamLowered {
        flat: state.flat,
        diagnostics: state.diagnostics,
        bindings: state.variables.first().cloned().unwrap_or_default(),
        annotations: state.annotations,
        module_attributes: state.module_attributes,
    }
}

fn collect_functions(markup: &Markup) -> HashMap<String, UserFunctionDefinition> {
    let mut functions = HashMap::new();
    lower::collect_user_functions(markup, &mut functions);
    functions
}

/// One parsed list row carried through Stream lowering.
struct ListSugarStreamRow {
    indent: usize,
    ordered: bool,
    body: FlatContent,
    range: TextRange,
}

struct StreamLowerState<'a> {
    source: &'a str,
    base_offset: usize,
    registry: &'a FunctionRegistry,
    variables: Vec<HashMap<String, Value>>,
    user_functions: HashMap<String, UserFunctionDefinition>,
    flat: FlatContent,
    diagnostics: Vec<EvalDiagnostic>,
    annotations: Vec<AnnotationEntry>,
    module_attributes: Vec<Attributes>,
    pending_annotations: Vec<(Attributes, TextRange)>,
    pending_block_start: Option<TextRange>,
    pending_block_end: usize,
}

impl StreamLowerState<'_> {
    fn lower_markup(&mut self, markup: &Markup) {
        for item in &markup.items {
            match item {
                MarkupItem::BlockAnnotation(annotation) => {
                    self.pending_annotations.push((
                        annotation.attributes.clone(),
                        annotation.range.shifted(self.base_offset),
                    ));
                }
                MarkupItem::ModuleAnnotation(annotation) => {
                    self.module_attributes.push(annotation.attributes.clone());
                }
                MarkupItem::Embedded(embedded) => self.lower_embedded(embedded),
                MarkupItem::Heading(sugar) => {
                    let body = self.lower_markup_body(&sugar.body);
                    self.push_node(StreamNode::Call(
                        StreamCall::new("heading", sugar.range.shifted(self.base_offset))
                            .argument("level", Value::Int(sugar.level as i64))
                            .with_body(body),
                    ));
                }
                MarkupItem::Rule(range) => {
                    self.push_node(StreamNode::Call(StreamCall::new(
                        "rule",
                        range.shifted(self.base_offset),
                    )));
                }
                MarkupItem::Text(text) => {
                    let (content, annotations, mut diagnostics) = lower::lower_inline_text(
                        self.source,
                        text,
                        self.base_offset,
                        self.registry,
                        &self.user_functions,
                        self.variables.clone(),
                    );
                    self.diagnostics.append(&mut diagnostics);
                    self.annotations.extend(annotations);
                    for node in crate::leaf::legacy_content_to_nodes(&content) {
                        self.push_node(StreamNode::Leaf(node));
                    }
                }
                MarkupItem::Wiki(link) => {
                    let range = link.range.shifted(self.base_offset);
                    self.push_node(StreamNode::Leaf(InstanceNode::ranged(
                        ElementInstance::new(ElementName::core("reference"), false).with_field(
                            "url",
                            FieldValue::String(crate::leaf::format_wiki_reference(&link.target)),
                        ),
                        range,
                    )));
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
            rows.push_back(ListSugarStreamRow {
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

    fn lower_list_rows(
        &mut self,
        rows: &mut VecDeque<ListSugarStreamRow>,
        indent: usize,
    ) -> Vec<StreamNode> {
        let mut nodes = Vec::new();
        while let Some(front_indent) = rows.front().map(|row| row.indent) {
            if front_indent < indent {
                break;
            }
            if front_indent > indent {
                let child_indent = front_indent;
                let children = self.lower_list_rows(rows, child_indent);
                let Some(StreamNode::Call(parent)) = nodes.last_mut() else {
                    self.diagnostics.push(EvalDiagnostic {
                        message: "nested list item is missing its parent item".into(),
                        range: children
                            .first()
                            .map(|node| match node {
                                StreamNode::Call(call) => call.range,
                                StreamNode::Leaf(leaf) => leaf.range,
                            })
                            .unwrap_or(TextRange::new(0, 0)),
                    });
                    break;
                };
                parent
                    .body
                    .get_or_insert_with(FlatContent::new)
                    .nodes
                    .extend(children);
                continue;
            }

            let row = rows.pop_front().unwrap();
            let call = StreamCall::new("item", row.range)
                .argument("ordered", Value::Bool(row.ordered))
                .with_body(row.body);
            nodes.push(StreamNode::Call(call));
        }
        nodes
    }

    fn lower_table_sugar(&mut self, sugar: &notist_syntax::TableSugar) {
        let mut body = FlatContent::new();
        for cell in sugar.header.iter().chain(sugar.rows.iter().flatten()) {
            let call = StreamCall::new("table-cell", cell.range.shifted(self.base_offset))
                .with_body(self.lower_markup_body(&cell.body));
            body.nodes.push(StreamNode::Call(call));
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
        let call = StreamCall::new("table", sugar.range.shifted(self.base_offset))
            .argument("columns", Value::Int(sugar.header.len() as i64))
            .argument("header", Value::Bool(true))
            .argument("align", Value::String(alignments))
            .with_body(body);
        self.push_node(StreamNode::Call(call));
    }

    fn push_node(&mut self, node: StreamNode) {
        let (range, inline, parbreak, blank) =
            match &node {
                StreamNode::Call(call) => {
                    let block = matches!(
                        call.name.as_str(),
                        "heading" | "rule" | "item" | "table-cell" | "table"
                    );
                    (call.range, !block, false, false)
                }
                StreamNode::Leaf(leaf) => if leaf.instance.is_core("parbreak") {
                    (leaf.range, false, true, false)
                } else if leaf.instance.is_core("text")
                    && leaf.instance.field("text").is_some_and(
                        |value| matches!(value, FieldValue::String(text) if text.trim().is_empty()),
                    )
                {
                    (leaf.range, true, false, true)
                } else {
                    (leaf.range, !leaf.instance.block, false, false)
                },
            };
        self.track_pending_annotation(range, inline, parbreak, blank);
        self.flat.nodes.push(node);
    }

    fn track_pending_annotation(
        &mut self,
        range: TextRange,
        inline: bool,
        parbreak: bool,
        blank: bool,
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
            if !inline {
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
                self.annotations.push(AnnotationEntry {
                    range: TextRange::new(start.start, block_end),
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
                message: "block annotation `@[...]` is not followed by a block".into(),
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
            .map(|tag| FieldValue::String(tag.value.clone()))
            .unwrap_or(FieldValue::None);
        let instance = ElementInstance::new(ElementName::core("raw"), block)
            .with_field("source", FieldValue::String(source))
            .with_field("lang", language)
            .with_field("block", FieldValue::Bool(block));
        self.push_node(StreamNode::Leaf(InstanceNode::ranged(instance, range)));
    }

    fn lower_markup_body(&mut self, markup: &Markup) -> FlatContent {
        let mut nested = StreamLowerState {
            source: self.source,
            base_offset: self.base_offset,
            registry: self.registry,
            variables: self.variables.clone(),
            user_functions: self.user_functions.clone(),
            flat: FlatContent::new(),
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
        nested.flat
    }

    fn lower_embedded(&mut self, embedded: &EmbeddedExpression) {
        if !embedded.attributes.items.is_empty() || embedded.attributes.id.is_some() {
            self.annotations.push(AnnotationEntry {
                range: embedded.scope_range.shifted(self.base_offset),
                attributes: embedded.attributes.clone(),
            });
        }

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
                if self.registry.get(&call.name.value).is_some()
                    && !self.variables.iter().rev().any(|scope| {
                        scope
                            .get(&call.name.value)
                            .is_some_and(|value| matches!(value, Value::Function(_)))
                    }) =>
            {
                self.lower_registry_call(call, embedded);
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

    fn lower_registry_call(&mut self, call: &Call, embedded: &EmbeddedExpression) {
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

        let mut arguments = Vec::new();
        for argument in &call.arguments {
            let value = match &argument.expression.kind {
                ExpressionKind::Content(block) => {
                    StreamValue::Stream(self.lower_markup_body(&block.markup))
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
                    StreamValue::Value(value)
                }
            };
            let name = match &argument.name {
                Some(name) => name.value.clone(),
                None if matches!(argument.expression.kind, ExpressionKind::Content(_)) => signature
                    .trailing_content
                    .clone()
                    .unwrap_or_else(|| "body".into()),
                None => {
                    let name = positional
                        .get(positional_index)
                        .cloned()
                        .unwrap_or_else(|| format!("arg{positional_index}"));
                    positional_index += 1;
                    name
                }
            };
            arguments.push(StreamArgument { name, value });
        }

        let body = if call.trailing.is_empty() {
            None
        } else {
            let mut nodes = Vec::new();
            for block in &call.trailing {
                nodes.extend(self.lower_markup_body(&block.markup).nodes);
            }
            Some(FlatContent { nodes })
        };

        self.push_node(StreamNode::Call(StreamCall {
            name: call.name.value.clone(),
            arguments,
            body,
            range: embedded.scope_range.shifted(self.base_offset),
        }));
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
            Value::Content(content) => {
                for node in crate::leaf::legacy_content_to_nodes(&content) {
                    self.push_node(StreamNode::Leaf(node));
                }
            }
            Value::String(text) => {
                self.push_node(StreamNode::Leaf(InstanceNode::ranged(
                    ElementInstance::text(text),
                    range,
                )));
            }
            Value::Int(value) => self.push_text_leaf(value.to_string(), range),
            Value::Float(value) => self.push_text_leaf(value.to_string(), range),
            Value::Bool(value) => self.push_text_leaf(value.to_string(), range),
            Value::None => {}
            other => self.diagnostics.push(EvalDiagnostic {
                message: format!("cannot insert {} into Markup", other.ty()),
                range,
            }),
        }
    }

    fn push_text_leaf(&mut self, text: String, range: TextRange) {
        self.push_node(StreamNode::Leaf(InstanceNode::ranged(
            ElementInstance::text(text),
            range,
        )));
    }
}
