//! 意图层：糖溶解、名字解析、唯一绑定结论。
//!
//! 这一层的实参是表达式而不是值。`.notc` 前端直接产出本层的语句与表达式，
//! markup 前端把片段脱糖成本层的调用；两者在 `Statement` 上汇聚。

use notist_model::TextRange;

use crate::ir::{CoreElement, Diagnostic, Value};
use crate::registry;
use crate::syntax::{Call, Inline, Literal, Piece, Surface};

/// 一个模块的意图层产物：语句序列。
pub struct PlannedModule {
    pub body: Vec<Statement>,
    pub diagnostics: Vec<Diagnostic>,
}

/// 一条语句。`let` 类语句不产内容，值语句产内容。
#[derive(Clone, Debug, PartialEq)]
pub enum Statement {
    /// 值绑定：顺序、不可变、词法作用域。
    Bind {
        name: String,
        value: Expr,
        range: TextRange,
    },
    /// 函数绑定。没有体就是只有声明，实现由外部提供。
    Function {
        name: String,
        params: Vec<Param>,
        body: Option<Vec<Statement>>,
        range: TextRange,
    },
    /// 值语句：值必须是 Content，否则只报 warn 不产内容。
    Value { expr: Expr, range: TextRange },
}

impl Statement {
    pub fn range(&self) -> TextRange {
        match self {
            Self::Bind { range, .. } | Self::Function { range, .. } | Self::Value { range, .. } => {
                *range
            }
        }
    }
}

/// 形参：名字与类型拼写。类型本切片只记录，不校验。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub ty: String,
}

/// 表达式。
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// 字面量：求值即自身。
    Literal { value: Value, range: TextRange },
    /// 词法环境里的一个名字。
    Local { name: String, range: TextRange },
    /// 内容表达式：一个语句序列归约成 Content。
    Content {
        statements: Vec<Statement>,
        range: TextRange,
    },
    /// 调用。
    Call {
        callee: Callee,
        args: Vec<(String, Expr)>,
        range: TextRange,
    },
}

impl Expr {
    pub fn range(&self) -> TextRange {
        match self {
            Self::Literal { range, .. }
            | Self::Local { range, .. }
            | Self::Content { range, .. }
            | Self::Call { range, .. } => *range,
        }
    }

    /// 单行形态，用于 dump。
    pub fn compact(&self) -> String {
        match self {
            Self::Literal { value, .. } => crate::dump::compact_value(value),
            Self::Local { name, .. } => name.clone(),
            Self::Content { statements, .. } => format!("[{} 条语句]", statements.len()),
            Self::Call { callee, args, .. } => {
                let args = args
                    .iter()
                    .map(|(name, value)| format!("{name}: {}", value.compact()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{callee}({args})")
            }
        }
    }
}

/// 被调者身份。
///
/// 糖钉住的 core 身份不走名字解析；手写调用留成书写名字，由 reduce 按
/// 「词法环境 → 注册表 → 未知兜底」解析。
#[derive(Clone, Debug, PartialEq)]
pub enum Callee {
    Core(CoreElement),
    Unresolved(String),
}

impl std::fmt::Display for Callee {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Core(element) => formatter.write_str(element.as_str()),
            Self::Unresolved(name) => formatter.write_str(name),
        }
    }
}

/// markup 前端：片段脱糖成语句。
pub fn plan(surface: &Surface) -> PlannedModule {
    PlannedModule {
        body: plan_pieces(&surface.pieces),
        diagnostics: surface.diagnostics.clone(),
    }
}

fn plan_pieces(pieces: &[Piece]) -> Vec<Statement> {
    let pieces = normalize_parbreaks(pieces);
    let mut statements = Vec::new();
    for piece in &pieces {
        match piece {
            Piece::Section {
                label, body, range, ..
            } => statements.push(call_statement(
                Callee::Core(CoreElement::Section),
                vec![
                    (
                        "label".to_owned(),
                        Expr::Content {
                            statements: plan_inlines(label),
                            range: *range,
                        },
                    ),
                    (
                        "body".to_owned(),
                        Expr::Content {
                            statements: plan_pieces(body),
                            range: *range,
                        },
                    ),
                ],
                *range,
            )),
            Piece::Run(inlines) => statements.extend(plan_inlines(inlines)),
            Piece::Parbreak { range } => statements.push(call_statement(
                Callee::Core(CoreElement::Parbreak),
                Vec::new(),
                *range,
            )),
        }
    }
    statements
}

