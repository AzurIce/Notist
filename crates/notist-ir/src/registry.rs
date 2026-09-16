//! core 元素声明表：签名、流角色、尾随参数。
//!
//! 声明是 plan 期唯一需要的插件信息；实现（handler）不在这张表里。

use crate::ir::{CoreElement, Flow};

/// 静态类型。本切片只有元素签名用到的四个。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    String,
    Int,
    Bool,
    Content,
}

impl Type {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::String => "String",
            Self::Int => "Int",
            Self::Bool => "Bool",
            Self::Content => "Content",
        }
    }
}

/// 一个形参。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Param {
    pub name: &'static str,
    pub ty: Type,
    pub required: bool,
}

/// 一个元素的声明。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signature {
    pub element: CoreElement,
    pub flow: Flow,
    pub params: &'static [Param],
    /// 接收尾随体的形参名。
    pub trailing: Option<&'static str>,
}

const TEXT: Signature = Signature {
    element: CoreElement::Text,
    flow: Flow::Inline,
    params: &[Param {
        name: "text",
        ty: Type::String,
        required: true,
    }],
    trailing: None,
};

const STRONG: Signature = Signature {
    element: CoreElement::Strong,
    flow: Flow::Inline,
    params: &[Param {
        name: "body",
        ty: Type::Content,
        required: true,
    }],
    trailing: Some("body"),
};

const EMPH: Signature = Signature {
    element: CoreElement::Emph,
    flow: Flow::Inline,
    params: STRONG.params,
    trailing: Some("body"),
};

const UNDERLINE: Signature = Signature {
    element: CoreElement::Underline,
    flow: Flow::Inline,
    params: STRONG.params,
    trailing: Some("body"),
};

const STRIKE: Signature = Signature {
    element: CoreElement::Strike,
    flow: Flow::Inline,
    params: STRONG.params,
    trailing: Some("body"),
};

const PARBREAK: Signature = Signature {
    element: CoreElement::Parbreak,
    flow: Flow::Separator,
    params: &[],
    trailing: None,
};

const SECTION: Signature = Signature {
    element: CoreElement::Section,
    flow: Flow::Standalone,
    params: &[
        Param {
            name: "label",
            ty: Type::Content,
            required: true,
        },
        Param {
            name: "body",
            ty: Type::Content,
            required: true,
        },
    ],
    trailing: None,
};

static SIGNATURES: &[Signature] = &[TEXT, STRONG, EMPH, UNDERLINE, STRIKE, PARBREAK, SECTION];

/// 取一个 core 元素的声明。
pub fn signature_of(element: CoreElement) -> &'static Signature {
    SIGNATURES
        .iter()
        .find(|signature| signature.element == element)
        .expect("every core element has a signature")
}

/// 按书写名字解析元素：手写调用走这条路径，糖不走。
pub fn resolve_name(name: &str) -> Option<CoreElement> {
    let local = name.strip_prefix("core::").unwrap_or(name);
    SIGNATURES
        .iter()
        .find(|signature| signature.element.as_str() == local)
        .map(|signature| signature.element)
}
