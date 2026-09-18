//! 名字解析与调用绑定：语法层到意图层的一段。
//!
//! 三件事在这里一次算完：名字指向哪个绑定（遮蔽在此定死）、闭包捕获哪些自由变量、
//! 每个调用点的实参怎么落到形参上（含静态类型结论）。求值期只执行这些结论。

use notist_model::TextRange;

use crate::ast;
use crate::hir::{self, BindingId, CallPlan, Signature};
use crate::ir::{Diagnostic, Value};
use crate::registry;
use crate::types::Type;

/// 解析一个模块：产出意图层产物与解析期诊断。
pub fn resolve(module: ast::Module) -> hir::Module {
    let mut resolver = Resolver {
        bindings: Vec::new(),
        scopes: vec![Vec::new()],
        frames: Vec::new(),
        diagnostics: Vec::new(),
    };
    let body = resolver.statements(&module.body);
    hir::Module {
        body,
        bindings: resolver.bindings,
        diagnostics: resolver.diagnostics,
    }
}

struct Resolver {
    bindings: Vec<hir::Binding>,
    /// 作用域栈，索引 0 是模块根。
    scopes: Vec<Vec<(String, BindingId)>>,
    /// 函数边界栈，用于收集闭包捕获。
    frames: Vec<Frame>,
    diagnostics: Vec<Diagnostic>,
}

struct Frame {
    /// 进入函数体前的作用域深度：绑定的作用域索引小于它即为自由变量。
    base_depth: usize,
    captures: Vec<BindingId>,
}

impl Resolver {
    // ---- 语句 ----

    fn statements(&mut self, statements: &[ast::Statement]) -> Vec<hir::Statement> {
        let mut resolved = Vec::new();
        for statement in statements {
            if let Some(statement) = self.statement(statement) {
                resolved.push(statement);
            }
        }
        resolved
    }

    fn statement(&mut self, statement: &ast::Statement) -> Option<hir::Statement> {
        match statement {
            ast::Statement::Bind {
                name,
                recursive,
                value,
                range,
            } => {
                // 递归绑定先占名字（并预置签名）再解析值，名字在自身值内可见。
                let placeholder = recursive.then(|| {
                    let binding = self.declare(name, *range);
                    if let ast::Expr::Lambda { params, returns, .. } = value {
                        let signature = self.signature_of_params(params, *returns);
                        self.bindings[binding as usize].kind = hir::BindingKind::Function {
                            signature,
                            declared_only: false,
                        };
                    }
                    binding
                });
                let value = self.expr(value);
                let kind = self.binding_kind(&value);
                let binding = match placeholder {
                    Some(binding) => {
                        self.bindings[binding as usize].kind = kind;
                        binding
                    }
                    None => {
                        let binding = self.declare(name, *range);
                        self.bindings[binding as usize].kind = kind;
                        binding
                    }
                };
                Some(hir::Statement::Bind {
                    binding,
                    value,
                    range: *range,
                })
            }
            ast::Statement::Extern {
                name,
                params,
                returns,
                range,
            } => {
                let signature = self.signature_of_params(params, *returns);
                let binding = self.declare(name, *range);
                self.bindings[binding as usize].kind = hir::BindingKind::Function {
                    signature,
                    declared_only: true,
                };
                Some(hir::Statement::Extern {
                    binding,
                    range: *range,
                })
            }
            ast::Statement::Value { expr, range } => Some(hir::Statement::Value {
                expr: self.expr(expr),
                range: *range,
            }),
        }
    }

    /// 分配一个绑定并在当前作用域定义它。
    fn declare(&mut self, name: &str, range: TextRange) -> BindingId {
        let binding = self.bindings.len() as BindingId;
        self.bindings.push(hir::Binding {
            name: name.to_owned(),
            kind: hir::BindingKind::Value { ty: None },
            range,
        });
        if let Some(scope) = self.scopes.last_mut() {
            scope.push((name.to_owned(), binding));
        }
        binding
    }

