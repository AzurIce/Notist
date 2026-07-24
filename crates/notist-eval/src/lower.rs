#![allow(dead_code)]

use std::collections::HashMap;

use notist_model::{
    Content, DefaultValue, Element, ElementNode, FunctionSignature, Parameter, TableAlignment,
    TextRange, Type,
};
use notist_syntax::{
    BinaryOperator, BodyForm, Call, ContentBlock, EmbeddedExpression, Expression, ExpressionKind,
    Markup, MarkupItem, RawLiteral, RawLiteralForm, UserFunctionDefinition,
};

use crate::builtin::citation_key_is_valid;
use crate::function::{FunctionContext, FunctionInput, FunctionRegistry};
use crate::type_system::{Value, ValueOrigin, bind_arguments, evaluate_literal};
use crate::{EvalDiagnostic, Evaluation};

pub(crate) fn evaluate_markup(
    source: &str,
    markup: &Markup,
    base_offset: usize,
    registry: &FunctionRegistry,
    depth: usize,
) -> Evaluation {
    let mut user_functions = HashMap::new();
    collect_user_functions(markup, &mut user_functions);
    user_functions.retain(|name, _| registry.get(name).is_none());
    evaluate_markup_in_environment(
        source,
        markup,
        base_offset,
        registry,
        depth,
        &user_functions,
        Vec::new(),
    )
}

fn evaluate_markup_in_environment(
    source: &str,
    markup: &Markup,
    base_offset: usize,
    registry: &FunctionRegistry,
    depth: usize,
    user_functions: &HashMap<String, UserFunctionDefinition>,
    variables: Vec<HashMap<String, Value>>,
) -> Evaluation {
    let mut state = LowerState {
        source,
        base_offset,
        registry,
        depth,
        user_functions,
        variables,
        content: Content::default(),
        diagnostics: Vec::new(),
    };
    state.lower_markup(markup);
    Evaluation {
        content: state.content,
        diagnostics: state.diagnostics,
    }
}

struct LowerState<'a> {
    source: &'a str,
    base_offset: usize,
    registry: &'a FunctionRegistry,
    depth: usize,
    user_functions: &'a HashMap<String, UserFunctionDefinition>,
    variables: Vec<HashMap<String, Value>>,
    content: Content,
    diagnostics: Vec<EvalDiagnostic>,
}

fn collect_user_functions(
    markup: &Markup,
    functions: &mut HashMap<String, UserFunctionDefinition>,
) {
    for item in &markup.items {
        let MarkupItem::Embedded(embedded) = item else {
            continue;
        };
        collect_expression_functions(&embedded.expression, functions);
    }
}

fn collect_expression_functions(
    expression: &Expression,
    functions: &mut HashMap<String, UserFunctionDefinition>,
) {
    match &expression.kind {
        ExpressionKind::Content(block) => collect_user_functions(&block.markup, functions),
        ExpressionKind::Call(call) => {
            for argument in &call.arguments {
                collect_expression_functions(&argument.expression, functions);
            }
            for block in &call.trailing {
                collect_user_functions(&block.markup, functions);
            }
        }
        ExpressionKind::Binary { left, right, .. } => {
            collect_expression_functions(left, functions);
            collect_expression_functions(right, functions);
        }
        ExpressionKind::LetFunction(definition) => {
            functions
                .entry(definition.name.value.clone())
                .or_insert_with(|| definition.as_ref().clone());
            collect_expression_functions(&definition.body, functions);
        }
        ExpressionKind::Parenthesized(inner) => collect_expression_functions(inner, functions),
        _ => {}
    }
}

fn signature_for_user_function(definition: &UserFunctionDefinition) -> FunctionSignature {
    let parameters = definition
        .parameters
        .iter()
        .map(|parameter| Parameter {
            name: parameter.name.value.clone(),
            ty: parameter.ty.clone(),
            default: parameter.default.as_ref().and_then(expression_default),
        })
        .collect::<Vec<_>>();
    let trailing_content = parameters
        .last()
        .filter(|parameter| parameter.ty == Type::Content)
        .map(|parameter| parameter.name.clone());
    FunctionSignature {
        parameters,
        trailing_content,
        result: definition.result.clone(),
    }
}

fn expression_default(expression: &Expression) -> Option<DefaultValue> {
    match &expression.kind {
        ExpressionKind::None => Some(DefaultValue::None),
        ExpressionKind::Bool(value) => Some(DefaultValue::Bool(*value)),
        ExpressionKind::Int(value) => Some(DefaultValue::Int(*value)),
        ExpressionKind::Float(value) => Some(DefaultValue::Float(*value)),
        ExpressionKind::String(value) => Some(DefaultValue::String(value.value.clone())),
        ExpressionKind::Parenthesized(inner) => expression_default(inner),
        _ => None,
    }
}

fn float_binary(operator: BinaryOperator, left: f64, right: f64) -> Option<f64> {
    match operator {
        BinaryOperator::Add => Some(left + right),
        BinaryOperator::Subtract => Some(left - right),
        BinaryOperator::Multiply => Some(left * right),
        BinaryOperator::Divide if right == 0.0 => None,
        BinaryOperator::Divide => Some(left / right),
    }
}