fn plan_inlines(inlines: &[Inline]) -> Vec<Statement> {
    let mut statements = Vec::new();
    for inline in inlines {
        match inline {
            Inline::Text { value, range } => statements.push(call_statement(
                Callee::Core(CoreElement::Text),
                vec![(
                    "text".to_owned(),
                    Expr::Literal {
                        value: Value::String(value.clone()),
                        range: *range,
                    },
                )],
                *range,
            )),
            Inline::Strong { body, range } => {
                statements.push(inline_body(CoreElement::Strong, body, *range));
            }
            Inline::Emph { body, range } => {
                statements.push(inline_body(CoreElement::Emph, body, *range));
            }
            Inline::Underline { body, range } => {
                statements.push(inline_body(CoreElement::Underline, body, *range));
            }
            Inline::Strike { body, range } => {
                statements.push(inline_body(CoreElement::Strike, body, *range));
            }
            Inline::Call(call) => statements.push(call_node(call)),
        }
    }
    statements
}

fn inline_body(element: CoreElement, body: &[Inline], range: TextRange) -> Statement {
    call_statement(
        Callee::Core(element),
        vec![(
            "body".to_owned(),
            Expr::Content {
                statements: plan_inlines(body),
                range,
            },
        )],
        range,
    )
}

fn call_node(call: &Call) -> Statement {
    let callee = match registry::resolve_name(&call.name) {
        Some(element) => Callee::Core(element),
        None => Callee::Unresolved(call.name.clone()),
    };
    let mut args: Vec<(String, Expr)> = call
        .args
        .iter()
        .map(|(name, literal)| {
            (
                name.clone(),
                Expr::Literal {
                    value: literal_value(literal),
                    range: call.range,
                },
            )
        })
        .collect();
    if let Some(body) = &call.body {
        let trailing = trailing_param(&callee).unwrap_or("body");
        // 显式给了同名实参时以显式实参为准，尾随体不再覆盖。
        if !args.iter().any(|(name, _)| name == trailing) {
            args.push((
                trailing.to_owned(),
                Expr::Content {
                    statements: plan_pieces(body),
                    range: call.range,
                },
            ));
        }
    }
    call_statement(callee, args, call.range)
}

fn call_statement(callee: Callee, args: Vec<(String, Expr)>, range: TextRange) -> Statement {
    Statement::Value {
        expr: Expr::Call { callee, args, range },
        range,
    }
}

fn trailing_param(callee: &Callee) -> Option<&'static str> {
    match callee {
        Callee::Core(element) => registry::signature_of(*element).trailing,
        Callee::Unresolved(_) => None,
    }
}

fn literal_value(literal: &Literal) -> Value {
    match literal {
        Literal::Bool(value) => Value::Bool(*value),
        Literal::Int(value) => Value::Int(*value),
        Literal::String(value) => Value::String(value.clone()),
    }
}

/// 空行只在两个行内流段之间成为 parbreak。
///
/// 容器边缘的空行、连续空行、以及块级构造旁的空行都不产生节点：它们不构成段落边界。
fn normalize_parbreaks(pieces: &[Piece]) -> Vec<Piece> {
    let mut kept: Vec<Piece> = Vec::new();
    for (index, piece) in pieces.iter().enumerate() {
        let keep = !matches!(piece, Piece::Parbreak { .. })
            || (matches!(kept.last(), Some(Piece::Run(_)))
                && matches!(pieces.get(index + 1), Some(Piece::Run(_))));
        if keep {
            kept.push(piece.clone());
        }
    }
    kept
}