    /// 绑定的静态形态：函数字面量带签名，其余只记静态类型。
    fn binding_kind(&self, value: &hir::Expr) -> hir::BindingKind {
        match value {
            hir::Expr::Lambda {
                params, returns, ..
            } => hir::BindingKind::Function {
                signature: Signature {
                    params: params.clone(),
                    returns: *returns,
                    variadic: None,
                    trailing: None,
                },
                declared_only: false,
            },
            other => hir::BindingKind::Value {
                ty: self.type_of(other),
            },
        }
    }

    // ---- 表达式 ----

    fn expr(&mut self, expr: &ast::Expr) -> hir::Expr {
        match expr {
            ast::Expr::Literal { value, range } => hir::Expr::Literal {
                value: value.clone(),
                range: *range,
            },
            ast::Expr::Local { name, range } => match self.lookup(name) {
                Some((scope, binding)) => {
                    self.note_capture(scope, binding);
                    hir::Expr::Local {
                        binding,
                        range: *range,
                    }
                }
                // 归属不在这里判定：求值期给出诊断并降级。
                None => hir::Expr::Unknown {
                    name: name.clone(),
                    range: *range,
                },
            },
            ast::Expr::Content { body, range } => {
                self.scopes.push(Vec::new());
                let body = self.statements(body);
                self.scopes.pop();
                hir::Expr::Content { body, range: *range }
            }
            ast::Expr::Block { body, range } => {
                self.scopes.push(Vec::new());
                let body = self.statements(body);
                self.scopes.pop();
                hir::Expr::Block { body, range: *range }
            }
            ast::Expr::Lambda {
                params,
                returns,
                body,
                range,
            } => {
                let base_depth = self.scopes.len();
                self.scopes.push(Vec::new());
                let mut resolved_params = Vec::new();
                let mut param_bindings = Vec::new();
                for param in params {
                    let binding = self.declare(&param.name, param.range);
                    self.bindings[binding as usize].kind = hir::BindingKind::Value {
                        ty: param.ty,
                    };
                    param_bindings.push(binding);
                    let default = param.default.as_ref().map(|value| self.expr(value));
                    resolved_params.push(hir::Param {
                        name: param.name.clone(),
                        ty: param.ty,
                        required: default.is_none(),
                        default,
                        range: param.range,
                    });
                }
                self.frames.push(Frame {
                    base_depth,
                    captures: Vec::new(),
                });
                let body = self.expr(body);
                let mut frame = self.frames.pop().expect("frame is pushed above");
                self.scopes.pop();
                frame.captures.sort_unstable();
                frame.captures.dedup();
                hir::Expr::Lambda {
                    params: resolved_params,
                    param_bindings,
                    returns: *returns,
                    body: Box::new(body),
                    captures: frame.captures,
                    range: *range,
                }
            }
            ast::Expr::Call {
                callee,
                args,
                trailing,
                range,
            } => self.call(callee, args, trailing.as_deref(), *range),
            ast::Expr::If {
                condition,
                then_branch,
                else_branch,
                range,
            } => hir::Expr::If {
                condition: Box::new(self.expr(condition)),
                then_branch: Box::new(self.expr(then_branch)),
                else_branch: else_branch
                    .as_ref()
                    .map(|branch| Box::new(self.expr(branch))),
                range: *range,
            },
            ast::Expr::Unary { op, operand, range } => hir::Expr::Unary {
                op: *op,
                operand: Box::new(self.expr(operand)),
                range: *range,
            },
            ast::Expr::Binary {
                op,
                left,
                right,
                range,
            } => hir::Expr::Binary {
                op: *op,
                left: Box::new(self.expr(left)),
                right: Box::new(self.expr(right)),
                range: *range,
            },
        }
    }

    // ---- 名字与捕获 ----

    fn lookup(&self, name: &str) -> Option<(usize, BindingId)> {
        for (index, scope) in self.scopes.iter().enumerate().rev() {
            if let Some((_, binding)) = scope.iter().rev().find(|(key, _)| key == name) {
                return Some((index, *binding));
            }
        }
        None
    }

