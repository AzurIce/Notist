//! 语法层：前端产物，名字尚未解析。
//!
//! 两种前端都产出本层类型：`.notc` 前端直接解析，Markup 前端把片段脱糖成本层语句。
//! 名字换成绑定 id 之后才是意图层（`crate::hir`）。

use notist_model::TextRange;

use crate::ir::{CoreElement, Value};
use crate::types::Type;

/// 一个模块的语法层产物。
pub struct Module {
    pub body: Vec<Statement>,
}

/// 一条语句。绑定与声明不产内容，值语句产内容。
#[derive(Clone, Debug, PartialEq)]
pub enum Statement {
    /// 值绑定：顺序、不可变、词法作用域。`recursive` 让名字在自身值内可见。
    Bind {
        name: String,
        recursive: bool,
        value: Expr,
        range: TextRange,
    },
    /// 只有声明的函数绑定，实现由外部提供。
    Extern {
        name: String,
        params: Vec<Param>,
        returns: Option<Type>,
        range: TextRange,
    },
    /// 值语句。
    Value { expr: Expr, range: TextRange },
}

impl Statement {
    pub fn range(&self) -> TextRange {
        match self {
            Self::Bind { range, .. } | Self::Extern { range, .. } | Self::Value { range, .. } => {
                *range
            }
        }
    }
}

/// 形参。
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    /// 省略标注即不检查。
    pub ty: Option<Type>,
    pub default: Option<Expr>,
    pub range: TextRange,
}

/// 表达式。
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// 字面量：只承载标量值。
    Literal { value: Value, range: TextRange },
    /// 词法环境里的一个名字。
    Local { name: String, range: TextRange },
    /// 内容块：语句序列，值语句的内容拼接成 Content。
    Content {
        body: Vec<Statement>,
        range: TextRange,
    },
    /// 代码块：语句序列，值是最后一条值语句的值，空块为 Unit。
    Block {
        body: Vec<Statement>,
        range: TextRange,
    },
    /// 函数字面量。`returns` 只在函数定义糖里出现。
    Lambda {
        params: Vec<Param>,
        returns: Option<Type>,
        body: Box<Expr>,
        range: TextRange,
    },
    /// 调用：位置与具名实参，可带尾随体。
    Call {
        callee: Callee,
        args: Vec<Arg>,
        trailing: Option<Box<Expr>>,
        range: TextRange,
    },
    /// 条件：两个分支都是块。
    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Option<Box<Expr>>,
        range: TextRange,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
        range: TextRange,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
        range: TextRange,
    },
}

impl Expr {
    pub fn range(&self) -> TextRange {
        match self {
            Self::Literal { range, .. }
            | Self::Local { range, .. }
            | Self::Content { range, .. }
            | Self::Block { range, .. }
            | Self::Lambda { range, .. }
            | Self::Call { range, .. }
            | Self::If { range, .. }
            | Self::Unary { range, .. }
            | Self::Binary { range, .. } => *range,
        }
    }
}

/// 被调者：糖钉住的 core 身份，或一个书写名字。
#[derive(Clone, Debug, PartialEq)]
pub enum Callee {
    Core(CoreElement),
    Named(String),
}

/// 一个实参。
#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    pub label: ArgLabel,
    pub value: Expr,
    pub range: TextRange,
}

/// 实参标签：位置或具名。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgLabel {
    Positional,
    Named(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Negate,
    Not,
}

impl UnaryOp {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Negate => "-",
            Self::Not => "not",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
}

impl BinaryOp {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Subtract => "-",
            Self::Multiply => "*",
            Self::Divide => "/",
            Self::Equal => "==",
            Self::NotEqual => "!=",
            Self::Less => "<",
            Self::LessEqual => "<=",
            Self::Greater => ">",
            Self::GreaterEqual => ">=",
            Self::And => "and",
            Self::Or => "or",
        }
    }

    /// 逻辑运算在表达式层短路，不进实参求值。
    pub const fn is_short_circuit(self) -> bool {
        matches!(self, Self::And | Self::Or)
    }
}
