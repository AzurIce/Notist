//! 严格底向上规约与降级恢复。
//!
//! 四条规则：
//! - 语句按文档序执行，`let` 顺序绑定、不可变、就近遮蔽；
//! - 实参先求值，内容表达式归约到终态，再分发本节点；
//! - 求值期只执行解析期的结论：绑定映射、捕获集、类型兜底检查；
//! - 任何失败都降级为可见 Item，内容不丢，只多一条 warn。

use notist_model::TextRange;

use crate::ast::{ArgLabel, BinaryOp, UnaryOp};
use crate::hir::{self, BindingId, CallPlan, Signature, Variadic};
use crate::ir::{
    Content, CoreElement, Diagnostic, ElementName, Flow, FunctionBody, FunctionValue, Item,
    ItemState, ModuleResult, Value,
};
use crate::registry::{self, CoreFunction};
use crate::resolve;
use crate::types::Type;

/// 把一个模块的意图层产物规约成权威产物。
pub fn reduce(module: &hir::Module) -> ModuleResult {
    let mut context = Context {
        bindings: &module.bindings,
        diagnostics: module.diagnostics.clone(),
    };
    let mut environment = Environment::default();
    let content = context.reduce_statements(&module.body, &mut environment);
    ModuleResult {
        content,
        bindings: root_bindings(module, &environment),
        diagnostics: context.diagnostics,
    }
}

/// 根作用域的绑定按定义顺序导出，同名取最后一次。
fn root_bindings(module: &hir::Module, environment: &Environment) -> Vec<(String, Value)> {
    let mut exported: Vec<(String, Value)> = Vec::new();
    for statement in &module.body {
        let binding = match statement {
            hir::Statement::Bind { binding, .. } | hir::Statement::Extern { binding, .. } => {
                *binding
            }
            hir::Statement::Value { .. } => continue,
        };
        let name = match module.bindings.get(binding as usize) {
            Some(binding) => binding.name.clone(),
            None => continue,
        };
        let value = environment.get(binding).cloned().unwrap_or(Value::Unit);
        exported.retain(|(existing, _)| existing != &name);
        exported.push((name, value));
    }
    exported
}

/// 词法环境：绑定 id 到值的栈，作用域退出时截断。
#[derive(Default)]
struct Environment {
    bindings: Vec<(BindingId, Value)>,
}

impl Environment {
    fn get(&self, binding: BindingId) -> Option<&Value> {
        self.bindings
            .iter()
            .rev()
            .find(|(id, _)| *id == binding)
            .map(|(_, value)| value)
    }

    fn define(&mut self, binding: BindingId, value: Value) {
        self.bindings.push((binding, value));
    }

    fn mark(&self) -> usize {
        self.bindings.len()
    }

    fn truncate(&mut self, mark: usize) {
        self.bindings.truncate(mark);
    }
}

struct Context<'a> {
    bindings: &'a [hir::Binding],
    diagnostics: Vec<Diagnostic>,
}

