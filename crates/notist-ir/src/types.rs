//! 静态类型：签名标注与调用检查用的类型词汇。
//!
//! 类型与值域一一对应，不新增类型构造；签名位上出现 `None` 表示该位不检查。

/// 一个类型拼写。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    Unit,
    Bool,
    Int,
    Float,
    String,
    Content,
    Function,
    Array,
    Dict,
    Target,
}

impl Type {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unit => "Unit",
            Self::Bool => "Bool",
            Self::Int => "Int",
            Self::Float => "Float",
            Self::String => "String",
            Self::Content => "Content",
            Self::Function => "Function",
            Self::Array => "Array",
            Self::Dict => "Dict",
            Self::Target => "Target",
        }
    }

    /// 标注拼写解析；未知拼写返回 `None`，由调用方给出诊断。
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "Unit" => Self::Unit,
            "Bool" => Self::Bool,
            "Int" => Self::Int,
            "Float" => Self::Float,
            "String" => Self::String,
            "Content" => Self::Content,
            "Function" => Self::Function,
            "Array" => Self::Array,
            "Dict" => Self::Dict,
            "Target" => Self::Target,
            _ => return None,
        })
    }
}
