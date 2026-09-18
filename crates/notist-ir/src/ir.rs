//! 物质层：一切消费者读的完成态词汇。
//!
//! 本文件的类型只描述「已经求值完成」的世界：表达式不是值，未规约态不进值域。
//! 意图层（`crate::hir`）有自己的一套类型，两者不共用一个节点表示。

use notist_model::TextRange;

use crate::hir::{BindingId, Expr, Signature};

/// 元素身份。
///
/// core 之外的来源与未知名字都保留原拼写，作为可见兜底的身份。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ElementName {
    Core(CoreElement),
    Unknown(String),
}

impl std::fmt::Display for ElementName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Core(element) => formatter.write_str(element.as_str()),
            Self::Unknown(name) => formatter.write_str(name),
        }
    }
}

/// core 标准包的元素。糖钉住的就是这些身份。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CoreElement {
    Text,
    Strong,
    Emph,
    Underline,
    Strike,
    Parbreak,
    Section,
    Rule,
    Raw,
    Math,
    ListItem,
    Gap,
    Table,
    TableCell,
    Callout,
    Details,
    Figure,
}

impl CoreElement {
    /// 元素名，与 `plugins/core` 的元素表一致。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Strong => "strong",
            Self::Emph => "emph",
            Self::Underline => "underline",
            Self::Strike => "strike",
            Self::Parbreak => "parbreak",
            Self::Section => "section",
            Self::Rule => "rule",
            Self::Raw => "raw",
            Self::Math => "math",
            Self::ListItem => "list-item",
            Self::Gap => "gap",
            Self::Table => "table",
            Self::TableCell => "table-cell",
            Self::Callout => "callout",
            Self::Details => "details",
            Self::Figure => "figure",
        }
    }
}

/// 元素在流组织里的角色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// 参与行内流。
    Inline,
    /// 自成一段并切断行内流。
    Standalone,
    /// 不产内容，也不切断流。
    Transparent,
    /// 不产内容，但切断行内流。
    Separator,
}

impl Flow {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::Standalone => "standalone",
            Self::Transparent => "transparent",
            Self::Separator => "separator",
        }
    }
}

/// 值域：只含完成态。
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Unit,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Content(Content),
    /// 闭包：签名、捕获的环境、实现（该语言写的体，或只有声明）。
    Function(Box<FunctionValue>),
    Array(Vec<Value>),
    /// 键序即书写序；同名键按最后一次取值。
    Dict(Vec<(String, Value)>),
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Unit => "Unit",
            Self::Bool(_) => "Bool",
            Self::Int(_) => "Int",
            Self::Float(_) => "Float",
            Self::String(_) => "String",
            Self::Content(_) => "Content",
            Self::Function(_) => "Function",
            Self::Array(_) => "Array",
            Self::Dict(_) => "Dict",
        }
    }

    /// 默认值的相等性：浮点按位比较，容器逐项比较。
    pub fn same(&self, other: &Self) -> bool {
        self == other
    }
}

/// 函数值。
///
/// 捕获是按值快照：定义点解析出的自由变量就是闭包环境。函数体是代码，不是待求值的东西。
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionValue {
    pub signature: Signature,
    /// 捕获的自由变量：绑定 id 与快照值。
    pub captures: Vec<(BindingId, Value)>,
    pub body: FunctionBody,
}

/// 函数实现。
#[derive(Clone, Debug, PartialEq)]
pub enum FunctionBody {
    /// 该语言写出的体表达式，与形参的绑定 id。
    User {
        body: Box<Expr>,
        param_bindings: Vec<BindingId>,
    },
    /// 只有声明，实现由外部提供。
    Extern,
}

impl FunctionValue {
    /// 形参表的单行形态，用于诊断与 dump。
    pub fn signature_text(&self) -> String {
        let params = self
            .signature
            .params
            .iter()
            .map(|param| match param.ty {
                Some(ty) => format!("{}: {}", param.name, ty.as_str()),
                None => param.name.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("({params})")
    }
}

/// 已归约的元素序列。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Content {
    pub items: Vec<Item>,
}

impl Content {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// 终态元素：一个名字、一张有序实参表、一个流角色、一个来源区间。
///
/// 没有独立的 children 字段：体是声明为尾随参数的具名实参。
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub name: ElementName,
    pub args: Vec<(String, Value)>,
    pub flow: Flow,
    pub state: ItemState,
    pub range: TextRange,
}

impl Item {
    /// 取最后一个同名实参。
    pub fn arg(&self, name: &str) -> Option<&Value> {
        self.args
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }
}

/// 元素自身的状态：绑定成功，或降级产物。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemState {
    Resolved,
    Degraded,
}

/// 诊断分级。内容问题最高 Warn，Error 只留给工具链故障。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Hint,
    Info,
    Warn,
    Error,
}

impl Severity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hint => "hint",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// 一份诊断。所有 pass 共用这一个类型。
#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub range: TextRange,
}

impl Diagnostic {
    pub fn warn(code: &'static str, message: impl Into<String>, range: TextRange) -> Self {
        Self {
            severity: Severity::Warn,
            code,
            message: message.into(),
            range,
        }
    }
}

/// 一个模块的权威产物：内容、根作用域绑定、诊断。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModuleResult {
    pub content: Content,
    /// 根作用域的绑定，按定义顺序。
    pub bindings: Vec<(String, Value)>,
    pub diagnostics: Vec<Diagnostic>,
}