    /// 跨函数边界的引用要沿途每一层函数都捕获，否则内层闭包在外层体里造不出来。
    fn note_capture(&mut self, scope: usize, binding: BindingId) {
        for frame in self.frames.iter_mut().rev() {
            if frame.base_depth > scope {
                frame.captures.push(binding);
            } else {
                break;
            }
        }
    }

    // ---- 调用 ----

    fn call(
        &mut self,
        callee: &ast::Callee,
        args: &[ast::Arg],
        trailing: Option<&ast::Expr>,
        range: TextRange,
    ) -> hir::Expr {
        let resolved_callee = self.resolve_callee(callee);
        let signature = self.signature_of(&resolved_callee, range);
        let label = callee_label(callee);

        let mut resolved_args: Vec<hir::Arg> = args
            .iter()
            .map(|arg| hir::Arg {
                label: arg.label.clone(),
                value: self.expr(&arg.value),
                range: arg.range,
            })
            .collect();
        // 尾随体收成具名实参，顺序仍在最后。
        if let Some(body) = trailing {
            let value = self.expr(body);
            let target = match &signature {
                Some(signature) => signature
                    .trailing
                    .map(|index| signature.params[index].name.clone()),
                None => None,
            };
            let range = value.range();
            match target {
                Some(name) => resolved_args.push(hir::Arg {
                    label: ast::ArgLabel::Named(name),
                    range,
                    value,
                }),
                None => {
                    self.diagnostics.push(Diagnostic::warn(
                        "unexpected-trailing-body",
                        format!("`{label}` has no trailing parameter"),
                        range,
                    ));
                    resolved_args.push(hir::Arg {
                        label: ast::ArgLabel::Positional,
                        range,
                        value,
                    });
                }
            }
        }

        let plan = match (&resolved_callee, &signature) {
            (CalleeKind::Unknown, _) => CallPlan::Deferred,
            // 高阶调用：绑定是值，签名要等求值期拿到函数值才知道。
            (CalleeKind::Local(_), None) => CallPlan::Deferred,
            (_, None) => {
                self.diagnostics.push(Diagnostic::warn(
                    "not-callable",
                    format!("`{label}` is not a function"),
                    range,
                ));
                CallPlan::Failed
            }
            (_, Some(signature)) => match signature.variadic {
                Some(variadic) => CallPlan::Variadic(variadic),
                None => self.bind_arguments(signature, &resolved_args, &label),
            },
        };

        hir::Expr::Call {
            callee: match resolved_callee {
                CalleeKind::Element(element) => hir::Callee::Element(element),
                CalleeKind::Function(function) => hir::Callee::Function(function),
                CalleeKind::Local(binding) => hir::Callee::Local(binding),
                CalleeKind::Unknown => hir::Callee::Unknown(label),
            },
            args: resolved_args,
            plan,
            range,
        }
    }

    fn resolve_callee(&mut self, callee: &ast::Callee) -> CalleeKind {
        let name = match callee {
            ast::Callee::Core(element) => return CalleeKind::Element(*element),
            ast::Callee::Named(name) => name,
        };
        if let Some((scope, binding)) = self.lookup(name) {
            self.note_capture(scope, binding);
            return CalleeKind::Local(binding);
        }
        match registry::resolve_name(name) {
            Some(registry::Resolved::Element(signature)) => CalleeKind::Element(signature.element),
            Some(registry::Resolved::Function(function)) => CalleeKind::Function(function),
            None => CalleeKind::Unknown,
        }
    }

    /// 被调者的静态签名：core 声明，或函数绑定的签名。
    fn signature_of(&self, callee: &CalleeKind, range: TextRange) -> Option<Signature> {
        match callee {
            CalleeKind::Element(element) => {
                let signature = registry::signature_of(*element);
                Some(Signature {
                    params: signature.hir_params(range),
                    returns: Some(Type::Content),
                    variadic: None,
                    trailing: signature.trailing_index(),
                })
            }
            CalleeKind::Function(function) => Some(Signature {
                params: function.hir_params(range),
                returns: function.returns,
                variadic: function.variadic,
                trailing: None,
            }),
            CalleeKind::Local(binding) => match self.bindings.get(*binding as usize) {
                Some(hir::Binding {
                    kind: hir::BindingKind::Function { signature, .. },
                    ..
                }) => Some(signature.clone()),
                _ => None,
            },
            CalleeKind::Unknown => None,
        }
    }

