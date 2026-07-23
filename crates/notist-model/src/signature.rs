use std::fmt;

/// A static type understood by the Notist type checker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Type {
    /// The absence of a value.
    None,
    /// A boolean value.
    Bool,
    /// A signed integer value.
    Int,
    /// A floating-point value.
    Float,
    /// A UTF-8 string value.
    String,
    /// Evaluated Notist content.
    Content,
    /// Either `none` or a value of the nested type.
    Optional(Box<Type>),
}

impl Type {
    /// Returns whether a value of the `actual` type can bind to this type.
    pub fn accepts(&self, actual: &Self) -> bool {
        self == actual
            || matches!(self, Self::Optional(inner) if actual == &Self::None || inner.accepts(actual))
            || matches!((self, actual), (Self::Float, Self::Int))
    }
}

impl fmt::Display for Type {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("None"),
            Self::Bool => formatter.write_str("Bool"),
            Self::Int => formatter.write_str("Int"),
            Self::Float => formatter.write_str("Float"),
            Self::String => formatter.write_str("String"),
            Self::Content => formatter.write_str("Content"),
            Self::Optional(inner) => write!(formatter, "{inner}?"),
        }
    }
}

/// A literal default value declared by a function signature.
#[derive(Clone, Debug, PartialEq)]
pub enum DefaultValue {
    /// The `none` value.
    None,
    /// A boolean default.
    Bool(bool),
    /// A signed integer default.
    Int(i64),
    /// A floating-point default.
    Float(f64),
    /// A string default.
    String(&'static str),
}

impl DefaultValue {
    /// Returns the static type of this default value.
    pub fn ty(&self) -> Type {
        match self {
            Self::None => Type::None,
            Self::Bool(_) => Type::Bool,
            Self::Int(_) => Type::Int,
            Self::Float(_) => Type::Float,
            Self::String(_) => Type::String,
        }
    }
}

/// One declared function parameter.
#[derive(Clone, Debug, PartialEq)]
pub struct Parameter {
    /// The parameter name.
    pub name: &'static str,
    /// The static type the parameter accepts.
    pub ty: Type,
    /// The default used when the argument is omitted.
    pub default: Option<DefaultValue>,
}

/// The statically checkable signature of a function.
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionSignature {
    /// Positional and named parameters in declaration order.
    pub parameters: Vec<Parameter>,
    /// The parameter bound by trailing Content literals.
    pub trailing_content: Option<&'static str>,
    /// The declared result type.
    pub result: Type,
}

/// The signature of the built-in `text` function.
pub fn text_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![Parameter {
            name: "value",
            ty: Type::String,
            default: None,
        }],
        trailing_content: None,
        result: Type::Content,
    }
}

/// The signature of the built-in `ref` function.
pub fn ref_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![Parameter {
            name: "target",
            ty: Type::String,
            default: None,
        }],
        trailing_content: None,
        result: Type::Content,
    }
}

/// The signature of the built-in `heading` function.
pub fn heading_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "level",
                ty: Type::Int,
                default: Some(DefaultValue::Int(1)),
            },
            Parameter {
                name: "body",
                ty: Type::Content,
                default: None,
            },
        ],
        trailing_content: Some("body"),
        result: Type::Content,
    }
}

/// The signature of the built-in `outline` function.
pub fn outline_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![Parameter {
            name: "depth",
            ty: Type::Int,
            default: Some(DefaultValue::Int(3)),
        }],
        trailing_content: None,
        result: Type::Content,
    }
}

/// The signature of the built-in `raw` function.
pub fn raw_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "text",
                ty: Type::String,
                default: None,
            },
            Parameter {
                name: "lang",
                ty: Type::Optional(Box::new(Type::String)),
                default: Some(DefaultValue::None),
            },
        ],
        trailing_content: None,
        result: Type::Content,
    }
}

/// The signature of the built-in `code` function.
pub fn code_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "text",
                ty: Type::String,
                default: None,
            },
            Parameter {
                name: "lang",
                ty: Type::Optional(Box::new(Type::String)),
                default: Some(DefaultValue::None),
            },
            Parameter {
                name: "block",
                ty: Type::Bool,
                default: Some(DefaultValue::Bool(false)),
            },
        ],
        trailing_content: None,
        result: Type::Content,
    }
}

/// The signature of the built-in `quote` function.
pub fn quote_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "attribution",
                ty: Type::Optional(Box::new(Type::Content)),
                default: Some(DefaultValue::None),
            },
            Parameter {
                name: "body",
                ty: Type::Content,
                default: None,
            },
        ],
        trailing_content: Some("body"),
        result: Type::Content,
    }
}