impl Context<'_> {
    /// 语句序列的内容语义：值语句的内容拼接，其余语句不产内容。
    fn reduce_statements(
        &mut self,
        statements: &[hir::Statement],
        environment: &mut Environment,
    ) -> Content {
        let mut items = Vec::new();
        for statement in statements {
            match statement {
                hir::Statement::Bind { binding, value, .. } => {
                    let value = self.eval(value, environment);
                    environment.define(*binding, value);
                }
                hir::Statement::Extern { binding, .. } => {
                    let value = self.extern_value(*binding);
                    environment.define(*binding, value);
                }
                hir::Statement::Value { expr, range } => {
                    let value = self.eval(expr, environment);
                    match value {
                        Value::Content(content) => items.extend(content.items),
                        other => self.diagnostics.push(Diagnostic::warn(
                            "value-is-not-content",
                            format!(
                                "statement produces {} and adds no content",
                                other.type_name()
                            ),
                            *range,
                        )),
                    }
                }
            }
        }
        Content { items }
    }

    /// 代码块的值语义：最后一条值语句的值，空块为 Unit。
    fn eval_block(&mut self, statements: &[hir::Statement], environment: &mut Environment) -> Value {
        let mark = environment.mark();
        let mut last = Value::Unit;
        for statement in statements {
            match statement {
                hir::Statement::Bind { binding, value, .. } => {
                    let value = self.eval(value, environment);
                    environment.define(*binding, value);
                }
                hir::Statement::Extern { binding, .. } => {
                    let value = self.extern_value(*binding);
                    environment.define(*binding, value);
                }
                hir::Statement::Value { expr, .. } => last = self.eval(expr, environment),
            }
        }
        environment.truncate(mark);
        last
    }

    fn extern_value(&self, binding: BindingId) -> Value {
        let signature = match self.bindings.get(binding as usize) {
            Some(hir::Binding {
                kind:
                    hir::BindingKind::Function {
                        signature,
                        declared_only: true,
                    },
                ..
            }) => signature.clone(),
            _ => Signature {
                params: Vec::new(),
                returns: None,
                variadic: None,
                trailing: None,
            },
        };
        Value::Function(Box::new(FunctionValue {
            signature,
            captures: Vec::new(),
            body: FunctionBody::Extern,
        }))
    }

    fn eval(&mut self, expr: &hir::Expr, environment: &mut Environment) -> Value {
        match expr {
            hir::Expr::Literal { value, .. } => value.clone(),
            hir::Expr::Local { binding, range } => match environment.get(*binding) {
                Some(value) => value.clone(),
                None => {
                    self.diagnostics.push(Diagnostic::warn(
                        "unresolved-name",
                        "this binding has no value yet",
                        *range,
                    ));
                    Value::Unit
                }
            },
            hir::Expr::Unknown { name, range } => {
                self.diagnostics.push(Diagnostic::warn(
                    "unresolved-name",
                    format!("unresolved name `{name}`"),
                    *range,
                ));
                Value::Unit
            }
            hir::Expr::Content { body, .. } => {
                let mark = environment.mark();
                let content = self.reduce_statements(body, environment);
                environment.truncate(mark);
                Value::Content(content)
            }
            hir::Expr::Block { body, .. } => self.eval_block(body, environment),
            hir::Expr::Lambda {
                params,
                param_bindings,
                returns,
                body,
                captures,
                ..
            } => {
                let captured = captures
                    .iter()
                    .filter_map(|binding| {
                        environment
                            .get(*binding)
                            .cloned()
                            .map(|value| (*binding, value))
                    })
                    .collect();
                Value::Function(Box::new(FunctionValue {
                    signature: Signature {
                        params: params.clone(),
                        returns: *returns,
                        variadic: None,
                        trailing: None,
                    },
                    captures: captured,
                    body: FunctionBody::User {
                        body: body.clone(),
                        param_bindings: param_bindings.clone(),
                    },
                }))
            }
            hir::Expr::Call {
                callee,
                args,
                plan,
                range,
            } => self.eval_call(callee, args, plan, *range, environment),
            hir::Expr::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => match self.eval(condition, environment) {
                Value::Bool(true) => self.eval(then_branch, environment),
                Value::Bool(false) => match else_branch {
                    Some(branch) => self.eval(branch, environment),
                    None => Value::Unit,
                },
                other => {
                    let range = condition.range();
                    self.diagnostics.push(Diagnostic::warn(
                        "type-mismatch",
                        format!("`if` expects Bool, found {}", other.type_name()),
                        range,
                    ));
                    Value::Unit
                }
            },
            hir::Expr::Unary { op, operand, range } => {
                let value = self.eval(operand, environment);
                self.unary(*op, value, *range)
            }
            hir::Expr::Binary {
                op,
                left,
                right,
                range,
            } => self.binary(*op, left, right, *range, environment),
        }
    }

    fn unary(&mut self, op: UnaryOp, value: Value, range: TextRange) -> Value {
        match (op, value) {
            (UnaryOp::Negate, Value::Int(value)) => Value::Int(-value),
            (UnaryOp::Negate, Value::Float(value)) => Value::Float(-value),
            (UnaryOp::Not, Value::Bool(value)) => Value::Bool(!value),
            (op, value) => {
                self.diagnostics.push(Diagnostic::warn(
                    "type-mismatch",
                    format!("`{}` does not accept {}", op.as_str(), value.type_name()),
                    range,
                ));
                Value::Unit
            }
        }
    }

    fn binary(
        &mut self,
        op: BinaryOp,
        left: &hir::Expr,
        right: &hir::Expr,
        range: TextRange,
        environment: &mut Environment,
    ) -> Value {
        // 逻辑运算在表达式层短路，不进实参求值。
        if op.is_short_circuit() {
            let value = match self.eval(left, environment) {
                Value::Bool(value) => value,
                other => {
                    self.diagnostics.push(Diagnostic::warn(
                        "type-mismatch",
                        format!("`{}` expects Bool, found {}", op.as_str(), other.type_name()),
                        left.range(),
                    ));
                    return Value::Unit;
                }
            };
            let decided = match op {
                BinaryOp::And => !value,
                BinaryOp::Or => value,
                _ => false,
            };
            if decided {
                return Value::Bool(value);
            }
            return match self.eval(right, environment) {
                Value::Bool(value) => Value::Bool(value),
                other => {
                    self.diagnostics.push(Diagnostic::warn(
                        "type-mismatch",
                        format!("`{}` expects Bool, found {}", op.as_str(), other.type_name()),
                        right.range(),
                    ));
                    Value::Unit
                }
            };
        }

        let left = self.eval(left, environment);
        let right = self.eval(right, environment);
        self.apply(op, left, right, range)
    }

    fn apply(&mut self, op: BinaryOp, left: Value, right: Value, range: TextRange) -> Value {
        match op {
            BinaryOp::Equal => return Value::Bool(left == right),
            BinaryOp::NotEqual => return Value::Bool(left != right),
            _ => {}
        }
        let numbers = match (&left, &right) {
            (Value::Int(left), Value::Int(right)) => Some((*left as f64, *right as f64, true)),
            (Value::Float(left), Value::Float(right)) => Some((*left, *right, false)),
            (Value::Float(left), Value::Int(right)) => Some((*left, *right as f64, false)),
            (Value::Int(left), Value::Float(right)) => Some((*left as f64, *right, false)),
            _ => None,
        };
        match op {
            BinaryOp::Add => match (&left, &right) {
                (Value::String(left), Value::String(right)) => {
                    return Value::String(format!("{left}{right}"));
                }
                _ => {}
            },
            _ => {}
        }
        let Some((left_number, right_number, integers)) = numbers else {
            self.diagnostics.push(Diagnostic::warn(
                "type-mismatch",
                format!(
                    "`{}` does not accept {} and {}",
                    op.as_str(),
                    left.type_name(),
                    right.type_name()
                ),
                range,
            ));
            return Value::Unit;
        };
        let result = match op {
            BinaryOp::Add => left_number + right_number,
            BinaryOp::Subtract => left_number - right_number,
            BinaryOp::Multiply => left_number * right_number,
            BinaryOp::Divide => {
                if right_number == 0.0 {
                    self.diagnostics.push(Diagnostic::warn(
                        "division-by-zero",
                        "division by zero",
                        range,
                    ));
                    return Value::Unit;
                }
                left_number / right_number
            }
            BinaryOp::Less => return Value::Bool(left_number < right_number),
            BinaryOp::LessEqual => return Value::Bool(left_number <= right_number),
            BinaryOp::Greater => return Value::Bool(left_number > right_number),
            BinaryOp::GreaterEqual => return Value::Bool(left_number >= right_number),
            BinaryOp::Equal | BinaryOp::NotEqual | BinaryOp::And | BinaryOp::Or => {
                unreachable!("handled above")
            }
        };
        if integers {
            Value::Int(result as i64)
        } else {
            Value::Float(result)
        }
    }

    // ---- 调用 ----

    fn eval_call(
        &mut self,
        callee: &hir::Callee,
        args: &[hir::Arg],
        plan: &CallPlan,
        range: TextRange,
        environment: &mut Environment,
    ) -> Value {
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            values.push(self.eval(&arg.value, environment));
        }

        match callee {
            hir::Callee::Element(element) => self.call_element(*element, plan, &values, range),
            hir::Callee::Function(function) => {
                self.call_registered(function, plan, args, &values, range)
            }
            hir::Callee::Local(binding) => match environment.get(*binding).cloned() {
                Some(Value::Function(function)) => {
                    self.call_function(Some(*binding), &function, plan, args, &values, range)
                }
                Some(other) => {
                    self.diagnostics.push(Diagnostic::warn(
                        "not-callable",
                        format!("this binding is {} and cannot be called", other.type_name()),
                        range,
                    ));
                    self.degrade("call", &values, range)
                }
                None => {
                    self.diagnostics.push(Diagnostic::warn(
                        "unresolved-name",
                        "this binding has no value yet",
                        range,
                    ));
                    self.degrade("call", &values, range)
                }
            },
            hir::Callee::Unknown(name) => {
                self.diagnostics.push(Diagnostic::warn(
                    "unknown-function",
                    format!("unknown function `{name}`"),
                    range,
                ));
                self.degrade(name, &values, range)
            }
        }
    }

    fn call_element(
        &mut self,
        element: CoreElement,
        plan: &CallPlan,
        values: &[Value],
        range: TextRange,
    ) -> Value {
        if matches!(plan, CallPlan::Failed) {
            return self.degrade(element.as_str(), values, range);
        }
        let signature = registry::signature_of(element);
        let params = signature.hir_params(range);
        let Some(bound) = self.assemble(&params, plan, values, range, element.as_str()) else {
            return self.degrade(element.as_str(), values, range);
        };
        let args = params
            .iter()
            .zip(bound)
            .filter_map(|(param, value)| value.map(|value| (param.name.clone(), value)))
            .collect();
        Value::Content(Content {
            items: vec![Item {
                name: ElementName::Core(element),
                args,
                flow: signature.flow,
                state: ItemState::Resolved,
                range,
            }],
        })
    }

    fn call_registered(
        &mut self,
        function: &'static CoreFunction,
        plan: &CallPlan,
        args: &[hir::Arg],
        values: &[Value],
        range: TextRange,
    ) -> Value {
        if matches!(plan, CallPlan::Failed) {
            return self.degrade(function.name, values, range);
        }
        let (positional, named) = match function.variadic {
            Some(Variadic::Positional) => {
                let mut collected = Vec::new();
                for (arg, value) in args.iter().zip(values) {
                    match &arg.label {
                        ArgLabel::Positional => collected.push(value.clone()),
                        ArgLabel::Named(name) => {
                            self.diagnostics.push(Diagnostic::warn(
                                "unknown-argument",
                                format!("`{}` takes positional arguments, found `{name}`", function.name),
                                arg.range,
                            ));
                        }
                    }
                }
                (collected, Vec::new())
            }
            Some(Variadic::Named) => {
                let mut collected = Vec::new();
                for (arg, value) in args.iter().zip(values) {
                    match &arg.label {
                        ArgLabel::Named(name) => collected.push((name.clone(), value.clone())),
                        ArgLabel::Positional => {
                            self.diagnostics.push(Diagnostic::warn(
                                "unknown-argument",
                                format!("`{}` takes named arguments", function.name),
                                arg.range,
                            ));
                        }
                    }
                }
                (Vec::new(), collected)
            }
            None => {
                let params = function.hir_params(range);
                let Some(bound) = self.assemble(&params, plan, values, range, function.name) else {
                    return self.degrade(function.name, values, range);
                };
                (
                    bound.into_iter().flatten().collect::<Vec<Value>>(),
                    Vec::new(),
                )
            }
        };
        match (function.call)(&positional, &named) {
            Ok(value) => value,
            Err(message) => {
                self.diagnostics.push(Diagnostic::warn(
                    "core-function-error",
                    format!("`{}`: {message}", function.name),
                    range,
                ));
                self.degrade(function.name, values, range)
            }
        }
    }

    fn call_function(
        &mut self,
        self_binding: Option<BindingId>,
        function: &FunctionValue,
        plan: &CallPlan,
        args: &[hir::Arg],
        values: &[Value],
        range: TextRange,
    ) -> Value {
        let FunctionBody::User {
            body,
            param_bindings,
        } = &function.body
        else {
            self.diagnostics.push(Diagnostic::warn(
                "implementation-missing",
                "this function is declared but has no implementation",
                range,
            ));
            return self.degrade("call", values, range);
        };
        if matches!(plan, CallPlan::Failed) {
            return self.degrade("call", values, range);
        }

        let params = &function.signature.params;
        // 高阶调用在解析期拿不到签名，这里现场装配；规则与解析期同一份实现。
        let (mapping, check) = match plan {
            CallPlan::Bound { mapping, check } => (mapping.clone(), check.clone()),
            _ => {
                let (mapping, diagnostics) = resolve::plan_mapping(&function.signature, args, "call");
                self.diagnostics.extend(diagnostics);
                let check = params
                    .iter()
                    .enumerate()
                    .filter_map(|(index, param)| param.ty.map(|ty| (index, ty)))
                    .collect();
                (mapping, check)
            }
        };

        // 闭包环境：先放捕获，再放函数自身（递归调用靠它），
        // 最后按形参顺序放实参，默认值在定义点环境求值。
        let mut environment = Environment::default();
        for (binding, value) in &function.captures {
            environment.define(*binding, value.clone());
        }
        if let Some(binding) = self_binding {
            environment.define(binding, Value::Function(Box::new(function.clone())));
        }
        for (index, param) in params.iter().enumerate() {
            let value = match mapping.get(index).copied().flatten() {
                Some(argument) => match param.ty {
                    Some(expected) => match coerce(expected, values[argument].clone()) {
                        Some(value) => value,
                        None => {
                            if check.iter().any(|(checked, _)| *checked == index) {
                                self.diagnostics.push(Diagnostic::warn(
                                    "type-mismatch",
                                    format!(
                                        "argument `{}` expects {}, found {}",
                                        param.name,
                                        expected.as_str(),
                                        values[argument].type_name()
                                    ),
                                    args[argument].range,
                                ));
                            }
                            return self.degrade("call", values, range);
                        }
                    },
                    None => values[argument].clone(),
                },
                None => match &param.default {
                    Some(default) => self.eval(default, &mut environment),
                    None => continue,
                },
            };
            if let Some(binding) = param_bindings.get(index) {
                environment.define(*binding, value);
            }
        }

        let value = self.eval(body, &mut environment);
        if let Some(expected) = function.signature.returns
            && !matches_type(expected, &value)
        {
            self.diagnostics.push(Diagnostic::warn(
                "return-type-mismatch",
                format!(
                    "function returns {}, expected {}",
                    value.type_name(),
                    expected.as_str()
                ),
                range,
            ));
        }
        value
    }

    /// 用解析期算好的映射把实参装到形参上；缺省的可选形参不出现。
    fn assemble(
        &mut self,
        params: &[hir::Param],
        plan: &CallPlan,
        values: &[Value],
        range: TextRange,
        label: &str,
    ) -> Option<Vec<Option<Value>>> {
        let CallPlan::Bound { mapping, check } = plan else {
            return Some(vec![None; params.len()]);
        };
        let mut bound: Vec<Option<Value>> = vec![None; params.len()];
        for (index, param) in params.iter().enumerate() {
            let Some(argument) = mapping.get(index).copied().flatten() else {
                continue;
            };
            let value = values[argument].clone();
            match param.ty {
                Some(expected) => match coerce(expected, value) {
                    Some(value) => bound[index] = Some(value),
                    None => {
                        if check.iter().any(|(checked, _)| *checked == index) {
                            self.diagnostics.push(Diagnostic::warn(
                                "type-mismatch",
                                format!(
                                    "argument `{}` of `{label}` expects {}, found {}",
                                    param.name,
                                    expected.as_str(),
                                    values[argument].type_name()
                                ),
                                range,
                            ));
                        }
                        return None;
                    }
                },
                None => bound[index] = Some(value),
            }
        }
        Some(bound)
    }

    /// 降级产物：保留名字与已归约的实参，只标记状态。
    fn degrade(&self, name: &str, provided: &[Value], range: TextRange) -> Value {
        let has_content = provided
            .iter()
            .any(|value| matches!(value, Value::Content(content) if !content.is_empty()));
        Value::Content(Content {
            items: vec![Item {
                name: ElementName::Unknown(name.to_owned()),
                args: provided
                    .iter()
                    .enumerate()
                    .map(|(index, value)| (format!("#{index}"), value.clone()))
                    .collect(),
                flow: if has_content {
                    Flow::Standalone
                } else {
                    Flow::Inline
                },
                state: ItemState::Degraded,
                range,
            }],
        })
    }
}

/// 类型是否接受：唯一的 coercion 是 Int 到 Float。
fn coerce(expected: Type, value: Value) -> Option<Value> {
    if matches_type(expected, &value) {
        return Some(value);
    }
    match (expected, value) {
        (Type::Float, Value::Int(integer)) => Some(Value::Float(integer as f64)),
        _ => None,
    }
}

fn matches_type(expected: Type, value: &Value) -> bool {
    matches!(
        (expected, value),
        (Type::Unit, Value::Unit)
            | (Type::Bool, Value::Bool(_))
            | (Type::Int, Value::Int(_))
            | (Type::Float, Value::Float(_))
            | (Type::Float, Value::Int(_))
            | (Type::String, Value::String(_))
            | (Type::Content, Value::Content(_))
            | (Type::Function, Value::Function(_))
            | (Type::Array, Value::Array(_))
            | (Type::Dict, Value::Dict(_))
    )
}