    /// 绑定装配 + 静态类型结论。
    fn bind_arguments(
        &mut self,
        signature: &Signature,
        args: &[hir::Arg],
        label: &str,
    ) -> CallPlan {
        let (mapping, diagnostics) = plan_mapping(signature, args, label);
        let mut failed = !diagnostics.is_empty();
        self.diagnostics.extend(diagnostics);

        // 静态判得出类型的当场查；判不出的留给求值期按值兜底。
        let mut check = Vec::new();
        for (index, param) in signature.params.iter().enumerate() {
            let Some(argument) = mapping[index] else {
                continue;
            };
            let Some(expected) = param.ty else {
                continue;
            };
            match self.type_of(&args[argument].value) {
                Some(actual) => {
                    if !accepts(expected, actual) {
                        let range = args[argument].range;
                        self.diagnostics.push(Diagnostic::warn(
                            "type-mismatch",
                            format!(
                                "argument `{}` of `{label}` expects {}, found {}",
                                param.name,
                                expected.as_str(),
                                actual.as_str()
                            ),
                            range,
                        ));
                        failed = true;
                    }
                }
                None => check.push((index, expected)),
            }
        }

        if failed {
            CallPlan::Failed
        } else {
            CallPlan::Bound { mapping, check }
        }
    }

    fn signature_of_params(&mut self, params: &[ast::Param], returns: Option<Type>) -> Signature {
        Signature {
            params: params
                .iter()
                .map(|param| {
                    let default = param.default.as_ref().map(|value| self.expr(value));
                    hir::Param {
                        name: param.name.clone(),
                        ty: param.ty,
                        required: default.is_none(),
                        default,
                        range: param.range,
                    }
                })
                .collect(),
            returns,
            variadic: None,
            trailing: None,
        }
    }

    // ---- 静态类型 ----

    fn type_of(&self, expr: &hir::Expr) -> Option<Type> {
        match expr {
            hir::Expr::Literal { value, .. } => Some(match value {
                Value::Unit => Type::Unit,
                Value::Bool(_) => Type::Bool,
                Value::Int(_) => Type::Int,
                Value::Float(_) => Type::Float,
                Value::String(_) => Type::String,
                _ => return None,
            }),
            hir::Expr::Local { binding, .. } => match &self.bindings.get(*binding as usize)?.kind {
                hir::BindingKind::Value { ty } => *ty,
                hir::BindingKind::Function { .. } => Some(Type::Function),
            },
            hir::Expr::Unknown { .. } => None,
            hir::Expr::Content { .. } => Some(Type::Content),
            hir::Expr::Block { body, .. } => body
                .iter()
                .rev()
                .find_map(|statement| match statement {
                    hir::Statement::Value { expr, .. } => Some(expr),
                    _ => None,
                })
                .and_then(|expr| self.type_of(expr)),
            hir::Expr::Lambda { .. } => Some(Type::Function),
            hir::Expr::Call { callee, .. } => match callee {
                hir::Callee::Element(_) => Some(Type::Content),
                hir::Callee::Function(function) => function.returns,
                hir::Callee::Local(binding) => match &self.bindings.get(*binding as usize)?.kind {
                    hir::BindingKind::Function { signature, .. } => signature.returns,
                    hir::BindingKind::Value { .. } => None,
                },
                hir::Callee::Unknown(_) => None,
            },
            hir::Expr::If {
                then_branch,
                else_branch,
                ..
            } => {
                let then_type = self.type_of(then_branch)?;
                match else_branch {
                    Some(branch) if self.type_of(branch) == Some(then_type) => Some(then_type),
                    Some(_) => None,
                    None => Some(then_type),
                }
            }
            hir::Expr::Unary { op, operand, .. } => match op {
                ast::UnaryOp::Negate => match self.type_of(operand)? {
                    Type::Int => Some(Type::Int),
                    Type::Float => Some(Type::Float),
                    _ => None,
                },
                ast::UnaryOp::Not => Some(Type::Bool),
            },
            hir::Expr::Binary {
                op, left, right, ..
            } => match op {
                ast::BinaryOp::Add => match (self.type_of(left)?, self.type_of(right)?) {
                    (Type::String, Type::String) => Some(Type::String),
                    (Type::Int, Type::Int) => Some(Type::Int),
                    (Type::Float, Type::Float)
                    | (Type::Float, Type::Int)
                    | (Type::Int, Type::Float) => Some(Type::Float),
                    _ => None,
                },
                ast::BinaryOp::Subtract | ast::BinaryOp::Multiply | ast::BinaryOp::Divide => {
                    match (self.type_of(left)?, self.type_of(right)?) {
                        (Type::Int, Type::Int) => Some(Type::Int),
                        (Type::Float, Type::Float)
                        | (Type::Float, Type::Int)
                        | (Type::Int, Type::Float) => Some(Type::Float),
                        _ => None,
                    }
                }
                _ => Some(Type::Bool),
            },
        }
    }
}

