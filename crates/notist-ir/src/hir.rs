//! 意图层：名字已解析成绑定 id，调用绑定结论已挂载。
//!
//! 本层不再出现书写名字，也不在求值期做绑定与类型判断——那些结论在
//! `crate::resolve` 与 `crate::reduce` 的绑定装配里各算一次。

use notist_model::TextRange;

use crate::ir::{CoreElement, Diagnostic, Value};
use crate::registry::CoreFunction;
use crate::types::Type;

/// 一个模块的意图层产物。
pub struct Module {
    pub body: Vec<Statement>,
    /// 绑定表，按定义顺序；id 即下标。
    pub bindings: Vec<Binding>,
    /// 解析与绑定阶段产出的诊断，随产物一起交给消费者。
    pub diagnostics: Vec<Diagnostic>,
}

/// 绑定身份。解析期分配，作用域退出后不再复用。
pub type BindingId = u32;

/// 一条绑定的静态面。
pub struct Binding {
    pub name: String,
    pub kind: BindingKind,
    pub range: TextRange,
}

/// 绑定的静态形态：值，或可调用（带签名的函数）。
#[derive(Clone, Debug, PartialEq)]
pub enum BindingKind {
    /// 值绑定；`ty` 是解析期能判定的静态类型。
    Value { ty: Option<Type> },
    /// 函数绑定：签名在解析期已知，只有声明的实现由外部提供。
    Function {
        signature: Signature,
        declared_only: bool,
    },
}

/// 一个可调用者的静态签名。
#[derive(Clone, Debug, PartialEq)]
pub struct Signature {
    pub params: Vec<Param>,
    /// 返回类型标注；省略即不检查。
    pub returns: Option<Type>,
    /// 变参：收集剩余位置实参为 Array，或全部具名实参为 Dict。
    pub variadic: Option<Variadic>,
    /// 接收尾随体的形参下标。
    pub trailing: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variadic {
    Positional,
    Named,
}

/// 一条语句。
#[derive(Clone, Debug, PartialEq)]
pub enum Statement {
    Bind {
        binding: BindingId,
        value: Expr,
        range: TextRange,
    },
    Extern {
        binding: BindingId,
        range: TextRange,
    },
    Value {
        expr: Expr,
        range: TextRange,
    },
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
    pub ty: Option<Type>,
    /// 没有默认值且不可省时为真。
    pub required: bool,
    pub default: Option<Expr>,
    pub range: TextRange,
}

/// 表达式。
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Literal {
        value: Value,
        range: TextRange,
    },
    Local {
        binding: BindingId,
        range: TextRange,
    },
    /// 解析不到的名字：求值时给出诊断并降级为 Unit。
    Unknown {
        name: String,
        range: TextRange,
    },
    Content {
        body: Vec<Statement>,
        range: TextRange,
    },
    Block {
        body: Vec<Statement>,
        range: TextRange,
    },
    /// 闭包：`captures` 是跨函数边界的自由变量，按值快照；调用期靠 `param_bindings` 把实参放回环境。
    Lambda {
        params: Vec<Param>,
        param_bindings: Vec<BindingId>,
        returns: Option<Type>,
        body: Box<Expr>,
        captures: Vec<BindingId>,
        range: TextRange,
    },
    Call {
        callee: Callee,
        args: Vec<Arg>,
        plan: CallPlan,
        range: TextRange,
    },
    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Option<Box<Expr>>,
        range: TextRange,
    },
    Unary {
        op: crate::ast::UnaryOp,
        operand: Box<Expr>,
        range: TextRange,
    },
    Binary {
        op: crate::ast::BinaryOp,
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
            | Self::Unknown { range, .. }
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

/// 被调者身份：解析的结论，只算一次。
#[derive(Clone, Debug)]
pub enum Callee {
    /// 糖钉住的 core 元素。
    Element(CoreElement),
    /// core 提供的函数。
    Function(&'static CoreFunction),
    /// 词法环境里的函数绑定。
    Local(BindingId),
    /// 解析不到的名字。
    Unknown(String),
}

impl PartialEq for Callee {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Element(left), Self::Element(right)) => left == right,
            (Self::Function(left), Self::Function(right)) => left.name == right.name,
            (Self::Local(left), Self::Local(right)) => left == right,
            (Self::Unknown(left), Self::Unknown(right)) => left == right,
            _ => false,
        }
    }
}

/// 一个实参：标签与已解析的表达式。
#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    pub label: crate::ast::ArgLabel,
    pub value: Expr,
    pub range: TextRange,
}

/// 调用计划：绑定装配的结论。
#[derive(Clone, Debug, PartialEq)]
pub enum CallPlan {
    /// 签名在解析期已知：形参 → 实参 的映射已定。
    ///
    /// `check` 是需要在求值期按值兜底检查的形参：静态判不出实参类型时带上它的标注类型。
    Bound {
        mapping: Vec<Option<usize>>,
        check: Vec<(usize, Type)>,
    },
    /// core 变参函数：位置实参整体或全部具名实参整体交给实现。
    Variadic(Variadic),
    /// 签名要等求值期拿到函数值才知道（高阶调用）。
    Deferred,
    /// 解析期已判定绑定失败；求值期直接降级，不重复诊断。
    Failed,
}
