//! 严格底向上规约与降级恢复。
//!
//! 四条规则：
//! - 语句按文档序执行，`let` 顺序绑定、不可变、就近遮蔽；
//! - 实参先求值，内容表达式归约到终态，再分发本节点；
//! - 名字解析次序：词法环境 → 注册表 → 未知兜底；
//! - 任何失败都降级为可见 Item，内容不丢，只多一条 warn。

use notist_model::TextRange;

use crate::ir::{
    Content, Diagnostic, ElementName, Flow, FunctionBody, FunctionValue, Item, ItemState,
    ModuleResult, Value,
};
use crate::plan::{Callee, Expr, PlannedModule, Statement};
use crate::registry::{self, Signature, Type};

/// 把一个模块的意图层产物规约成权威产物。
pub fn reduce(planned: &PlannedModule) -> ModuleResult {
    let mut context = Context {
        diagnostics: planned.diagnostics.clone(),
    };
    let mut environment = Environment::root();
    let content = context.reduce_statements(&planned.body, &mut environment);
    ModuleResult {
        content,
        bindings: environment.root_bindings(),
        diagnostics: context.diagnostics,
    }
}

/// 词法环境：作用域栈。绑定不可变，同名遮蔽靠新绑定。
struct Environment {
    scopes: Vec<Vec<(String, Value)>>,
}

impl Environment {
    fn root() -> Self {
        Self {
            scopes: vec![Vec::new()],
        }
    }

    /// 从闭包捕获的环境开一个作用域栈：捕获一层，参数一层。
    fn from_captured(captured: &[(String, Value)]) -> Self {
        Self {
            scopes: vec![captured.to_vec(), Vec::new()],
        }
    }

    fn push(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop(&mut self) {
        self.scopes.pop();
    }

    fn define(&mut self, name: &str, value: Value) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.push((name.to_owned(), value));
        }
    }

    fn get(&self, name: &str) -> Option<&Value> {
        self.scopes.iter().rev().find_map(|scope| {
            scope
                .iter()
                .rev()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value)
        })
    }

    /// 定义点可见的全部绑定，按顺序、同名的取最近一次。
    fn visible(&self) -> Vec<(String, Value)> {
        let mut visible: Vec<(String, Value)> = Vec::new();
        for scope in &self.scopes {
            for (name, value) in scope {
                visible.retain(|(existing, _)| existing != name);
                visible.push((name.clone(), value.clone()));
            }
        }
        visible
    }

    fn root_bindings(&self) -> Vec<(String, Value)> {
        let mut bindings: Vec<(String, Value)> = Vec::new();
        for (name, value) in self.scopes.first().map(Vec::as_slice).unwrap_or_default() {
            bindings.retain(|(existing, _)| existing != name);
            bindings.push((name.clone(), value.clone()));
        }
        bindings
    }
}

struct Context {
    diagnostics: Vec<Diagnostic>,
}

