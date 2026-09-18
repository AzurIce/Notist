//! core 标准包的声明表：元素签名、core 函数、流角色与尾随参数。
//!
//! 声明是解析期唯一需要的信息；元素实现（handler）与函数实现都不在这张表里
//! 展开，插件的声明走同一条解析路径。

use notist_model::TextRange;

use crate::hir::{self, Variadic};
use crate::ir::{CoreElement, Flow, Value};
use crate::types::Type;

/// 一个形参。`ty` 为 `None` 表示该位不检查类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Param {
    pub name: &'static str,
    pub ty: Option<Type>,
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

/// 一个 core 函数的声明。
///
/// 固定形参按签名绑定后交给实现；变参函数拿到的是收集结果。
#[derive(Clone, Copy, Debug)]
pub struct CoreFunction {
    pub name: &'static str,
    pub params: &'static [Param],
    pub variadic: Option<Variadic>,
    /// `None` 表示返回类型未知，调用点不做返回检查。
    pub returns: Option<Type>,
    pub call: fn(&[Value], &[(String, Value)]) -> Result<Value, String>,
}

impl PartialEq for CoreFunction {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

/// 名字解析的结论。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Resolved {
    Element(&'static Signature),
    Function(&'static CoreFunction),
}

const TEXT: Signature = Signature {
    element: CoreElement::Text,
    flow: Flow::Inline,
    params: &[Param {
        name: "text",
        ty: Some(Type::String),
        required: true,
    }],
    trailing: None,
};

const STRONG: Signature = Signature {
    element: CoreElement::Strong,
    flow: Flow::Inline,
    params: &[Param {
        name: "body",
        ty: Some(Type::Content),
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
            ty: Some(Type::Content),
            required: true,
        },
        Param {
            name: "body",
            ty: Some(Type::Content),
            required: true,
        },
    ],
    trailing: None,
};

const RULE: Signature = Signature {
    element: CoreElement::Rule,
    flow: Flow::Standalone,
    params: &[],
    trailing: None,
};

const RAW: Signature = Signature {
    element: CoreElement::Raw,
    flow: Flow::Inline,
    params: &[
        Param {
            name: "source",
            ty: Some(Type::String),
            required: true,
        },
        Param {
            name: "lang",
            ty: Some(Type::String),
            required: false,
        },
        Param {
            name: "block",
            ty: Some(Type::Bool),
            required: false,
        },
    ],
    trailing: None,
};

const MATH: Signature = Signature {
    element: CoreElement::Math,
    flow: Flow::Inline,
    params: &[
        Param {
            name: "source",
            ty: Some(Type::String),
            required: true,
        },
        Param {
            name: "block",
            ty: Some(Type::Bool),
            required: false,
        },
    ],
    trailing: None,
};

const LIST_ITEM: Signature = Signature {
    element: CoreElement::ListItem,
    flow: Flow::Standalone,
    params: &[
        Param {
            name: "body",
            ty: Some(Type::Content),
            required: true,
        },
        Param {
            name: "ordered",
            ty: Some(Type::Bool),
            required: false,
        },
    ],
    trailing: Some("body"),
};

const GAP: Signature = Signature {
    element: CoreElement::Gap,
    flow: Flow::Transparent,
    params: &[],
    trailing: None,
};

const TABLE: Signature = Signature {
    element: CoreElement::Table,
    flow: Flow::Standalone,
    params: &[
        Param {
            name: "columns",
            ty: Some(Type::Int),
            required: true,
        },
        Param {
            name: "align",
            ty: Some(Type::Array),
            required: true,
        },
        Param {
            name: "body",
            ty: Some(Type::Content),
            required: true,
        },
    ],
    trailing: Some("body"),
};

const TABLE_CELL: Signature = Signature {
    element: CoreElement::TableCell,
    flow: Flow::Standalone,
    params: &[
        Param {
            name: "body",
            ty: Some(Type::Content),
            required: true,
        },
        Param {
            name: "header",
            ty: Some(Type::Bool),
            required: false,
        },
        Param {
            name: "colspan",
            ty: Some(Type::Int),
            required: false,
        },
        Param {
            name: "rowspan",
            ty: Some(Type::Int),
            required: false,
        },
    ],
    trailing: Some("body"),
};

const CALLOUT: Signature = Signature {
    element: CoreElement::Callout,
    flow: Flow::Standalone,
    params: &[
        Param {
            name: "kind",
            ty: Some(Type::String),
            required: true,
        },
        Param {
            name: "title",
            ty: Some(Type::Content),
            required: false,
        },
        Param {
            name: "body",
            ty: Some(Type::Content),
            required: true,
        },
    ],
    trailing: Some("body"),
};

const DETAILS: Signature = Signature {
    element: CoreElement::Details,
    flow: Flow::Standalone,
    params: &[
        Param {
            name: "summary",
            ty: Some(Type::Content),
            required: false,
        },
        Param {
            name: "open",
            ty: Some(Type::Bool),
            required: false,
        },
        Param {
            name: "body",
            ty: Some(Type::Content),
            required: true,
        },
    ],
    trailing: Some("body"),
};

const FIGURE: Signature = Signature {
    element: CoreElement::Figure,
    flow: Flow::Standalone,
    params: &[
        Param {
            name: "caption",
            ty: Some(Type::Content),
            required: false,
        },
        Param {
            name: "body",
            ty: Some(Type::Content),
            required: true,
        },
    ],
    trailing: Some("body"),
};

static ELEMENT_SIGNATURES: &[Signature] = &[
    TEXT,
    STRONG,
    EMPH,
    UNDERLINE,
    STRIKE,
    PARBREAK,
    SECTION,
    RULE,
    RAW,
    MATH,
    LIST_ITEM,
    GAP,
    TABLE,
    TABLE_CELL,
    CALLOUT,
    DETAILS,
    FIGURE,
];

static ARRAY: CoreFunction = CoreFunction {
    name: "array",
    params: &[],
    variadic: Some(Variadic::Positional),
    returns: Some(Type::Array),
    call: |items, _| Ok(Value::Array(items.to_vec())),
};

static DICT: CoreFunction = CoreFunction {
    name: "dict",
    params: &[],
    variadic: Some(Variadic::Named),
    returns: Some(Type::Dict),
    call: |_, pairs| Ok(Value::Dict(pairs.to_vec())),
};

static AT: CoreFunction = CoreFunction {
    name: "at",
    params: &[
        Param {
            name: "value",
            ty: Some(Type::Array),
            required: true,
        },
        Param {
            name: "index",
            ty: Some(Type::Int),
            required: true,
        },
    ],
    variadic: None,
    returns: None,
    call: |values, _| match values {
        [Value::Array(items), Value::Int(index)] => {
            let index = *index as usize;
            items.get(index).cloned().ok_or_else(|| {
                format!("index {index} is out of range for an array of {}", items.len())
            })
        }
        _ => Err("`at` expects an Array and an Int".to_owned()),
    },
};

static GET: CoreFunction = CoreFunction {
    name: "get",
    params: &[
        Param {
            name: "value",
            ty: Some(Type::Dict),
            required: true,
        },
        Param {
            name: "key",
            ty: Some(Type::String),
            required: true,
        },
    ],
    variadic: None,
    returns: None,
    call: |values, _| match values {
        [Value::Dict(pairs), Value::String(key)] => Ok(pairs
            .iter()
            .rev()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
            .unwrap_or(Value::Unit)),
        _ => Err("`get` expects a Dict and a String".to_owned()),
    },
};

static LEN: CoreFunction = CoreFunction {
    name: "len",
    params: &[Param {
        name: "value",
        ty: None,
        required: true,
    }],
    variadic: None,
    returns: Some(Type::Int),
    call: |values, _| match values {
        [Value::Array(items)] => Ok(Value::Int(items.len() as i64)),
        [Value::Dict(pairs)] => Ok(Value::Int(pairs.len() as i64)),
        [Value::String(text)] => Ok(Value::Int(text.chars().count() as i64)),
        [Value::Content(content)] => Ok(Value::Int(content.items.len() as i64)),
        _ => Err("`len` expects an Array, a Dict, a String or Content".to_owned()),
    },
};

static FUNCTIONS: &[&CoreFunction] = &[&ARRAY, &DICT, &AT, &GET, &LEN];

/// 按拼写取一个元素的声明。
pub fn signature_of(element: CoreElement) -> &'static Signature {
    ELEMENT_SIGNATURES
        .iter()
        .find(|signature| signature.element == element)
        .expect("every core element has a signature")
}

impl Signature {
    /// 声明转成意图层的形参表；core 声明没有自己的区间，用调用点代替。
    pub fn hir_params(&self, range: TextRange) -> Vec<hir::Param> {
        self.params
            .iter()
            .map(|param| hir::Param {
                name: param.name.to_owned(),
                ty: param.ty,
                required: param.required,
                default: None,
                range,
            })
            .collect()
    }

    /// 尾随形参的下标。
    pub fn trailing_index(&self) -> Option<usize> {
        self.trailing
            .and_then(|name| self.params.iter().position(|param| param.name == name))
    }
}

impl CoreFunction {
    /// 声明转成意图层的形参表。
    pub fn hir_params(&self, range: TextRange) -> Vec<hir::Param> {
        self.params
            .iter()
            .map(|param| hir::Param {
                name: param.name.to_owned(),
                ty: param.ty,
                required: param.required,
                default: None,
                range,
            })
            .collect()
    }
}

/// 按书写名字解析：手写调用走这条路径，糖钉住的身份不走。
///
/// `core::` 前缀是显式寻址 core 的写法。
pub fn resolve_name(name: &str) -> Option<Resolved> {
    let local = name.strip_prefix("core::").unwrap_or(name);
    if let Some(signature) = ELEMENT_SIGNATURES
        .iter()
        .find(|signature| signature.element.as_str() == local)
    {
        return Some(Resolved::Element(signature));
    }
    FUNCTIONS
        .iter()
        .find(|function| function.name == local)
        .map(|function| Resolved::Function(function))
}
