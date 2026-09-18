//! Markup 脱糖：把 Markup 片段翻成语义层能走的语句序列。
//!
//! 糖钉住的 core 身份在这里定下；手写调用只留名字，走与 `.notc` 相同的解析路径。
//! 本阶段冻结：Markup 面不再演进，产物形态与 `.notc` 前端一致。

use notist_model::TextRange;

use crate::ast::{Arg, ArgLabel, Callee, Expr, Statement};
use crate::ir::{CoreElement, Value};
use crate::syntax::{Call, Inline, Literal, Piece, Surface};

/// 把一个 Markup 前端产物脱糖成语句序列。
pub fn desugar(surface: &Surface) -> Vec<Statement> {
    pieces(&surface.pieces)
}

fn pieces(source: &[Piece]) -> Vec<Statement> {
    let kept = normalize_parbreaks(source);
    let mut statements = Vec::new();
    for piece in &kept {
        match piece {
            Piece::Section {
                label, body, range, ..
            } => statements.push(call(
                Callee::Core(CoreElement::Section),
                vec![
                    named("label", content(inline_statements(label), *range), *range),
                    named("body", content(pieces(body), *range), *range),
                ],
                None,
                *range,
            )),
            Piece::Run(inlines) => statements.extend(inline_statements(inlines)),
            Piece::Parbreak { range } => statements.push(call(
                Callee::Core(CoreElement::Parbreak),
                Vec::new(),
                None,
                *range,
            )),
        }
    }
    statements
}

fn inline_statements(inlines: &[Inline]) -> Vec<Statement> {
    let mut statements = Vec::new();
    for inline in inlines {
        match inline {
            Inline::Text { value, range } => statements.push(call(
                Callee::Core(CoreElement::Text),
                vec![named(
                    "text",
                    Expr::Literal {
                        value: Value::String(value.clone()),
                        range: *range,
                    },
                    *range,
                )],
                None,
                *range,
            )),
            Inline::Strong { body, range } => {
                statements.push(body_element(CoreElement::Strong, body, *range));
            }
            Inline::Emph { body, range } => {
                statements.push(body_element(CoreElement::Emph, body, *range));
            }
            Inline::Underline { body, range } => {
                statements.push(body_element(CoreElement::Underline, body, *range));
            }
            Inline::Strike { body, range } => {
                statements.push(body_element(CoreElement::Strike, body, *range));
            }
            Inline::Call(call) => statements.push(handwritten_call(call)),
        }
    }
    statements
}

/// 行内元素：体按签名声明的尾随参数收。
fn body_element(element: CoreElement, body: &[Inline], range: TextRange) -> Statement {
    call(
        Callee::Core(element),
        Vec::new(),
        Some(content(inline_statements(body), range)),
        range,
    )
}

/// 手写调用：名字留给解析期，实参按书写形态保留。
fn handwritten_call(written: &Call) -> Statement {
    let args = written
        .args
        .iter()
        .map(|(name, literal)| {
            named(
                name,
                Expr::Literal {
                    value: literal_value(literal),
                    range: written.range,
                },
                written.range,
            )
        })
        .collect();
    let trailing = written
        .body
        .as_ref()
        .map(|body| content(pieces(body), written.range));
    call(
        Callee::Named(written.name.clone()),
        args,
        trailing,
        written.range,
    )
}

fn call(
    callee: Callee,
    args: Vec<Arg>,
    trailing: Option<Expr>,
    range: TextRange,
) -> Statement {
    Statement::Value {
        expr: Expr::Call {
            callee,
            args,
            trailing: trailing.map(Box::new),
            range,
        },
        range,
    }
}

fn named(label: &str, value: Expr, range: TextRange) -> Arg {
    Arg {
        label: ArgLabel::Named(label.to_owned()),
        value,
        range,
    }
}

fn content(statements: Vec<Statement>, range: TextRange) -> Expr {
    Expr::Content {
        body: statements,
        range,
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