impl Context {
    fn reduce_statements(
        &mut self,
        statements: &[Statement],
        environment: &mut Environment,
    ) -> Content {
        let mut items = Vec::new();
        for statement in statements {
            match statement {
                Statement::Bind { name, value, .. } => {
                    let value = self.eval(value, environment);
                    environment.define(name, value);
                }
                Statement::Function {
                    name, params, body, ..
                } => {
                    let function = FunctionValue {
                        params: params.clone(),
                        captured: environment.visible(),
                        body: match body {
                            Some(statements) => FunctionBody::User(statements.clone()),
                            None => FunctionBody::Extern,
                        },
                    };
                    environment.define(name, Value::Function(Box::new(function)));
                }
                Statement::Value { expr, range } => {
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

    fn eval(&mut self, expression: &Expr, environment: &mut Environment) -> Value {
        match expression {
            Expr::Literal { value, .. } => value.clone(),
            Expr::Local { name, range } => match environment.get(name) {
                Some(value) => value.clone(),
                None => {
                    self.diagnostics.push(Diagnostic::warn(
                        "unresolved-name",
                        format!("unresolved name `{name}`"),
                        *range,
                    ));
                    Value::Unit
                }
            },
            Expr::Content { statements, .. } => {
                environment.push();
                let content = self.reduce_statements(statements, environment);
                environment.pop();
                Value::Content(content)
            }
            Expr::Call { callee, args, range } => self.eval_call(callee, args, *range, environment),
        }
    }

    fn eval_call(
        &mut self,
        callee: &Callee,
        args: &[(String, Expr)],
        range: TextRange,
        environment: &mut Environment,
    ) -> Value {
        let mut provided: Vec<(String, Value)> = Vec::new();
        for (name, expression) in args {
            let value = self.eval(expression, environment);
            provided.push((name.clone(), value));
        }
        match callee {
            // 糖钉住的 core 身份不走名字解析。
            Callee::Core(element) => self.call_core(*element, &provided, range, None),
            Callee::Unresolved(name) => {
                // 名字解析次序：词法环境 → 注册表 → 未知兜底。
                if let Some(value) = environment.get(name).cloned() {
                    return match value {
                        Value::Function(function) => {
                            self.call_function(name, &function, &provided, range)
                        }
                        other => {
                            self.diagnostics.push(Diagnostic::warn(
                                "not-callable",
                                format!(
                                    "`{name}` is {} and cannot be called",
                                    other.type_name()
                                ),
                                range,
                            ));
                            self.degrade_value(name, &provided, range)
                        }
                    };
                }
                match registry::resolve_name(name) {
                    Some(element) => self.call_core(element, &provided, range, Some(name)),
                    None => {
                        self.diagnostics.push(Diagnostic::warn(
                            "unknown-function",
                            format!("unknown function `{name}`"),
                            range,
                        ));
                        self.degrade_value(name, &provided, range)
                    }
                }
            }
        }
    }

    /// 调用一个 core 元素。`written` 是书写名字，用于诊断。
    fn call_core(
        &mut self,
        element: crate::ir::CoreElement,
        provided: &[(String, Value)],
        range: TextRange,
        written: Option<&str>,
    ) -> Value {
        let signature = registry::signature_of(element);
        let label = written.unwrap_or(element.as_str());
        match bind(signature, provided, range, label) {
            Ok(args) => Value::Content(Content {
                items: vec![Item {
                    name: ElementName::Core(signature.element),
                    args,
                    flow: signature.flow,
                    state: ItemState::Resolved,
                    range,
                }],
            }),
            Err(diagnostics) => {
                self.diagnostics.extend(diagnostics);
                self.degrade_value(element.as_str(), provided, range)
            }
        }
    }

    /// 调用一个用户函数或只有声明的函数。
    fn call_function(
        &mut self,
        name: &str,
        function: &FunctionValue,
        provided: &[(String, Value)],
        range: TextRange,
    ) -> Value {
        let FunctionBody::User(statements) = &function.body else {
            self.diagnostics.push(Diagnostic::warn(
                "implementation-missing",
                format!("`{name}` is declared but has no implementation"),
                range,
            ));
            return self.degrade_value(name, provided, range);
        };
        let mut bound: Vec<(String, Value)> = Vec::new();
        let mut ok = true;
        for (argument, _) in provided {
            if !function.params.iter().any(|param| &param.name == argument) {
                self.diagnostics.push(Diagnostic::warn(
                    "unknown-argument",
                    format!("`{name}{}` has no parameter `{argument}`", function.signature()),
                    range,
                ));
                ok = false;
            }
        }
        for param in &function.params {
            match provided
                .iter()
                .rev()
                .find(|(argument, _)| argument == &param.name)
            {
                Some((_, value)) => bound.push((param.name.clone(), value.clone())),
                None => {
                    self.diagnostics.push(Diagnostic::warn(
                        "missing-argument",
                        format!(
                            "`{name}{}` is missing argument `{}`",
                            function.signature(),
                            param.name
                        ),
                        range,
                    ));
                    ok = false;
                }
            }
        }
        if !ok {
            return self.degrade_value(name, provided, range);
        }
        let mut environment = Environment::from_captured(&function.captured);
        for (param, value) in bound {
            environment.define(&param, value);
        }
        Value::Content(self.reduce_statements(statements, &mut environment))
    }

    fn degrade_value(&self, name: &str, provided: &[(String, Value)], range: TextRange) -> Value {
        Value::Content(Content {
            items: vec![degrade(
                ElementName::Unknown(name.to_owned()),
                provided,
                range,
            )],
        })
    }
}

/// 唯一的绑定实现：按形参表收集实参、检查类型、给出诊断。
///
/// 失败不返回半个结果，调用方降级为可见 Item。
fn bind(
    signature: &Signature,
    provided: &[(String, Value)],
    range: TextRange,
    label: &str,
) -> Result<Vec<(String, Value)>, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    for (name, _) in provided {
        if !signature.params.iter().any(|param| param.name == name) {
            diagnostics.push(Diagnostic::warn(
                "unknown-argument",
                format!("element `{label}` has no argument `{name}`"),
                range,
            ));
        }
    }
    let mut bound = Vec::new();
    for param in signature.params {
        let Some((_, value)) = provided.iter().rev().find(|(name, _)| name == param.name) else {
            if param.required {
                diagnostics.push(Diagnostic::warn(
                    "missing-argument",
                    format!("element `{label}` is missing argument `{}`", param.name),
                    range,
                ));
            }
            continue;
        };
        if !accepts(param.ty, value) {
            diagnostics.push(Diagnostic::warn(
                "type-mismatch",
                format!(
                    "argument `{}` of `{label}` expects {}, found {}",
                    param.name,
                    param.ty.as_str(),
                    value.type_name()
                ),
                range,
            ));
            continue;
        }
        bound.push((param.name.to_owned(), value.clone()));
    }
    if diagnostics.is_empty() {
        Ok(bound)
    } else {
        Err(diagnostics)
    }
}

fn accepts(ty: Type, value: &Value) -> bool {
    matches!(
        (ty, value),
        (Type::String, Value::String(_))
            | (Type::Int, Value::Int(_))
            | (Type::Bool, Value::Bool(_))
            | (Type::Content, Value::Content(_))
    )
}

/// 降级产物：保留名字、已归约的实参与内容，只标记状态。
fn degrade(name: ElementName, provided: &[(String, Value)], range: TextRange) -> Item {
    let has_content =
        provided
            .iter()
            .any(|(_, value)| matches!(value, Value::Content(content) if !content.is_empty()));
    Item {
        name,
        args: provided.to_vec(),
        flow: if has_content {
            Flow::Standalone
        } else {
            Flow::Inline
        },
        state: ItemState::Degraded,
        range,
    }
}