/// The signature of the built-in `callout` function.
pub fn callout_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "kind",
                ty: Type::String,
                default: Some(DefaultValue::String("note")),
            },
            Parameter {
                name: "title",
                ty: Type::Optional(Box::new(Type::Content)),
                default: Some(DefaultValue::None),
            },
            Parameter {
                name: "body",
                ty: Type::Content,
                default: None,
            },
        ],
        trailing_content: Some("body"),
        result: Type::Content,
    }
}

/// The signature of the built-in `details` function.
pub fn details_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "summary",
                ty: Type::Optional(Box::new(Type::Content)),
                default: Some(DefaultValue::None),
            },
            Parameter {
                name: "open",
                ty: Type::Bool,
                default: Some(DefaultValue::Bool(false)),
            },
            Parameter {
                name: "body",
                ty: Type::Content,
                default: None,
            },
        ],
        trailing_content: Some("body"),
        result: Type::Content,
    }
}

/// The signature shared by inline content wrappers such as `strong` and `emph`.
pub fn inline_body_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![Parameter {
            name: "body",
            ty: Type::Content,
            default: None,
        }],
        trailing_content: Some("body"),
        result: Type::Content,
    }
}

/// The signature of the built-in `math` function.
pub fn math_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "text",
                ty: Type::String,
                default: None,
            },
            Parameter {
                name: "block",
                ty: Type::Bool,
                default: Some(DefaultValue::Bool(false)),
            },
        ],
        trailing_content: None,
        result: Type::Content,
    }
}

/// The signature of the built-in `abbr` function.
pub fn abbr_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "term",
                ty: Type::String,
                default: None,
            },
            Parameter {
                name: "expansion",
                ty: Type::String,
                default: None,
            },
        ],
        trailing_content: None,
        result: Type::Content,
    }
}

/// The signature of the built-in `time` function.
pub fn time_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "datetime",
                ty: Type::String,
                default: None,
            },
            Parameter {
                name: "body",
                ty: Type::Content,
                default: None,
            },
        ],
        trailing_content: Some("body"),
        result: Type::Content,
    }
}

/// The signature of the built-in `cite` function.
pub fn cite_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "key",
                ty: Type::String,
                default: None,
            },
            Parameter {
                name: "locator",
                ty: Type::Optional(Box::new(Type::String)),
                default: Some(DefaultValue::None),
            },
        ],
        trailing_content: None,
        result: Type::Content,
    }
}

/// The signature of the built-in `link` function.
pub fn link_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "destination",
                ty: Type::String,
                default: None,
            },
            Parameter {
                name: "title",
                ty: Type::Optional(Box::new(Type::String)),
                default: Some(DefaultValue::None),
            },
            Parameter {
                name: "body",
                ty: Type::Content,
                default: None,
            },
        ],
        trailing_content: Some("body"),
        result: Type::Content,
    }
}

/// The signature of the built-in `image` function.
pub fn image_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "source",
                ty: Type::String,
                default: None,
            },
            Parameter {
                name: "alt",
                ty: Type::String,
                default: Some(DefaultValue::String("")),
            },
            Parameter {
                name: "title",
                ty: Type::Optional(Box::new(Type::String)),
                default: Some(DefaultValue::None),
            },
            Parameter {
                name: "width",
                ty: Type::Optional(Box::new(Type::Int)),
                default: Some(DefaultValue::None),
            },
            Parameter {
                name: "height",
                ty: Type::Optional(Box::new(Type::Int)),
                default: Some(DefaultValue::None),
            },
        ],
        trailing_content: None,
        result: Type::Content,
    }
}

/// The signature of the built-in `figure` function.
pub fn figure_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "source",
                ty: Type::String,
                default: None,
            },
            Parameter {
                name: "alt",
                ty: Type::String,
                default: Some(DefaultValue::String("")),
            },
            Parameter {
                name: "title",
                ty: Type::Optional(Box::new(Type::String)),
                default: Some(DefaultValue::None),
            },
            Parameter {
                name: "caption",
                ty: Type::Content,
                default: None,
            },
        ],
        trailing_content: Some("caption"),
        result: Type::Content,
    }
}

/// The signature of the built-in `video` function.
pub fn video_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "source",
                ty: Type::String,
                default: None,
            },
            Parameter {
                name: "poster",
                ty: Type::Optional(Box::new(Type::String)),
                default: Some(DefaultValue::None),
            },
            Parameter {
                name: "controls",
                ty: Type::Bool,
                default: Some(DefaultValue::Bool(true)),
            },
        ],
        trailing_content: None,
        result: Type::Content,
    }
}

