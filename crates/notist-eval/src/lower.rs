//! Expression evaluation for the unified pipeline.
//!
//! The lowering pass (`stream_lower`) owns the Markup → `Node` forest
//! translation; this module owns the expression language: literals, names,
//! operators, blocks, `let`, `if`, lambdas, user functions, and calls in
//! expression position. `Content` literals and trailing bodies lower through
//! the stream lowering pass and reduce on the node engine, so expression
//! evaluation always observes the input-side (fully reduced) forest.

use std::collections::HashMap;

use notist_model::{DefaultValue, FunctionSignature, Node, NodeValue, Parameter, TextRange, Type};
use notist_syntax::{
    BinaryOperator, BodyForm, Call, ContentBlock, Expression, ExpressionKind, Markup, MarkupItem,
    UnaryOperator, UserFunctionDefinition, UserParameter,
};

use crate::function::{Function, FunctionContext, FunctionInput, FunctionRegistry};
use crate::leaf::{ReduceFrame, ReduceLimits, node_engine};
use crate::type_system::{
    FunctionImplementation, FunctionValue, Value, ValueOrigin, bind_arguments, evaluate_literal,
};
use crate::{EvalDiagnostic, stream_lower};

pub(crate) fn collect_user_functions(
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
            MarkupItem::Table(sugar) => {
                for cell in &sugar.header {
                    collect_user_functions(&cell.body, functions);
                }
                for row in &sugar.rows {
                    for cell in row {
                        collect_user_functions(&cell.body, functions);
                    }
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

pub(crate) fn user_function_value(
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

/// Evaluates one expression with an isolated expression state, used by the
/// lowering pass to evaluate ordinary argument/let values while the
/// document-level environment stays in the lowerer.
pub(crate) fn evaluate_expression_fragment(
    source: &str,
    expression: &Expression,
    base_offset: usize,
    registry: &FunctionRegistry,
    depth: usize,
    user_functions: &HashMap<String, UserFunctionDefinition>,
    variables: Vec<HashMap<String, Value>>,
) -> (Value, Vec<EvalDiagnostic>) {
    let mut state = ExpressionState {
        source,
        base_offset,
        registry,
        depth,
        user_functions,
        variables,
    };
    let (value, _, diagnostics) = state.evaluate_expression(expression, expression.range);
    (value, diagnostics)
}

/// The expression evaluator's environment: a scope stack over the source,
/// with the function registry and the user-function table of the enclosing
/// document.
struct ExpressionState<'a> {
    source: &'a str,
    base_offset: usize,
    registry: &'a FunctionRegistry,
    depth: usize,
    user_functions: &'a HashMap<String, UserFunctionDefinition>,
    variables: Vec<HashMap<String, Value>>,
}

impl ExpressionState<'_> {
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
                                implementation: FunctionImplementation::Builtin(name.value.clone()),
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
                                existing.extend(content);
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
                (
                    joined.unwrap_or(Value::None),
                    ValueOrigin::Default,
                    diagnostics,
                )
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
            ExpressionKind::Target(reference) => (
                Value::Target((**reference).clone()),
                ValueOrigin::Literal {
                    range: expression.range,
                    payload_range: None,
                    string_form: None,
                    string_style: None,
                },
                Vec::new(),
            ),
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
            (BinaryOperator::Equal | BinaryOperator::NotEqual, ref left, ref right) => {
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

    /// Evaluates a `Content` literal: the block markup lowers through the
    /// stream lowering pass and reduces to the fixpoint, so the resulting
    /// `Value::Content` forest is input-side (fully reduced).
    fn evaluate_content_block(&mut self, block: &ContentBlock) -> (Value, Vec<EvalDiagnostic>) {
        let (nodes, mut diagnostics) = stream_lower::lower_body_with_environment(
            self.source,
            &block.markup,
            self.base_offset,
            self.registry,
            self.user_functions,
            self.variables.clone(),
        );
        let (forest, errors) = self.reduce_forest(nodes);
        diagnostics.extend(errors);
        (Value::Content(forest), diagnostics)
    }

    /// Reduces a lowered forest to the fixpoint with a fresh budget.
    fn reduce_forest(&self, nodes: Vec<Node>) -> (Vec<Node>, Vec<EvalDiagnostic>) {
        let limits = ReduceLimits::default();
        let mut frame = ReduceFrame::root(&limits);
        node_engine::reduce_nodes_recovering(nodes, self.registry, &limits, &mut frame)
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
            // Fixpoint rule: an unhandled name IS a leaf. The unresolved call
            // is preserved as content; check-phase owns unknown-name
            // diagnostics.
            let mut diagnostics = Vec::new();
            let (trailing, mut trailing_diagnostics) = self.evaluate_trailing(&call.trailing);
            diagnostics.append(&mut trailing_diagnostics);
            let arguments = call
                .arguments_range
                .map(|range| self.source[range.start..range.end].to_owned());
            let block = call
                .trailing
                .iter()
                .any(|block| block.form == BodyForm::Block);
            let mut node = Node::call(
                "core::unresolved-call",
                site_range.shifted(self.base_offset),
            )
            .arg("name", name.clone());
            if let Some(arguments) = arguments {
                node.args
                    .push(("arguments".into(), NodeValue::String(arguments)));
            }
            node.children = trailing
                .into_iter()
                .next()
                .map(|(forest, _)| forest)
                .unwrap_or_default();
            node.block = block;
            return (Value::Content(vec![node]), diagnostics);
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
            // Functions always return a Value; content results arrive as
            // `Value::Content` forests. Expression-position calls adopt the
            // returned forest as-is; macro-style re-reduction happens when a
            // forest re-enters the document stream, not inside one expression.
            Ok(value) if signature.result.accepts(&value.ty()) => (value, diagnostics),
            Ok(value) => {
                diagnostics.push(EvalDiagnostic {
                    message: format!(
                        "function `{name}` returned {}, expected {}",
                        value.ty(),
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

    /// Evaluates trailing Content blocks into reduced forests, one per block.
    fn evaluate_trailing(
        &mut self,
        trailing: &[ContentBlock],
    ) -> (Vec<(Vec<Node>, TextRange)>, Vec<EvalDiagnostic>) {
        let mut result = Vec::new();
        let mut diagnostics = Vec::new();
        for block in trailing {
            let (nodes, mut block_diagnostics) = stream_lower::lower_body_with_environment(
                self.source,
                &block.markup,
                self.base_offset,
                self.registry,
                self.user_functions,
                self.variables.clone(),
            );
            diagnostics.append(&mut block_diagnostics);
            let (forest, errors) = self.reduce_forest(nodes);
            diagnostics.extend(errors);
            let range = block.payload_range.shifted(self.base_offset);
            result.push((forest, range));
        }
        (result, diagnostics)
    }
}