/// 解析出的被调者类别，装配期间用；随后折叠成 `hir::Callee`。
enum CalleeKind {
    Element(crate::ir::CoreElement),
    Function(&'static registry::CoreFunction),
    Local(BindingId),
    Unknown,
}

/// 类型是否接受：唯一的 coercion 是 Int 到 Float。
fn accepts(expected: Type, actual: Type) -> bool {
    expected == actual || (expected == Type::Float && actual == Type::Int)
}

/// 书写名字，用于诊断。
fn callee_label(callee: &ast::Callee) -> String {
    match callee {
        ast::Callee::Core(element) => element.as_str().to_owned(),
        ast::Callee::Named(name) => name.clone(),
    }
}

/// 实参落到形参上：位置填前几位，具名按名字，重复以具名胜。
///
/// 解析期与求值期共用这一份规则：前者对静态已知的签名用，后者对高阶调用拿到的签名用。
pub fn plan_mapping(
    signature: &Signature,
    args: &[hir::Arg],
    label: &str,
) -> (Vec<Option<usize>>, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    let positional: Vec<usize> = args
        .iter()
        .enumerate()
        .filter(|(_, arg)| arg.label == ast::ArgLabel::Positional)
        .map(|(index, _)| index)
        .collect();
    let named: Vec<(&str, usize)> = args
        .iter()
        .enumerate()
        .filter_map(|(index, arg)| match &arg.label {
            ast::ArgLabel::Named(name) => Some((name.as_str(), index)),
            ast::ArgLabel::Positional => None,
        })
        .collect();

    let mut mapping: Vec<Option<usize>> = vec![None; signature.params.len()];
    for (index, argument) in positional.iter().enumerate() {
        match mapping.get_mut(index) {
            Some(slot) => *slot = Some(*argument),
            None => diagnostics.push(Diagnostic::warn(
                "too-many-arguments",
                format!("`{label}` takes {} argument(s)", signature.params.len()),
                args[*argument].range,
            )),
        }
    }
    for (name, index) in &named {
        match signature
            .params
            .iter()
            .position(|param| &param.name == name)
        {
            Some(slot) => {
                if mapping[slot].is_some() {
                    diagnostics.push(Diagnostic::warn(
                        "duplicate-argument",
                        format!("argument `{name}` of `{label}` is given twice"),
                        args[*index].range,
                    ));
                }
                mapping[slot] = Some(*index);
            }
            None => diagnostics.push(Diagnostic::warn(
                "unknown-argument",
                format!("`{label}` has no parameter `{name}`"),
                args[*index].range,
            )),
        }
    }
    for (index, param) in signature.params.iter().enumerate() {
        if mapping[index].is_none() && param.required {
            let range = param.range;
            diagnostics.push(Diagnostic::warn(
                "missing-argument",
                format!("`{label}` is missing argument `{}`", param.name),
                range,
            ));
        }
    }
    (mapping, diagnostics)
}