/// The signature of the built-in `audio` function.
pub fn audio_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "source",
                ty: Type::String,
                default: None,
            },
            Parameter {
                name: "controls",
                ty: Type::Bool,
                default: Some(DefaultValue::Bool(true)),
            },
            Parameter {
                name: "loop",
                ty: Type::Bool,
                default: Some(DefaultValue::Bool(false)),
            },
        ],
        trailing_content: None,
        result: Type::Content,
    }
}

/// The signature shared by argument-free content constructors.
pub fn empty_content_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: Vec::new(),
        trailing_content: None,
        result: Type::Content,
    }
}

/// The signature of the built-in unordered `list::item` function.
pub fn list_item_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![Parameter {
            name: "body",
            ty: Type::Content,
            default: None,
        }],
        trailing_content: Some("body"),
        result: Type::Content,
    }
}

/// The signature shared by the built-in `list` and `enum` container functions.
pub fn list_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![Parameter {
            name: "body",
            ty: Type::Content,
            default: None,
        }],
        trailing_content: Some("body"),
        result: Type::Content,
    }
}

/// The signature of the built-in ordered `enum::item` function.
pub fn enum_item_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "value",
                ty: Type::Optional(Box::new(Type::Int)),
                default: Some(DefaultValue::None),
            },
            Parameter {
                name: "body",
                ty: Type::Content,
                default: None,
            },
        ],
        trailing_content: Some("body"),
        result: Type::Content,
    }
}

/// The signature of the built-in `terms::item` function.
pub fn terms_item_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "term",
                ty: Type::Content,
                default: None,
            },
            Parameter {
                name: "description",
                ty: Type::Content,
                default: None,
            },
        ],
        trailing_content: Some("description"),
        result: Type::Content,
    }
}

/// The signature of the built-in `task::item` function.
pub fn task_item_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "checked",
                ty: Type::Bool,
                default: Some(DefaultValue::Bool(false)),
            },
            Parameter {
                name: "body",
                ty: Type::Content,
                default: None,
            },
        ],
        trailing_content: Some("body"),
        result: Type::Content,
    }
}

/// The signature of the built-in `table::cell` function.
pub fn table_cell_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "colspan",
                ty: Type::Int,
                default: Some(DefaultValue::Int(1)),
            },
            Parameter {
                name: "rowspan",
                ty: Type::Int,
                default: Some(DefaultValue::Int(1)),
            },
            Parameter {
                name: "body",
                ty: Type::Content,
                default: None,
            },
        ],
        trailing_content: Some("body"),
        result: Type::Content,
    }
}

/// The signature of the built-in `table` function.
pub fn table_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "columns",
                ty: Type::Int,
                default: None,
            },
            Parameter {
                name: "header",
                ty: Type::Bool,
                default: Some(DefaultValue::Bool(false)),
            },
            Parameter {
                name: "align",
                ty: Type::Optional(Box::new(Type::String)),
                default: Some(DefaultValue::None),
            },
            Parameter {
                name: "caption",
                ty: Type::Optional(Box::new(Type::Content)),
                default: Some(DefaultValue::None),
            },
            Parameter {
                name: "body",
                ty: Type::Content,
                default: None,
            },
        ],
        trailing_content: Some("body"),
        result: Type::Content,
    }
}

/// The names and signatures of all built-in functions.
pub fn builtin_signatures() -> [(&'static str, FunctionSignature); 30] {
    [
        ("text", text_signature()),
        ("paragraph", inline_body_signature()),
        ("ref", ref_signature()),
        ("heading", heading_signature()),
        ("raw", raw_signature()),
        ("code", code_signature()),
        ("quote", quote_signature()),
        ("callout", callout_signature()),
        ("details", details_signature()),
        ("list", list_signature()),
        ("enum", list_signature()),
        ("list::item", list_item_signature()),
        ("enum::item", enum_item_signature()),
        ("task", list_signature()),
        ("task::item", task_item_signature()),
        ("table::cell", table_cell_signature()),
        ("table", table_signature()),
        ("strong", inline_body_signature()),
        ("emph", inline_body_signature()),
        ("strike", inline_body_signature()),
        ("underline", inline_body_signature()),
        ("kbd", inline_body_signature()),
        ("math", math_signature()),
        ("link", link_signature()),
        ("image", image_signature()),
        ("figure", figure_signature()),
        ("linebreak", empty_content_signature()),
        ("parbreak", empty_content_signature()),
        ("rule", empty_content_signature()),
        ("pagebreak", empty_content_signature()),
    ]
}