impl LowerState<'_> {
    fn lower_markup(&mut self, markup: &Markup) {
        let table_spans = table_source_spans(self.source, markup);
        let heading_spans = heading_source_spans(self.source, markup, &table_spans);
        let list_spans = list_source_spans(self.source, markup, &table_spans);
        if !table_spans.is_empty() || !heading_spans.is_empty() || !list_spans.is_empty() {
            let mut source_spans = table_spans
                .iter()
                .map(|span| (span.start, span.end, SourceSugarKind::Table))
                .chain(
                    heading_spans
                        .iter()
                        .map(|span| (span.start, span.end, SourceSugarKind::Heading)),
                )
                .chain(
                    list_spans
                        .iter()
                        .map(|span| (span.start, span.end, span.kind)),
                )
                .collect::<Vec<_>>();
            source_spans.sort_by_key(|(start, _, _)| *start);
            let mut cursor = markup.range.start;
            for (start, end, kind) in source_spans {
                self.lower_markup_fragment(cursor, start);
                let lowered = match kind {
                    SourceSugarKind::Table => self.lower_table_source(start, end),
                    SourceSugarKind::Heading => self.lower_heading_source(start, end),
                    SourceSugarKind::List => self.lower_list_sugar(start, end),
                    SourceSugarKind::Task => self.lower_task_sugar(start, end),
                };
                if !lowered {
                    self.lower_markup_fragment(start, end);
                }
                cursor = end;
            }
            self.lower_markup_fragment(cursor, markup.range.end);
            return;
        }

        for item in &markup.items {
            match item {
                MarkupItem::Text(text) => self.push_text_with_parbreaks(text),
                MarkupItem::Wiki(link) => {
                    self.push_element(
                        Element::Reference(link.target.clone()),
                        link.range.shifted(self.base_offset),
                    );
                }
                MarkupItem::Raw(raw) => self.lower_raw(raw),
                MarkupItem::Embedded(embedded) => self.lower_embedded(embedded),
            }
        }
    }

    fn lower_markup_fragment(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }
        let source = &self.source[start..end];
        let parse = notist_syntax::parse(source);
        let evaluation = evaluate_markup_in_environment(
            source,
            &parse.root,
            self.base_offset + start,
            self.registry,
            self.depth,
            self.user_functions,
            self.variables.clone(),
        );
        self.content.elements.extend(evaluation.content.elements);
        self.diagnostics.extend(evaluation.diagnostics);
    }

    fn lower_embedded(&mut self, embedded: &EmbeddedExpression) {
        let (value, _, mut diagnostics) =
            self.evaluate_expression(&embedded.expression, embedded.scope_range);
        self.diagnostics.append(&mut diagnostics);
        match value {
            Value::Content(content) => {
                self.content.elements.extend(content.elements);
            }
            Value::String(text) => {
                self.push_element(
                    Element::Text(text),
                    embedded.scope_range.shifted(self.base_offset),
                );
            }
            Value::None => {}
            other => {
                self.diagnostics.push(EvalDiagnostic {
                    message: format!("cannot insert {} into Markup", other.ty()),
                    range: embedded.expression.range.shifted(self.base_offset),
                });
            }
        }
    }

    fn lower_raw(&mut self, raw: &RawLiteral) {
        let payload = &self.source[raw.payload_range.start..raw.payload_range.end];
        let block = raw.form == RawLiteralForm::Fenced;
        let language = raw
            .tag
            .as_ref()
            .map(|tag| tag.value.clone())
            .filter(|v| !v.is_empty());
        self.push_element(
            Element::Raw {
                text: payload.to_owned(),
                block,
                language,
            },
            raw.range.shifted(self.base_offset),
        );
    }

    fn evaluate_expression(
        &mut self,
        expression: &Expression,
        expression_range: TextRange,
    ) -> (Value, ValueOrigin, Vec<EvalDiagnostic>) {
        if let Some((value, origin)) = evaluate_literal(expression, self.base_offset) {
            return (value, origin, Vec::new());
        }
        match &expression.kind {
            ExpressionKind::Content(block) => {
                let (value, diagnostics) = self.evaluate_content_block(block);
                (
                    value,
                    ValueOrigin::ContentLiteral {
                        range: block.range.shifted(self.base_offset),
                    },
                    diagnostics,
                )
            }
            ExpressionKind::Call(call) => {
                let (value, diagnostics) = self.evaluate_call(call, expression_range);
                (value, ValueOrigin::Default, diagnostics)
            }
            ExpressionKind::Name(name) => {
                let value = self
                    .variables
                    .iter()
                    .rev()
                    .find_map(|scope| scope.get(&name.value))
                    .cloned()
                    .or_else(|| {
                        (self.user_functions.contains_key(&name.value)
                            || self.registry.get(&name.value).is_some())
                        .then(|| Value::Function(name.value.clone()))
                    });
                match value {
                    Some(value) => (value, ValueOrigin::Default, Vec::new()),
                    None => (
                        Value::None,
                        ValueOrigin::Default,
                        vec![EvalDiagnostic {
                            message: format!("unresolved name `{}`", name.value),
                            range: name.range.shifted(self.base_offset),
                        }],
                    ),
                }
            }
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                let (value, diagnostics) =
                    self.evaluate_binary(*operator, left, right, expression_range);
                (value, ValueOrigin::Default, diagnostics)
            }
            ExpressionKind::LetFunction(_) => (Value::None, ValueOrigin::Default, Vec::new()),
            ExpressionKind::Parenthesized(inner) => self.evaluate_expression(inner, inner.range),
            ExpressionKind::Error => (Value::None, ValueOrigin::Default, Vec::new()),
            ExpressionKind::None
            | ExpressionKind::Bool(_)
            | ExpressionKind::Int(_)
            | ExpressionKind::Float(_)
            | ExpressionKind::String(_) => {
                unreachable!("literal expressions are evaluated before dispatch")
            }
        }
    }

    fn evaluate_binary(
        &mut self,
        operator: BinaryOperator,
        left: &Expression,
        right: &Expression,
        range: TextRange,
    ) -> (Value, Vec<EvalDiagnostic>) {
        let (left, _, mut diagnostics) = self.evaluate_expression(left, left.range);
        let (right, _, mut right_diagnostics) = self.evaluate_expression(right, right.range);
        diagnostics.append(&mut right_diagnostics);
        if !diagnostics.is_empty() {
            return (Value::None, diagnostics);
        }

        let value = match (operator, left, right) {
            (BinaryOperator::Add, Value::String(left), Value::String(right)) => {
                Value::String(left + &right)
            }
            (BinaryOperator::Add, Value::Int(left), Value::Int(right)) => {
                let Some(value) = left.checked_add(right) else {
                    return self.arithmetic_overflow(range, diagnostics);
                };
                Value::Int(value)
            }
            (BinaryOperator::Subtract, Value::Int(left), Value::Int(right)) => {
                let Some(value) = left.checked_sub(right) else {
                    return self.arithmetic_overflow(range, diagnostics);
                };
                Value::Int(value)
            }
            (BinaryOperator::Multiply, Value::Int(left), Value::Int(right)) => {
                let Some(value) = left.checked_mul(right) else {
                    return self.arithmetic_overflow(range, diagnostics);
                };
                Value::Int(value)
            }
            (BinaryOperator::Divide, Value::Int(_), Value::Int(0)) => {
                return self.division_by_zero(range, diagnostics);
            }
            (BinaryOperator::Divide, Value::Int(left), Value::Int(right)) => {
                let Some(value) = left.checked_div(right) else {
                    return self.arithmetic_overflow(range, diagnostics);
                };
                Value::Int(value)
            }
            (operator, Value::Int(left), Value::Float(right)) => {
                match float_binary(operator, left as f64, right) {
                    Some(value) => Value::Float(value),
                    None => return self.division_by_zero(range, diagnostics),
                }
            }
            (operator, Value::Float(left), Value::Int(right)) => {
                match float_binary(operator, left, right as f64) {
                    Some(value) => Value::Float(value),
                    None => return self.division_by_zero(range, diagnostics),
                }
            }
            (operator, Value::Float(left), Value::Float(right)) => {
                match float_binary(operator, left, right) {
                    Some(value) => Value::Float(value),
                    None => return self.division_by_zero(range, diagnostics),
                }
            }
            (_, left, right) => {
                diagnostics.push(EvalDiagnostic {
                    message: format!(
                        "operator {operator:?} does not accept {} and {}",
                        left.ty(),
                        right.ty()
                    ),
                    range: range.shifted(self.base_offset),
                });
                return (Value::None, diagnostics);
            }
        };
        (value, diagnostics)
    }

    fn division_by_zero(
        &self,
        range: TextRange,
        mut diagnostics: Vec<EvalDiagnostic>,
    ) -> (Value, Vec<EvalDiagnostic>) {
        diagnostics.push(EvalDiagnostic {
            message: "division by zero".into(),
            range: range.shifted(self.base_offset),
        });
        (Value::None, diagnostics)
    }

    fn arithmetic_overflow(
        &self,
        range: TextRange,
        mut diagnostics: Vec<EvalDiagnostic>,
    ) -> (Value, Vec<EvalDiagnostic>) {
        diagnostics.push(EvalDiagnostic {
            message: "integer arithmetic overflow".into(),
            range: range.shifted(self.base_offset),
        });
        (Value::None, diagnostics)
    }

    fn evaluate_content_block(&mut self, block: &ContentBlock) -> (Value, Vec<EvalDiagnostic>) {
        let evaluation = evaluate_markup_in_environment(
            self.source,
            &block.markup,
            self.base_offset,
            self.registry,
            self.depth,
            self.user_functions,
            self.variables.clone(),
        );
        let diagnostics = evaluation.diagnostics;
        (Value::Content(evaluation.content), diagnostics)
    }

    fn evaluate_call(
        &mut self,
        call: &Call,
        site_range: TextRange,
    ) -> (Value, Vec<EvalDiagnostic>) {
        let mut diagnostics = Vec::new();
        let name = &call.name.value;

        if let Some(definition) = self.user_functions.get(name).cloned() {
            return self.evaluate_user_function(&definition, call, site_range);
        }

        let Some(function) = self.registry.get(name) else {
            self.diagnostics.push(EvalDiagnostic {
                message: format!("unknown function `{name}`"),
                range: call.name.range.shifted(self.base_offset),
            });
            let (trailing, mut trailing_diagnostics) = self.evaluate_trailing(&call.trailing);
            diagnostics.append(&mut trailing_diagnostics);
            let arguments = call
                .arguments_range
                .map(|range| self.source[range.start..range.end].to_owned());
            let block = call
                .trailing
                .iter()
                .any(|block| block.form == BodyForm::Block);
            let content = Content::single(
                Element::UnresolvedCall {
                    name: name.clone(),
                    arguments,
                    trailing: trailing.into_iter().map(|(content, _)| content).next(),
                    block,
                },
                site_range.shifted(self.base_offset),
            );
            return (Value::Content(content), diagnostics);
        };

        let signature = function.signature();
        let (trailing_content, mut trailing_diagnostics) = self.evaluate_trailing(&call.trailing);
        diagnostics.append(&mut trailing_diagnostics);

        let bound = match bind_arguments(
            &signature,
            &call.arguments,
            trailing_content,
            call.name.range,
            self.base_offset,
            |expression| {
                let (value, origin, diagnostics) =
                    self.evaluate_expression(expression, expression.range);
                if diagnostics.is_empty() {
                    Ok((value, origin))
                } else {
                    Err(diagnostics)
                }
            },
        ) {
            Ok(bound) => bound,
            Err(mut errors) => {
                diagnostics.append(&mut errors);
                return (Value::None, diagnostics);
            }
        };

        let context = FunctionContext {
            registry: self.registry,
            depth: self.depth,
        };
        let input = FunctionInput {
            name,
            arguments: bound,
            range: site_range.shifted(self.base_offset),
        };
        match function.call(&context, input) {
            Ok(output) if signature.result.accepts(&output.value.ty()) => {
                (output.value, diagnostics)
            }
            Ok(output) => {
                diagnostics.push(EvalDiagnostic {
                    message: format!(
                        "function `{name}` returned {}, expected {}",
                        output.value.ty(),
                        signature.result
                    ),
                    range: site_range.shifted(self.base_offset),
                });
                (Value::None, diagnostics)
            }
            Err(mut errors) => {
                diagnostics.append(&mut errors);
                (Value::None, diagnostics)
            }
        }
    }

    fn evaluate_user_function(
        &mut self,
        definition: &UserFunctionDefinition,
        call: &Call,
        site_range: TextRange,
    ) -> (Value, Vec<EvalDiagnostic>) {
        let mut diagnostics = Vec::new();
        if self.depth >= 64 {
            diagnostics.push(EvalDiagnostic {
                message: format!(
                    "function `{}` exceeded the evaluation depth limit",
                    definition.name.value
                ),
                range: site_range.shifted(self.base_offset),
            });
            return (Value::None, diagnostics);
        }

        let signature = signature_for_user_function(definition);
        let (trailing_content, mut trailing_diagnostics) = self.evaluate_trailing(&call.trailing);
        diagnostics.append(&mut trailing_diagnostics);
        let bound = match bind_arguments(
            &signature,
            &call.arguments,
            trailing_content,
            call.name.range,
            self.base_offset,
            |expression| {
                let (value, origin, diagnostics) =
                    self.evaluate_expression(expression, expression.range);
                if diagnostics.is_empty() {
                    Ok((value, origin))
                } else {
                    Err(diagnostics)
                }
            },
        ) {
            Ok(bound) => bound,
            Err(mut errors) => {
                diagnostics.append(&mut errors);
                return (Value::None, diagnostics);
            }
        };

        self.variables.push(bound.into_values());
        self.depth += 1;
        let (value, _, mut body_diagnostics) =
            self.evaluate_expression(&definition.body, definition.body.range);
        self.depth -= 1;
        self.variables.pop();
        diagnostics.append(&mut body_diagnostics);
        if !signature.result.accepts(&value.ty()) {
            diagnostics.push(EvalDiagnostic {
                message: format!(
                    "function `{}` returned {}, expected {}",
                    definition.name.value,
                    value.ty(),
                    signature.result
                ),
                range: site_range.shifted(self.base_offset),
            });
            return (Value::None, diagnostics);
        }
        (value, diagnostics)
    }

    fn evaluate_trailing(
        &mut self,
        trailing: &[ContentBlock],
    ) -> (Vec<(Content, TextRange)>, Vec<EvalDiagnostic>) {
        let mut result = Vec::new();
        let mut diagnostics = Vec::new();
        for block in trailing {
            let evaluation = evaluate_markup_in_environment(
                self.source,
                &block.markup,
                self.base_offset,
                self.registry,
                self.depth + 1,
                self.user_functions,
                self.variables.clone(),
            );
            diagnostics.extend(evaluation.diagnostics);
            let range = block.payload_range.shifted(self.base_offset);
            result.push((evaluation.content, range));
        }
        (result, diagnostics)
    }

    fn push_element(&mut self, element: Element, range: TextRange) {
        self.content.elements.push(ElementNode { element, range });
    }

    fn lower_abbr_sugar(&mut self, text: &notist_syntax::SpannedText) -> bool {
        let line = text.value.trim_end_matches(['\r', '\n']).trim();
        let Some(rest) = line.strip_prefix("*[") else {
            return false;
        };
        let Some((term, expansion)) = rest.split_once("]: ") else {
            return false;
        };
        let term = term.trim();
        let expansion = expansion.trim();
        if term.is_empty() || expansion.is_empty() {
            return false;
        }
        self.push_element(
            Element::Abbr {
                term: term.to_owned(),
                expansion: expansion.to_owned(),
            },
            self.text_range(text, 0, line.len()),
        );
        true
    }

    fn lower_list_sugar(&mut self, source_start: usize, source_end: usize) -> bool {
        struct ListRow {
            indent: usize,
            ordered: bool,
            value: Option<u32>,
            body: Content,
            range: TextRange,
        }

        fn nested_items(
            rows: &mut std::collections::VecDeque<ListRow>,
            indent: usize,
        ) -> Option<Vec<ElementNode>> {
            let mut items: Vec<ElementNode> = Vec::new();
            while let Some(row) = rows.front() {
                if row.indent < indent {
                    break;
                }
                if row.indent > indent {
                    let child_indent = row.indent;
                    let children = nested_items(rows, child_indent)?;
                    let parent = items.last_mut()?;
                    let body = match &mut parent.element {
                        Element::ListItem(body) | Element::EnumItem { body, .. } => body,
                        _ => return None,
                    };
                    body.elements.extend(children);
                    continue;
                }

                let row = rows.pop_front().unwrap();
                items.push(ElementNode {
                    element: if row.ordered {
                        Element::EnumItem {
                            value: row.value,
                            body: row.body,
                        }
                    } else {
                        Element::ListItem(row.body)
                    },
                    range: row.range,
                });
            }
            Some(items)
        }

        let mut rows = std::collections::VecDeque::new();
        let mut offset = 0usize;
        let source = &self.source[source_start..source_end];
        for line in source.split_inclusive('\n') {
            let line_without_newline = line.trim_end_matches(['\r', '\n']);
            if line_without_newline.trim().is_empty() {
                return false;
            }
            let trimmed = line_without_newline.trim_start_matches([' ', '\t']);
            let (ordered, value, marker_len, body) = if let Some(body) = trimmed.strip_prefix("- ")
            {
                (false, None, 2, body)
            } else if let Some(body) = trimmed.strip_prefix("+ ") {
                (true, None, 2, body)
            } else {
                return false;
            };
            let indent = line_without_newline.len() - trimmed.len();
            let body_start = offset + indent + marker_len;
            let body = body.trim_end_matches([' ', '\t']);
            if body.is_empty() {
                return false;
            }
            let content = self.inline_source_fragment(
                source_start + body_start,
                source_start + body_start + body.len(),
            );
            let item_range = TextRange::new(
                self.base_offset + source_start + offset,
                self.base_offset + source_start + offset + line_without_newline.len(),
            );
            rows.push_back(ListRow {
                indent,
                ordered,
                value,
                body: content,
                range: item_range,
            });
            offset += line.len();
        }
        let Some(base_indent) = rows.front().map(|row| row.indent) else {
            return false;
        };
        let Some(items) = nested_items(&mut rows, base_indent) else {
            return false;
        };
        if !rows.is_empty() {
            return false;
        }
        self.content.elements.extend(items);
        true
    }

    fn lower_terms_sugar(&mut self, text: &notist_syntax::SpannedText) -> bool {
        struct TermRow {
            indent: usize,
            term: Content,
            description: Content,
            range: TextRange,
        }

        fn nested_terms(
            rows: &mut std::collections::VecDeque<TermRow>,
            indent: usize,
        ) -> Option<Vec<ElementNode>> {
            let mut items: Vec<ElementNode> = Vec::new();
            while let Some(row) = rows.front() {
                if row.indent < indent {
                    break;
                }
                if row.indent > indent {
                    let child_indent = row.indent;
                    let children = nested_terms(rows, child_indent)?;
                    let parent = items.last_mut()?;
                    let Element::TermItem { description, .. } = &mut parent.element else {
                        return None;
                    };
                    description.elements.extend(children);
                    continue;
                }
                let row = rows.pop_front().unwrap();
                items.push(ElementNode {
                    element: Element::TermItem {
                        term: row.term,
                        description: row.description,
                    },
                    range: row.range,
                });
            }
            Some(items)
        }

        let mut rows = std::collections::VecDeque::new();
        let mut offset = 0usize;
        for line in text.value.split_inclusive('\n') {
            let line_without_newline = line.trim_end_matches(['\r', '\n']);
            let trimmed = line_without_newline.trim_start_matches([' ', '\t']);
            let Some(body) = trimmed.strip_prefix("/ ") else {
                return false;
            };
            let Some(separator) = body.find(": ") else {
                return false;
            };
            let term = body[..separator].trim();
            let description = body[separator + 2..].trim();
            if term.is_empty() || description.is_empty() {
                return false;
            }
            let indent = line_without_newline.len() - trimmed.len();
            let term_leading = body[..separator].len() - body[..separator].trim_start().len();
            let description_source = &body[separator + 2..];
            let description_leading =
                description_source.len() - description_source.trim_start().len();
            let term_start = offset + indent + 2 + term_leading;
            let description_start = offset + indent + 2 + separator + 2 + description_leading;
            let range = self.text_range(text, offset, offset + line_without_newline.len());
            rows.push_back(TermRow {
                indent,
                term: self.inline_content(text, term_start, term_start + term.len()),
                description: self.inline_content(
                    text,
                    description_start,
                    description_start + description.len(),
                ),
                range,
            });
            offset += line.len();
        }
        let Some(base_indent) = rows.front().map(|row| row.indent) else {
            return false;
        };
        let Some(items) = nested_terms(&mut rows, base_indent) else {
            return false;
        };
        if !rows.is_empty() {
            return false;
        }
        self.content.elements.extend(items);
        true
    }

    fn lower_task_sugar(&mut self, source_start: usize, source_end: usize) -> bool {
        struct TaskRow {
            indent: usize,
            checked: bool,
            body: Content,
            range: TextRange,
        }

        fn nested_tasks(
            rows: &mut std::collections::VecDeque<TaskRow>,
            indent: usize,
        ) -> Option<Vec<ElementNode>> {
            let mut items: Vec<ElementNode> = Vec::new();
            while let Some(row) = rows.front() {
                if row.indent < indent {
                    break;
                }
                if row.indent > indent {
                    let child_indent = row.indent;
                    let children = nested_tasks(rows, child_indent)?;
                    let parent = items.last_mut()?;
                    let Element::TaskItem { body, .. } = &mut parent.element else {
                        return None;
                    };
                    body.elements.extend(children);
                    continue;
                }
                let row = rows.pop_front().unwrap();
                items.push(ElementNode {
                    element: Element::TaskItem {
                        checked: row.checked,
                        body: row.body,
                    },
                    range: row.range,
                });
            }
            Some(items)
        }

        let mut rows = std::collections::VecDeque::new();
        let mut offset = 0usize;
        let source = &self.source[source_start..source_end];
        for line in source.split_inclusive('\n') {
            let line_without_newline = line.trim_end_matches(['\r', '\n']);
            let trimmed = line_without_newline.trim_start_matches([' ', '\t']);
            let (checked, body) = if let Some(body) = trimmed.strip_prefix("- [ ] ") {
                (false, body)
            } else if let Some(body) = trimmed.strip_prefix("- [x] ") {
                (true, body)
            } else if let Some(body) = trimmed.strip_prefix("- [X] ") {
                (true, body)
            } else {
                return false;
            };
            let body = body.trim_end();
            if body.is_empty() {
                return false;
            }
            let indent = line_without_newline.len() - trimmed.len();
            let body_start = offset + indent + 6;
            let range = TextRange::new(
                self.base_offset + source_start + offset,
                self.base_offset + source_start + offset + line_without_newline.len(),
            );
            rows.push_back(TaskRow {
                indent,
                checked,
                body: self.inline_source_fragment(
                    source_start + body_start,
                    source_start + body_start + body.len(),
                ),
                range,
            });
            offset += line.len();
        }
        let Some(base_indent) = rows.front().map(|row| row.indent) else {
            return false;
        };
        let Some(items) = nested_tasks(&mut rows, base_indent) else {
            return false;
        };
        if !rows.is_empty() {
            return false;
        }
        self.content.elements.extend(items);
        true
    }

    fn lower_heading_source(&mut self, source_start: usize, source_end: usize) -> bool {
        let line = self.source[source_start..source_end].trim_end_matches(['\r', '\n']);
        let trimmed = line.trim_start_matches([' ', '\t']);
        let level = trimmed.bytes().take_while(|byte| *byte == b'=').count();
        if !(1..=6).contains(&level) || trimmed.as_bytes().get(level) != Some(&b' ') {
            return false;
        }
        let body_source = &trimmed[level + 1..];
        let body = body_source.trim();
        if body.is_empty() {
            return false;
        }
        let indent = line.len() - trimmed.len();
        let leading = body_source.len() - body_source.trim_start().len();
        let body_start = source_start + indent + level + 1 + leading;
        let body_end = body_start + body.len();
        let body = self.inline_source_fragment(body_start, body_end);
        self.push_element(
            Element::Heading {
                level: level as u8,
                body,
            },
            TextRange::new(
                self.base_offset + source_start,
                self.base_offset + source_start + line.len(),
            ),
        );
        true
    }

    fn lower_table_source(&mut self, source_start: usize, source_end: usize) -> bool {
        let source = &self.source[source_start..source_end];
        let mut rows = Vec::new();
        let mut columns = None;
        let mut caption_range = None;
        let mut offset = 0usize;
        let mut lines = source.split_inclusive('\n').peekable();
        while let Some(line) = lines.next() {
            let line_without_newline = line.trim_end_matches(['\r', '\n']);
            let trimmed = line_without_newline.trim();
            if lines.peek().is_none() {
                let caption_trimmed = line_without_newline.trim_start_matches([' ', '\t']);
                if let Some(caption_source) = caption_trimmed.strip_prefix(": ") {
                    let caption = caption_source.trim();
                    if caption.is_empty() {
                        return false;
                    }
                    let indent = line_without_newline.len() - caption_trimmed.len();
                    let leading = caption_source.len() - caption_source.trim_start().len();
                    caption_range = Some((offset + indent + 2 + leading, caption.len()));
                    offset += line.len();
                    continue;
                }
            }
            if trimmed.is_empty() || !trimmed.starts_with('|') || !trimmed.ends_with('|') {
                return false;
            }
            let content_start = line_without_newline.find('|').unwrap() + 1;
            let content_end = line_without_newline.rfind('|').unwrap();
            let parts = table_cell_ranges(line_without_newline, content_start, content_end);
            if parts.is_empty() {
                return false;
            }
            if let Some(expected) = columns {
                if expected != parts.len() {
                    return false;
                }
            } else {
                columns = Some(parts.len());
            }
            let mut row = Vec::new();
            for (part_start, part_end) in parts {
                let part = &line_without_newline[part_start..part_end];
                let leading = part.len() - part.trim_start_matches([' ', '\t']).len();
                let value = part.trim();
                let start = offset + part_start + leading;
                row.push((start, value.len()));
            }
            rows.push(row);
            offset += line.len();
        }
        let Some(columns) = columns else { return false };
        if columns > u16::MAX as usize || rows.is_empty() {
            return false;
        }
        let header_alignments = (rows.len() >= 2).then(|| {
            rows[1]
                .iter()
                .map(|(start, len)| table_separator_alignment(&source[*start..*start + *len]))
                .collect::<Option<Vec<_>>>()
        });
        let header = matches!(header_alignments, Some(Some(_)));
        let alignments = header_alignments
            .flatten()
            .unwrap_or_else(|| vec![TableAlignment::Default; columns]);
        if header {
            rows.remove(1);
        }
        let mut cells = Vec::new();
        for row in rows {
            for (start, len) in row {
                let range = TextRange::new(
                    self.base_offset + source_start + start,
                    self.base_offset + source_start + start + len,
                );
                cells.push(ElementNode {
                    element: Element::TableCell {
                        body: self.inline_source_fragment(
                            source_start + start,
                            source_start + start + len,
                        ),
                        colspan: 1,
                        rowspan: 1,
                    },
                    range,
                });
            }
        }
        let caption = caption_range.map(|(start, len)| {
            self.inline_source_fragment(source_start + start, source_start + start + len)
        });
        self.push_element(
            Element::Table {
                columns: columns as u16,
                header,
                alignments,
                caption,
                cells,
            },
            TextRange::new(
                self.base_offset + source_start,
                self.base_offset + source_end,
            ),
        );
        true
    }

    fn inline_source_fragment(&mut self, start: usize, end: usize) -> Content {
        let source = &self.source[start..end];
        let parse = notist_syntax::parse(source);
        let evaluation = evaluate_markup_in_environment(
            source,
            &parse.root,
            self.base_offset + start,
            self.registry,
            self.depth + 1,
            self.user_functions,
            self.variables.clone(),
        );
        self.diagnostics.extend(evaluation.diagnostics);
        evaluation.content
    }

    fn push_text_with_parbreaks(&mut self, text: &notist_syntax::SpannedText) {
        let bytes = text.value.as_bytes();
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
                cursor = after;
                continue;
            }
            self.push_inline_text(text, segment_start, cursor);
            self.push_element(Element::Parbreak, self.text_range(text, cursor, after));
            segment_start = after;
            cursor = after;
        }

        self.push_inline_text(text, segment_start, bytes.len());
    }

    fn push_inline_text(&mut self, text: &notist_syntax::SpannedText, start: usize, end: usize) {
        let bytes = text.value.as_bytes();
        let mut plain_start = start;
        let mut cursor = start;
        while cursor < end {
            if bytes[cursor..end].starts_with(b"$$")
                && (cursor == start || bytes[cursor - 1] != b'\\')
                && let Some(closing) = find_unescaped_sequence(bytes, cursor + 2, end, b"$$")
                && closing > cursor + 2
            {
                self.push_plain_text(text, plain_start, cursor);
                self.push_element(
                    Element::Math {
                        text: text.value[cursor + 2..closing].trim().to_owned(),
                        block: true,
                    },
                    self.text_range(text, cursor, closing + 2),
                );
                cursor = closing + 2;
                plain_start = cursor;
                continue;
            }
            if bytes[cursor] == b'$'
                && (cursor == start || bytes[cursor - 1] != b'\\')
                && let Some(closing) = find_unescaped_sequence(bytes, cursor + 1, end, b"$")
                && closing > cursor + 1
            {
                self.push_plain_text(text, plain_start, cursor);
                self.push_element(
                    Element::Math {
                        text: text.value[cursor + 1..closing].to_owned(),
                        block: false,
                    },
                    self.text_range(text, cursor, closing + 1),
                );
                cursor = closing + 1;
                plain_start = cursor;
                continue;
            }

            if bytes[cursor..end].starts_with(b"~~")
                && (cursor == start || bytes[cursor - 1] != b'\\')
                && let Some(closing) = find_unescaped_sequence(bytes, cursor + 2, end, b"~~")
                && closing > cursor + 2
            {
                self.push_plain_text(text, plain_start, cursor);
                let body = self.inline_content(text, cursor + 2, closing);
                self.push_element(
                    Element::Strike(body),
                    self.text_range(text, cursor, closing + 2),
                );
                cursor = closing + 2;
                plain_start = cursor;
                continue;
            }

            let paired = if bytes[cursor..end].starts_with(b"__") {
                Some((
                    b"__".as_slice(),
                    Element::Underline as fn(Content) -> Element,
                ))
            } else {
                None
            };
            if let Some((delimiter, element)) = paired
                && (cursor == start || bytes[cursor - 1] != b'\\')
                && let Some(closing) =
                    find_unescaped_sequence(bytes, cursor + delimiter.len(), end, delimiter)
                && closing > cursor + delimiter.len()
            {
                self.push_plain_text(text, plain_start, cursor);
                let body = self.inline_content(text, cursor + delimiter.len(), closing);
                self.push_element(
                    element(body),
                    self.text_range(text, cursor, closing + delimiter.len()),
                );
                cursor = closing + delimiter.len();
                plain_start = cursor;
                continue;
            }

            if bytes[cursor] == b'\\'
                && newline_end(bytes, cursor + 1).is_some_and(|after| after <= end)
            {
                self.push_plain_text(text, plain_start, cursor);
                let after = newline_end(bytes, cursor + 1).unwrap();
                self.push_element(Element::Linebreak, self.text_range(text, cursor, after));
                cursor = after;
                plain_start = cursor;
                continue;
            }

            if bytes[cursor] == b'\\'
                && let Some(&escaped) = bytes.get(cursor + 1)
                && escaped.is_ascii_punctuation()
            {
                self.push_plain_text(text, plain_start, cursor);
                self.push_element(
                    Element::Text((escaped as char).to_string()),
                    self.text_range(text, cursor, cursor + 2),
                );
                cursor += 2;
                plain_start = cursor;
                continue;
            }

            if matches!(bytes[cursor], b'*' | b'_')
                && (cursor == start || bytes[cursor - 1] != b'\\')
            {
                let delimiter = bytes[cursor];
                if let Some(closing) = find_unescaped_sequence(bytes, cursor + 1, end, &[delimiter])
                    && closing > cursor + 1
                {
                    self.push_plain_text(text, plain_start, cursor);
                    let body = self.inline_content(text, cursor + 1, closing);
                    self.push_element(
                        if delimiter == b'*' {
                            Element::Strong(body)
                        } else {
                            Element::Emph(body)
                        },
                        self.text_range(text, cursor, closing + 1),
                    );
                    cursor = closing + 1;
                    plain_start = cursor;
                    continue;
                }
            }

            if (cursor == start || !is_email_character(bytes[cursor - 1]))
                && let Some(email_end) = bare_email_end(bytes, cursor, end)
            {
                self.push_plain_text(text, plain_start, cursor);
                let range = self.text_range(text, cursor, email_end);
                let address = text.value[cursor..email_end].to_owned();
                self.push_element(
                    Element::Link {
                        destination: format!("mailto:{address}"),
                        title: None,
                        body: Content::single(Element::Text(address), range),
                    },
                    range,
                );
                cursor = email_end;
                plain_start = cursor;
                continue;
            }

            if (bytes[cursor..end].starts_with(b"https://")
                || bytes[cursor..end].starts_with(b"http://"))
                && (cursor == start || bytes[cursor - 1].is_ascii_whitespace())
            {
                self.push_plain_text(text, plain_start, cursor);
                let mut url_end = cursor;
                while url_end < end
                    && !bytes[url_end].is_ascii_whitespace()
                    && !(bytes[url_end] == b'\\'
                        && newline_end(bytes, url_end + 1).is_some_and(|after| after <= end))
                {
                    url_end += 1;
                }
                while url_end > cursor
                    && matches!(
                        bytes[url_end - 1],
                        b'.' | b',' | b';' | b'!' | b'?' | b')' | b']'
                    )
                {
                    url_end -= 1;
                }
                let range = self.text_range(text, cursor, url_end);
                let destination = text.value[cursor..url_end].to_owned();
                let body = Content::single(Element::Text(destination.clone()), range);
                self.push_element(
                    Element::Link {
                        destination,
                        title: None,
                        body,
                    },
                    range,
                );
                cursor = url_end;
                plain_start = cursor;
                continue;
            }

            cursor += 1;
        }
        self.push_plain_text(text, plain_start, end);
    }

    fn push_plain_text(&mut self, text: &notist_syntax::SpannedText, start: usize, end: usize) {
        if start < end {
            self.push_element(
                Element::Text(text.value[start..end].to_owned()),
                self.text_range(text, start, end),
            );
        }
    }

    fn inline_content(
        &self,
        text: &notist_syntax::SpannedText,
        start: usize,
        end: usize,
    ) -> Content {
        let mut state = LowerState {
            source: self.source,
            base_offset: self.base_offset,
            registry: self.registry,
            depth: self.depth,
            user_functions: self.user_functions,
            variables: self.variables.clone(),
            content: Content::new(),
            diagnostics: Vec::new(),
        };
        state.push_inline_text(text, start, end);
        state.content
    }

    fn text_range(&self, text: &notist_syntax::SpannedText, start: usize, end: usize) -> TextRange {
        TextRange::new(
            self.base_offset + text.range.start + start,
            self.base_offset + text.range.start + end,
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

#[derive(Clone, Copy)]
struct TableSourceSpan {
    start: usize,
    end: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceSugarKind {
    Heading,
    Table,
    List,
    Task,
}

#[derive(Clone, Copy)]
struct ListSourceSpan {
    start: usize,
    end: usize,
    kind: SourceSugarKind,
}

fn table_source_spans(source: &str, markup: &Markup) -> Vec<TableSourceSpan> {
    let mut lines = Vec::new();
    let mut offset = markup.range.start;
    for line in source[markup.range.start..markup.range.end].split_inclusive('\n') {
        let end = offset + line.len();
        lines.push((offset, end, line.trim_end_matches(['\r', '\n'])));
        offset = end;
    }
    if offset < markup.range.end {
        lines.push((offset, markup.range.end, &source[offset..markup.range.end]));
    }

    let marker_is_text = |position: usize| {
        markup.items.iter().any(|item| {
            matches!(item, MarkupItem::Text(text)
                if text.range.start <= position && position < text.range.end)
        })
    };

    let mut spans = Vec::new();
    let mut index = 0usize;
    while index < lines.len() {
        let (start, _, line) = lines[index];
        let trimmed = line.trim();
        let marker = start + line.len().saturating_sub(line.trim_start().len());
        if !trimmed.starts_with('|') || !trimmed.ends_with('|') || !marker_is_text(marker) {
            index += 1;
            continue;
        }

        let span_start = start;
        let mut span_end = lines[index].1;
        index += 1;
        while index < lines.len() {
            let candidate = lines[index].2.trim();
            if !candidate.starts_with('|') || !candidate.ends_with('|') {
                break;
            }
            span_end = lines[index].1;
            index += 1;
        }
        if index < lines.len()
            && lines[index]
                .2
                .trim_start_matches([' ', '\t'])
                .starts_with(": ")
        {
            span_end = lines[index].1;
            index += 1;
        }
        spans.push(TableSourceSpan {
            start: span_start,
            end: span_end,
        });
    }
    spans
}

fn heading_source_spans(
    source: &str,
    markup: &Markup,
    table_spans: &[TableSourceSpan],
) -> Vec<TableSourceSpan> {
    let marker_is_text = |position: usize| {
        markup.items.iter().any(|item| {
            matches!(item, MarkupItem::Text(text)
                if text.range.start <= position && position < text.range.end)
        })
    };

    let mut spans = Vec::new();
    let mut offset = markup.range.start;
    for line in source[markup.range.start..markup.range.end].split_inclusive('\n') {
        let end = offset + line.len();
        let line_without_newline = line.trim_end_matches(['\r', '\n']);
        let trimmed = line_without_newline.trim_start_matches([' ', '\t']);
        let level = trimmed.bytes().take_while(|byte| *byte == b'=').count();
        let marker = offset + line_without_newline.len().saturating_sub(trimmed.len());
        let inside_table = table_spans
            .iter()
            .any(|span| span.start <= marker && marker < span.end);
        if !inside_table
            && marker_is_text(marker)
            && (1..=6).contains(&level)
            && trimmed.as_bytes().get(level) == Some(&b' ')
            && !trimmed[level + 1..].trim().is_empty()
        {
            spans.push(TableSourceSpan { start: offset, end });
        }
        offset = end;
    }
    spans
}

fn list_source_spans(
    source: &str,
    markup: &Markup,
    table_spans: &[TableSourceSpan],
) -> Vec<ListSourceSpan> {
    let marker_is_text = |position: usize| {
        markup.items.iter().any(|item| {
            matches!(item, MarkupItem::Text(text)
                if text.range.start <= position && position < text.range.end)
        })
    };
    let line_kind = |line: &str| {
        let trimmed = line
            .trim_end_matches(['\r', '\n'])
            .trim_start_matches([' ', '\t']);
        if ["- [ ] ", "- [x] ", "- [X] "].iter().any(|marker| {
            trimmed
                .strip_prefix(marker)
                .is_some_and(|body| !body.trim().is_empty())
        }) {
            Some(SourceSugarKind::Task)
        } else if ["- ", "+ "].iter().any(|marker| {
            trimmed
                .strip_prefix(marker)
                .is_some_and(|body| !body.trim().is_empty())
        }) {
            Some(SourceSugarKind::List)
        } else {
            None
        }
    };

    let mut lines = Vec::new();
    let mut offset = markup.range.start;
    for line in source[markup.range.start..markup.range.end].split_inclusive('\n') {
        let end = offset + line.len();
        lines.push((offset, end, line));
        offset = end;
    }

    let mut spans = Vec::new();
    let mut index = 0usize;
    while index < lines.len() {
        let (start, end, line) = lines[index];
        let marker = start
            + line.trim_end_matches(['\r', '\n']).len().saturating_sub(
                line.trim_end_matches(['\r', '\n'])
                    .trim_start_matches([' ', '\t'])
                    .len(),
            );
        let inside_table = table_spans
            .iter()
            .any(|span| span.start <= marker && marker < span.end);
        let Some(kind) = (!inside_table && marker_is_text(marker))
            .then(|| line_kind(line))
            .flatten()
        else {
            index += 1;
            continue;
        };

        let span_start = start;
        let mut span_end = end;
        index += 1;
        while index < lines.len() {
            let (candidate_start, candidate_end, candidate) = lines[index];
            let candidate_line = candidate.trim_end_matches(['\r', '\n']);
            let candidate_marker = candidate_start
                + candidate_line
                    .len()
                    .saturating_sub(candidate_line.trim_start_matches([' ', '\t']).len());
            if !marker_is_text(candidate_marker) || line_kind(candidate) != Some(kind) {
                break;
            }
            span_end = candidate_end;
            index += 1;
        }
        spans.push(ListSourceSpan {
            start: span_start,
            end: span_end,
            kind,
        });
    }
    spans
}

fn table_separator_alignment(value: &str) -> Option<TableAlignment> {
    let value = value.trim();
    let left = value.starts_with(':');
    let right = value.ends_with(':');
    let value = value.strip_prefix(':').unwrap_or(value);
    let value = value.strip_suffix(':').unwrap_or(value);
    (value.len() >= 3 && value.bytes().all(|byte| byte == b'-')).then_some(match (left, right) {
        (true, false) => TableAlignment::Left,
        (false, true) => TableAlignment::Right,
        (true, true) => TableAlignment::Center,
        (false, false) => TableAlignment::Default,
    })
}

fn table_cell_ranges(source: &str, start: usize, end: usize) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut cells = Vec::new();
    let mut cell_start = start;
    let mut cursor = start;
    let mut raw_ticks = 0usize;
    let mut quoted = false;
    let mut square_depth = 0usize;
    let mut paren_depth = 0usize;

    while cursor < end {
        if raw_ticks > 0 {
            if bytes[cursor] == b'`' {
                let run = bytes[cursor..end]
                    .iter()
                    .take_while(|byte| **byte == b'`')
                    .count();
                if run == raw_ticks {
                    raw_ticks = 0;
                }
                cursor += run;
            } else {
                cursor += 1;
            }
            continue;
        }

        if quoted {
            match bytes[cursor] {
                b'\\' if cursor + 1 < end => cursor += 2,
                b'"' => {
                    quoted = false;
                    cursor += 1;
                }
                _ => cursor += 1,
            }
            continue;
        }

        match bytes[cursor] {
            b'`' => {
                raw_ticks = bytes[cursor..end]
                    .iter()
                    .take_while(|byte| **byte == b'`')
                    .count();
                cursor += raw_ticks;
            }
            b'"' => {
                quoted = true;
                cursor += 1;
            }
            b'[' => {
                square_depth += 1;
                cursor += 1;
            }
            b']' => {
                square_depth = square_depth.saturating_sub(1);
                cursor += 1;
            }
            b'(' => {
                paren_depth += 1;
                cursor += 1;
            }
            b')' => {
                paren_depth = paren_depth.saturating_sub(1);
                cursor += 1;
            }
            b'|' if square_depth == 0
                && paren_depth == 0
                && preceding_backslashes(bytes, start, cursor).is_multiple_of(2) =>
            {
                cells.push((cell_start, cursor));
                cell_start = cursor + 1;
                cursor += 1;
            }
            _ => cursor += 1,
        }
    }
    cells.push((cell_start, end));
    cells
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

fn bare_email_end(bytes: &[u8], start: usize, end: usize) -> Option<usize> {
    let mut candidate_end = start;
    while candidate_end < end && is_email_character(bytes[candidate_end]) {
        candidate_end += 1;
    }
    while candidate_end > start && bytes[candidate_end - 1] == b'.' {
        candidate_end -= 1;
    }
    let candidate = &bytes[start..candidate_end];
    let at = candidate.iter().position(|byte| *byte == b'@')?;
    if candidate[at + 1..].contains(&b'@') || at == 0 {
        return None;
    }
    let local = &candidate[..at];
    let domain = &candidate[at + 1..];
    if local.first() == Some(&b'.')
        || local.last() == Some(&b'.')
        || !local.iter().all(|byte| is_email_local_character(*byte))
    {
        return None;
    }
    let labels: Vec<_> = domain.split(|byte| *byte == b'.').collect();
    if labels.len() < 2
        || labels.iter().any(|label| {
            label.is_empty()
                || !label.first().is_some_and(u8::is_ascii_alphanumeric)
                || !label.last().is_some_and(u8::is_ascii_alphanumeric)
                || !label
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
        })
    {
        return None;
    }
    Some(candidate_end)
}

fn is_email_character(byte: u8) -> bool {
    is_email_local_character(byte) || byte == b'@'
}

fn is_email_local_character(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'.' | b'!'
                | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'/'
                | b'='
                | b'?'
                | b'^'
                | b'_'
                | b'`'
                | b'{'
                | b'|'
                | b'}'
                | b'~'
        )
}

fn citation_parts(value: &str) -> Option<(&str, Option<&str>)> {
    let (key, locator) = match value.split_once(',') {
        Some((key, locator)) => {
            let locator = locator.trim();
            if locator.is_empty() {
                return None;
            }
            (key.trim(), Some(locator))
        }
        None => (value.trim(), None),
    };
    citation_key_is_valid(key).then_some((key, locator))
}
