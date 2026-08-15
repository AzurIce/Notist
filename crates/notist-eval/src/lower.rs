#![allow(dead_code)]

use std::collections::HashMap;

use notist_model::{
    Content, DefaultValue, Element, ElementNode, FunctionSignature, Parameter, TextRange, Type,
};
use notist_syntax::{
    Attributes, BinaryOperator, BodyForm, Call, ContentBlock, EmbeddedExpression, Expression,
    ExpressionKind, Markup, MarkupItem, RawLiteral, RawLiteralForm, UnaryOperator,
    UserFunctionDefinition, UserParameter,
};

use crate::function::{Function, FunctionContext, FunctionInput, FunctionRegistry};
use crate::type_system::{
    FunctionImplementation, FunctionValue, Value, ValueOrigin, bind_arguments, evaluate_literal,
};
use crate::{EvalDiagnostic, Evaluation};

pub(crate) fn evaluate_markup(
    source: &str,
    markup: &Markup,
    base_offset: usize,
    registry: &FunctionRegistry,
    depth: usize,
) -> Evaluation {
    evaluate_markup_with_bindings(source, markup, base_offset, registry, depth, HashMap::new())
}

/// Evaluates markup with a pre-seeded document scope: the analysis layer uses
/// this to inject imported bindings before evaluation (D0004).
pub(crate) fn evaluate_markup_with_bindings(
    source: &str,
    markup: &Markup,
    base_offset: usize,
    registry: &FunctionRegistry,
    depth: usize,
    bindings: HashMap<String, Value>,
) -> Evaluation {
    // D0002 sequential scope: bindings (including function definitions) are
    // visible only after their declaration point; hoisting is gone.
    let user_functions = HashMap::new();
    evaluate_markup_in_environment(
        source,
        markup,
        base_offset,
        registry,
        depth,
        &user_functions,
        vec![bindings],
        true,
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
    handle_annotations: bool,
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
        annotations: Vec::new(),
        handle_annotations,
        pending_annotations: Vec::new(),
        pending_block_start: None,
        pending_block_end: 0,
        module_attributes: Vec::new(),
    };
    state.lower_markup(markup);
    state.finish_annotations();
    let bindings = state.variables.first().cloned().unwrap_or_default();
    Evaluation {
        content: state.content,
        diagnostics: state.diagnostics,
        bindings,
        annotations: state.annotations,
        module_attributes: state.module_attributes,
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
    annotations: Vec<crate::AnnotationEntry>,
    /// Whether this state processes `@[...]` / `@![...]` annotations. Only
    /// the document-level state does: nested fragment evaluations re-parse
    /// the same source and must not double-bind them.
    handle_annotations: bool,
    /// Block-prefix annotations awaiting their block, in source order.
    pending_annotations: Vec<(Attributes, TextRange)>,
    /// Range of the block currently being annotated, once it has started.
    pending_block_start: Option<TextRange>,
    /// Exclusive end of the block currently being annotated.
    pending_block_end: usize,
    /// Module annotations collected from `@![...]` items.
    module_attributes: Vec<Attributes>,
}

fn collect_user_functions(
    markup: &Markup,
    functions: &mut HashMap<String, UserFunctionDefinition>,
) {
    for item in &markup.items {
        match item {
            MarkupItem::Embedded(embedded) => {
                collect_expression_functions(&embedded.expression, functions);
            }
            MarkupItem::Heading(sugar) => collect_user_functions(&sugar.body, functions),
            MarkupItem::List(sugar) => {
                for row in &sugar.rows {
                    collect_user_functions(&row.body, functions);
                }
            }
            _ => {}
        }
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

fn user_function_value(
    definition: &UserFunctionDefinition,
    variables: &[HashMap<String, Value>],
) -> FunctionValue {
    let signature = signature_for_user_function(definition);
    FunctionValue {
        signature,
        implementation: FunctionImplementation::User {
            parameters: definition.parameters.clone(),
            result: definition.result.clone(),
            body: definition.body.clone(),
        },
        captured: variables.first().cloned().unwrap_or_default(),
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

fn is_arithmetic(operator: BinaryOperator) -> bool {
    matches!(
        operator,
        BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
    )
}

fn compare_ints(operator: BinaryOperator, left: i64, right: i64) -> bool {
    match operator {
        BinaryOperator::Less => left < right,
        BinaryOperator::LessEqual => left <= right,
        BinaryOperator::Greater => left > right,
        BinaryOperator::GreaterEqual => left >= right,
        _ => unreachable!("compare_ints only handles ordering operators"),
    }
}

fn compare_floats(operator: BinaryOperator, left: f64, right: f64) -> bool {
    match operator {
        BinaryOperator::Less => left < right,
        BinaryOperator::LessEqual => left <= right,
        BinaryOperator::Greater => left > right,
        BinaryOperator::GreaterEqual => left >= right,
        _ => unreachable!("compare_floats only handles ordering operators"),
    }
}

fn float_binary(operator: BinaryOperator, left: f64, right: f64) -> Option<f64> {
    match operator {
        BinaryOperator::Add => Some(left + right),
        BinaryOperator::Subtract => Some(left - right),
        BinaryOperator::Multiply => Some(left * right),
        BinaryOperator::Divide if right == 0.0 => None,
        BinaryOperator::Divide => Some(left / right),
        _ => None,
    }
}

impl LowerState<'_> {
    fn lower_markup(&mut self, markup: &Markup) {
        // D0006: block-prefix and module annotations are collected once from
        // the document-level parse; nested fragment re-parses skip them.
        if self.handle_annotations {
            for item in &markup.items {
                match item {
                    MarkupItem::BlockAnnotation(annotation) => {
                        self.pending_annotations
                            .push((annotation.attributes.clone(), annotation.range));
                    }
                    MarkupItem::ModuleAnnotation(annotation) => {
                        self.module_attributes.push(annotation.attributes.clone());
                    }
                    _ => {}
                }
            }
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
                MarkupItem::Heading(sugar) => self.lower_heading_sugar(sugar),
                MarkupItem::Rule(range) => {
                    self.push_element(Element::Rule, range.shifted(self.base_offset));
                }
                MarkupItem::List(sugar) => self.lower_list_sugar(sugar),
                // Annotations were collected ahead of the content loop.
                MarkupItem::BlockAnnotation(_) | MarkupItem::ModuleAnnotation(_) => {}
            }
        }
    }

    /// Lowers a heading sugar node (D0003): the body Markup is evaluated as a
    /// nested fragment and becomes the heading body.
    fn lower_heading_sugar(&mut self, sugar: &notist_syntax::HeadingSugar) {
        let evaluation = evaluate_markup_in_environment(
            self.source,
            &sugar.body,
            self.base_offset,
            self.registry,
            self.depth + 1,
            self.user_functions,
            self.variables.clone(),
            false,
        );
        self.annotations.extend(evaluation.annotations);
        self.diagnostics.extend(evaluation.diagnostics);
        self.push_element(
            Element::Heading {
                level: sugar.level as u8,
                body: evaluation.content,
            },
            sugar.range.shifted(self.base_offset),
        );
    }

    fn lower_embedded(&mut self, embedded: &EmbeddedExpression) {
        let (value, _, mut diagnostics) =
            self.evaluate_expression(&embedded.expression, embedded.scope_range);
        self.diagnostics.append(&mut diagnostics);
        if !embedded.attributes.items.is_empty() || embedded.attributes.id.is_some() {
            self.annotations.push(crate::AnnotationEntry {
                range: embedded.scope_range.shifted(self.base_offset),
                attributes: embedded.attributes.clone(),
            });
        }
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
                // D0002 insertion rules: Int / Float / Bool stringify into
                // Text; only Function (and legacy collection values) refuse.
                let text = match other {
                    Value::Int(value) => Some(value.to_string()),
                    Value::Float(value) => Some(value.to_string()),
                    Value::Bool(value) => Some(value.to_string()),
                    _ => None,
                };
                match text {
                    Some(text) => self.push_element(
                        Element::Text(text),
                        embedded.scope_range.shifted(self.base_offset),
                    ),
                    None => self.diagnostics.push(EvalDiagnostic {
                        message: format!("cannot insert {} into Markup", other.ty()),
                        range: embedded.expression.range.shifted(self.base_offset),
                    }),
                }
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
                        self.registry.get(&name.value).map(|function| {
                            Value::Function(Box::new(FunctionValue {
                                signature: function.signature(),
                                implementation: FunctionImplementation::Builtin(
                                    name.value.clone(),
                                ),
                                captured: self.variables.first().cloned().unwrap_or_default(),
                            }))
                        })
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
            ExpressionKind::Unary { operator, operand } => {
                let (value, _, mut diagnostics) = self.evaluate_expression(operand, operand.range);
                match (operator, value) {
                    (UnaryOperator::Not, Value::Bool(value)) => {
                        (Value::Bool(!value), ValueOrigin::Default, diagnostics)
                    }
                    (UnaryOperator::Not, other) => {
                        diagnostics.push(EvalDiagnostic {
                            message: format!("`not` requires a Bool operand, got {}", other.ty()),
                            range: expression_range.shifted(self.base_offset),
                        });
                        (Value::None, ValueOrigin::Default, diagnostics)
                    }
                }
            }
            ExpressionKind::Block(statements) => {
                // D0006: the block value is the join of its statement values;
                // `let` yields none and does not participate. Blocks are
                // lexical scopes: bindings do not escape.
                let mut joined: Option<Value> = None;
                let mut diagnostics = Vec::new();
                self.variables.push(HashMap::new());
                for statement in statements {
                    let (value, _, mut statement_diagnostics) =
                        self.evaluate_expression(statement, statement.range);
                    diagnostics.append(&mut statement_diagnostics);
                    match value {
                        Value::None => {}
                        Value::Content(content) => match &mut joined {
                            None => joined = Some(Value::Content(content)),
                            Some(Value::Content(existing)) => {
                                existing.elements.extend(content.elements);
                            }
                            Some(other) => diagnostics.push(EvalDiagnostic {
                                message: format!(
                                    "cannot combine Content with {} in a code block",
                                    other.ty()
                                ),
                                range: expression_range.shifted(self.base_offset),
                            }),
                        },
                        value => match &mut joined {
                            None => joined = Some(value),
                            Some(existing) => diagnostics.push(EvalDiagnostic {
                                message: format!(
                                    "cannot combine {} with {} in a code block",
                                    existing.ty(),
                                    value.ty()
                                ),
                                range: expression_range.shifted(self.base_offset),
                            }),
                        },
                    }
                }
                self.variables.pop();
                (joined.unwrap_or(Value::None), ValueOrigin::Default, diagnostics)
            }
            ExpressionKind::Let { name, value, .. } => {
                let (value, _, diagnostics) = self.evaluate_expression(value, value.range);
                // Document-level bindings need a base scope; nested blocks and
                // content literals already carry one (cloned) scope stack.
                if self.variables.is_empty() {
                    self.variables.push(HashMap::new());
                }
                if let Some(scope) = self.variables.last_mut() {
                    scope.insert(name.value.clone(), value);
                }
                (Value::None, ValueOrigin::Default, diagnostics)
            }
            ExpressionKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let (condition_value, _, mut diagnostics) =
                    self.evaluate_expression(condition, condition.range);
                match condition_value {
                    Value::Bool(true) => {
                        let (value, origin, mut branch_diagnostics) =
                            self.evaluate_expression(then_branch, then_branch.range);
                        diagnostics.append(&mut branch_diagnostics);
                        (value, origin, diagnostics)
                    }
                    Value::Bool(false) => match else_branch {
                        Some(branch) => {
                            let (value, origin, mut branch_diagnostics) =
                                self.evaluate_expression(branch, branch.range);
                            diagnostics.append(&mut branch_diagnostics);
                            (value, origin, diagnostics)
                        }
                        None => (Value::None, ValueOrigin::Default, diagnostics),
                    },
                    other => {
                        diagnostics.push(EvalDiagnostic {
                            message: format!("`if` condition must be a Bool, got {}", other.ty()),
                            range: expression_range.shifted(self.base_offset),
                        });
                        (Value::None, ValueOrigin::Default, diagnostics)
                    }
                }
            }
            ExpressionKind::Lambda { parameters, body } => {
                let signature = FunctionSignature {
                    parameters: parameters
                        .iter()
                        .map(|parameter| Parameter {
                            name: parameter.name.value.clone(),
                            ty: parameter.ty.clone(),
                            default: parameter.default.as_ref().and_then(expression_default),
                        })
                        .collect(),
                    trailing_content: parameters
                        .last()
                        .filter(|parameter| parameter.ty == Type::Content)
                        .map(|parameter| parameter.name.value.clone()),
                    // The result type is inferred by the static checker; at
                    // runtime the internal Inferred marker accepts any value
                    // (R07: inference stays outside the written surface).
                    result: Type::Inferred,
                };
                let captured = self.variables.first().cloned().unwrap_or_default();
                let function = FunctionValue {
                    signature: signature.clone(),
                    implementation: FunctionImplementation::User {
                        parameters: parameters.clone(),
                        result: signature.result,
                        body: (**body).clone(),
                    },
                    captured,
                };
                (
                    Value::Function(Box::new(function)),
                    ValueOrigin::Default,
                    Vec::new(),
                )
            }
            ExpressionKind::Import { .. } => {
                // Imports resolve only within a vault context: the analysis
                // layer orchestrates cross-module evaluation and seeds the
                // imported bindings (D0004). Standalone evaluation is a no-op.
                (Value::None, ValueOrigin::Default, Vec::new())
            }
            ExpressionKind::LetFunction(definition) => {
                // D0002: the definition binds a first-class closure into the
                // current scope (hoisting remains as a transitional fallback).
                if self.variables.is_empty() {
                    self.variables.push(HashMap::new());
                }
                let function = user_function_value(definition, &self.variables);
                if let Some(scope) = self.variables.last_mut() {
                    scope.insert(
                        definition.name.value.clone(),
                        Value::Function(Box::new(function)),
                    );
                }
                (Value::None, ValueOrigin::Default, Vec::new())
            }
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
        // `and` / `or` short-circuit (D0007): the right side is only evaluated
        // when the left side does not already decide the result.
        if matches!(operator, BinaryOperator::And | BinaryOperator::Or) {
            let Value::Bool(left_bool) = left else {
                diagnostics.push(EvalDiagnostic {
                    message: format!("operator {operator:?} requires Bool operands"),
                    range: range.shifted(self.base_offset),
                });
                return (Value::None, diagnostics);
            };
            if (operator == BinaryOperator::And && !left_bool)
                || (operator == BinaryOperator::Or && left_bool)
            {
                return (Value::Bool(left_bool), diagnostics);
            }
            let (right, _, mut right_diagnostics) = self.evaluate_expression(right, right.range);
            diagnostics.append(&mut right_diagnostics);
            if !diagnostics.is_empty() {
                return (Value::None, diagnostics);
            }
            let Value::Bool(right_bool) = right else {
                diagnostics.push(EvalDiagnostic {
                    message: format!("operator {operator:?} requires Bool operands"),
                    range: range.shifted(self.base_offset),
                });
                return (Value::None, diagnostics);
            };
            return (Value::Bool(right_bool), diagnostics);
        }
        let (right, _, mut right_diagnostics) = self.evaluate_expression(right, right.range);
        diagnostics.append(&mut right_diagnostics);
        if !diagnostics.is_empty() {
            return (Value::None, diagnostics);
        }

        let value = match (operator, left, right) {
            (
                BinaryOperator::Equal | BinaryOperator::NotEqual,
                ref left,
                ref right,
            ) => {
                if matches!(left, Value::Content(_) | Value::Function(_))
                    || matches!(right, Value::Content(_) | Value::Function(_))
                {
                    diagnostics.push(EvalDiagnostic {
                        message: format!(
                            "operator {operator:?} is not defined for Content or Function values"
                        ),
                        range: range.shifted(self.base_offset),
                    });
                    return (Value::None, diagnostics);
                }
                // Same-family equality; cross-family values are unequal (D0007).
                let equal = left == right;
                Value::Bool(if operator == BinaryOperator::Equal {
                    equal
                } else {
                    !equal
                })
            }
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
            (operator, Value::Int(left), Value::Float(right)) if is_arithmetic(operator) => {
                match float_binary(operator, left as f64, right) {
                    Some(value) => Value::Float(value),
                    None => return self.division_by_zero(range, diagnostics),
                }
            }
            (operator, Value::Float(left), Value::Int(right)) if is_arithmetic(operator) => {
                match float_binary(operator, left, right as f64) {
                    Some(value) => Value::Float(value),
                    None => return self.division_by_zero(range, diagnostics),
                }
            }
            (operator, Value::Float(left), Value::Float(right)) if is_arithmetic(operator) => {
                match float_binary(operator, left, right) {
                    Some(value) => Value::Float(value),
                    None => return self.division_by_zero(range, diagnostics),
                }
            }
            (
                BinaryOperator::Less
                | BinaryOperator::LessEqual
                | BinaryOperator::Greater
                | BinaryOperator::GreaterEqual,
                Value::Int(left),
                Value::Int(right),
            ) => Value::Bool(compare_ints(operator, left, right)),
            (
                BinaryOperator::Less
                | BinaryOperator::LessEqual
                | BinaryOperator::Greater
                | BinaryOperator::GreaterEqual,
                Value::Int(left),
                Value::Float(right),
            ) => Value::Bool(compare_floats(operator, left as f64, right)),
            (
                BinaryOperator::Less
                | BinaryOperator::LessEqual
                | BinaryOperator::Greater
                | BinaryOperator::GreaterEqual,
                Value::Float(left),
                Value::Int(right),
            ) => Value::Bool(compare_floats(operator, left, right as f64)),
            (
                BinaryOperator::Less
                | BinaryOperator::LessEqual
                | BinaryOperator::Greater
                | BinaryOperator::GreaterEqual,
                Value::Float(left),
                Value::Float(right),
            ) => Value::Bool(compare_floats(operator, left, right)),
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
            false,
        );
        let diagnostics = evaluation.diagnostics;
        self.annotations.extend(evaluation.annotations);
        (Value::Content(evaluation.content), diagnostics)
    }

    fn evaluate_call(
        &mut self,
        call: &Call,
        site_range: TextRange,
    ) -> (Value, Vec<EvalDiagnostic>) {
        let name = &call.name.value;

        // D0002: the callee is a first-class value resolved in the current
        // environment; closures dispatch on their implementation.
        if let Some(value) = self
            .variables
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
        {
            return match value {
                Value::Function(function) => {
                    self.evaluate_function_value(&function, call, site_range)
                }
                other => (
                    Value::None,
                    vec![EvalDiagnostic {
                        message: format!("`{name}` is not callable ({})", other.ty()),
                        range: call.name.range.shifted(self.base_offset),
                    }],
                ),
            };
        }

        let Some(function) = self.registry.get(name) else {
            let mut diagnostics = Vec::new();
            diagnostics.push(EvalDiagnostic {
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
        self.evaluate_builtin(function, name, call, site_range)
    }

    /// Dispatches a first-class function value (D0002).
    fn evaluate_function_value(
        &mut self,
        function: &FunctionValue,
        call: &Call,
        site_range: TextRange,
    ) -> (Value, Vec<EvalDiagnostic>) {
        match &function.implementation {
            FunctionImplementation::Builtin(name) => match self.registry.get(name) {
                Some(builtin) => self.evaluate_builtin(builtin, name, call, site_range),
                None => (
                    Value::None,
                    vec![EvalDiagnostic {
                        message: format!("unknown builtin `{name}`"),
                        range: call.name.range.shifted(self.base_offset),
                    }],
                ),
            },
            FunctionImplementation::User {
                parameters,
                result,
                body,
            } => self.evaluate_closure_body(
                &call.name.value,
                &function.signature,
                parameters,
                result,
                body,
                &function.captured,
                call,
                site_range,
            ),
        }
    }

    /// Evaluates a call against a registered builtin function.
    fn evaluate_builtin(
        &mut self,
        function: &dyn Function,
        name: &str,
        call: &Call,
        site_range: TextRange,
    ) -> (Value, Vec<EvalDiagnostic>) {
        let mut diagnostics = Vec::new();
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

    /// Evaluates a user-defined body as a closure call: arguments bind per the
    /// signature, then the body runs with the captured environment below the
    /// parameter scope (D0002).
    #[allow(clippy::too_many_arguments)]
    fn evaluate_closure_body(
        &mut self,
        name: &str,
        signature: &FunctionSignature,
        _parameters: &[UserParameter],
        result: &Type,
        body: &Expression,
        captured: &HashMap<String, Value>,
        call: &Call,
        site_range: TextRange,
    ) -> (Value, Vec<EvalDiagnostic>) {
        let mut diagnostics = Vec::new();
        if self.depth >= 64 {
            diagnostics.push(EvalDiagnostic {
                message: format!("function `{name}` exceeded the evaluation depth limit"),
                range: site_range.shifted(self.base_offset),
            });
            return (Value::None, diagnostics);
        }
        let (trailing_content, mut trailing_diagnostics) = self.evaluate_trailing(&call.trailing);
        diagnostics.append(&mut trailing_diagnostics);
        let bound = match bind_arguments(
            signature,
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

        self.variables.push(captured.clone());
        self.variables.push(bound.into_values());
        self.depth += 1;
        let (value, _, mut body_diagnostics) = self.evaluate_expression(body, body.range);
        self.depth -= 1;
        self.variables.pop();
        self.variables.pop();
        diagnostics.append(&mut body_diagnostics);
        if !result.accepts(&value.ty()) {
            diagnostics.push(EvalDiagnostic {
                message: format!(
                    "function `{name}` returned {}, expected {}",
                    value.ty(),
                    result
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
                false,
            );
            diagnostics.extend(evaluation.diagnostics);
            self.annotations.extend(evaluation.annotations);
            let range = block.payload_range.shifted(self.base_offset);
            result.push((evaluation.content, range));
        }
        (result, diagnostics)
    }

    fn push_element(&mut self, element: Element, range: TextRange) {
        let inline = element.is_inline();
        let parbreak = matches!(element, Element::Parbreak);
        let blank = matches!(&element, Element::Text(text) if text.trim().is_empty());
        self.content.elements.push(ElementNode { element, range });
        self.track_pending_annotation(range, inline, parbreak, blank);
    }

    /// Advances the block-annotation tracking for one produced element: the
    /// first non-Parbreak element starts the annotated block, inline elements
    /// extend it, and a Parbreak or a block-level element closes it (D0006:
    /// `@[...]` binds the immediately following block-level node).
    fn track_pending_annotation(&mut self, range: TextRange, inline: bool, parbreak: bool, blank: bool) {
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

    /// Emits annotation-table entries for the tracked block `[start, end)` to
    /// every pending `@[...]` annotation that precedes the block start; later
    /// annotations stay pending for their own following block (D0006: stacked
    /// annotations share one block, separate annotations get separate blocks).
    fn flush_pending_annotations(&mut self, block_end: usize) {
        let Some(start) = self.pending_block_start.take() else {
            return;
        };
        let mut remaining = Vec::new();
        for (attributes, annotation_range) in self.pending_annotations.drain(..) {
            if annotation_range.start < start.start {
                self.annotations.push(crate::AnnotationEntry {
                    range: TextRange::new(start.start, block_end),
                    attributes,
                });
            } else {
                remaining.push((attributes, annotation_range));
            }
        }
        self.pending_annotations = remaining;
    }

    /// Finishes block-annotation tracking at the end of a markup sequence:
    /// a block extending to the end flushes normally; annotations with no
    /// following block are dangling (D0006 error recovery).
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

    /// Lowers a list sugar node (D0003): each row body is evaluated as a
    /// nested fragment, and indentation nests items into their parent bodies.
    fn lower_list_sugar(&mut self, sugar: &notist_syntax::ListSugar) {
        struct ListRow {
            indent: usize,
            ordered: bool,
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
                            value: None,
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

        let mut rows: std::collections::VecDeque<ListRow> = sugar
            .rows
            .iter()
            .map(|row| {
                let evaluation = evaluate_markup_in_environment(
                    self.source,
                    &row.body,
                    self.base_offset,
                    self.registry,
                    self.depth + 1,
                    self.user_functions,
                    self.variables.clone(),
                    false,
                );
                self.annotations.extend(evaluation.annotations);
                self.diagnostics.extend(evaluation.diagnostics);
                ListRow {
                    indent: row.indent,
                    ordered: row.ordered,
                    body: evaluation.content,
                    range: row.range.shifted(self.base_offset),
                }
            })
            .collect();
        let mut items = Vec::new();
        while let Some(base_indent) = rows.front().map(|row| row.indent) {
            let Some(root_items) = nested_items(&mut rows, base_indent) else {
                return;
            };
            items.extend(root_items);
        }
        self.content.elements.extend(items);
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
                && bytes
                    .get(cursor + 1)
                    .is_some_and(|&next| !next.is_ascii_whitespace() && next != bytes[cursor])
            {
                let delimiter = bytes[cursor];
                if let Some(closing) = find_unescaped_sequence(bytes, cursor + 1, end, &[delimiter])
                    && closing > cursor + 1
                    && !bytes[closing - 1].is_ascii_whitespace()
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
            annotations: Vec::new(),
            handle_annotations: false,
            pending_annotations: Vec::new(),
            pending_block_start: None,
            pending_block_end: 0,
            module_attributes: Vec::new(),
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
