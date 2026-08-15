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
    /// A callable value. Its concrete signature is carried by symbol metadata
    /// (D0002); the written form is `fn(parameters) -> R` (D0007).
    Function,
    /// Either `none` or a value of the nested type.
    Optional(Box<Type>),
    /// An inferred or unchecked type. This is an internal marker, never
    /// writable in the surface grammar: it stands wherever the checker has
    /// not determined a type yet (D0002 keeps inference separate from the
    /// written type surface, R07).
    Inferred,
}

impl Type {
    /// Returns whether a value of the `actual` type can bind to this type.
    pub fn accepts(&self, actual: &Self) -> bool {
        if self == actual || matches!(actual, Self::Inferred) {
            return true;
        }
        match (self, actual) {
            (Self::Float, Self::Int) => true,
            (Self::Optional(_), Self::None) => true,
            (Self::Optional(expected), Self::Optional(actual)) => expected.accepts(actual),
            (Self::Optional(expected), actual) => expected.accepts(actual),
            // An inferred target accepts anything (D0002 static checking
            // keeps inference separate from the written surface).
            (Self::Inferred, _) => true,
            _ => false,
        }
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
            Self::Function => formatter.write_str("Function"),
            Self::Optional(inner) => write!(formatter, "{inner}?"),
            Self::Inferred => formatter.write_str("Inferred"),
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
    String(String),
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
    pub name: String,
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
    pub trailing_content: Option<String>,
    /// The declared result type.
    pub result: Type,
}

/// The signature of the built-in `ref` function.
pub fn ref_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![Parameter {
            name: "url".into(),
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
                name: "level".into(),
                ty: Type::Int,
                default: Some(DefaultValue::Int(1)),
            },
            Parameter {
                name: "body".into(),
                ty: Type::Content,
                default: None,
            },
        ],
        trailing_content: Some("body".into()),
        result: Type::Content,
    }
}

/// The signature of the built-in `raw` function.
pub fn raw_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "source".into(),
                ty: Type::String,
                default: None,
            },
            Parameter {
                name: "lang".into(),
                ty: Type::Optional(Box::new(Type::String)),
                default: Some(DefaultValue::None),
            },
            Parameter {
                name: "block".into(),
                ty: Type::Bool,
                default: Some(DefaultValue::Bool(false)),
            },
        ],
        trailing_content: None,
        result: Type::Content,
    }
}

/// The signature of the built-in `callout` function.
pub fn callout_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "kind".into(),
                ty: Type::String,
                default: Some(DefaultValue::String("note".into())),
            },
            Parameter {
                name: "title".into(),
                ty: Type::Optional(Box::new(Type::Content)),
                default: Some(DefaultValue::None),
            },
            Parameter {
                name: "body".into(),
                ty: Type::Content,
                default: None,
            },
        ],
        trailing_content: Some("body".into()),
        result: Type::Content,
    }
}

/// The signature of the built-in `details` function.
pub fn details_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "summary".into(),
                ty: Type::Optional(Box::new(Type::Content)),
                default: Some(DefaultValue::None),
            },
            Parameter {
                name: "open".into(),
                ty: Type::Bool,
                default: Some(DefaultValue::Bool(false)),
            },
            Parameter {
                name: "body".into(),
                ty: Type::Content,
                default: None,
            },
        ],
        trailing_content: Some("body".into()),
        result: Type::Content,
    }
}

/// The signature of the built-in `item` function.
pub fn item_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "ordered".into(),
                ty: Type::Bool,
                default: Some(DefaultValue::Bool(false)),
            },
            Parameter {
                name: "body".into(),
                ty: Type::Content,
                default: None,
            },
        ],
        trailing_content: Some("body".into()),
        result: Type::Content,
    }
}

/// The signature of the built-in `table-cell` function.
pub fn table_cell_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "colspan".into(),
                ty: Type::Int,
                default: Some(DefaultValue::Int(1)),
            },
            Parameter {
                name: "rowspan".into(),
                ty: Type::Int,
                default: Some(DefaultValue::Int(1)),
            },
            Parameter {
                name: "body".into(),
                ty: Type::Content,
                default: None,
            },
        ],
        trailing_content: Some("body".into()),
        result: Type::Content,
    }
}

/// The signature of the built-in `table` function.
pub fn table_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "columns".into(),
                ty: Type::Int,
                default: None,
            },
            Parameter {
                name: "header".into(),
                ty: Type::Bool,
                default: Some(DefaultValue::Bool(false)),
            },
            Parameter {
                name: "align".into(),
                ty: Type::Optional(Box::new(Type::String)),
                default: Some(DefaultValue::None),
            },
            Parameter {
                name: "body".into(),
                ty: Type::Content,
                default: None,
            },
        ],
        trailing_content: Some("body".into()),
        result: Type::Content,
    }
}

/// The signature of the built-in `figure` function (Typst-style subset).
pub fn figure_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            Parameter {
                name: "kind".into(),
                ty: Type::Optional(Box::new(Type::String)),
                default: Some(DefaultValue::None),
            },
            Parameter {
                name: "supplement".into(),
                ty: Type::Optional(Box::new(Type::Content)),
                default: Some(DefaultValue::None),
            },
            Parameter {
                name: "caption".into(),
                ty: Type::Optional(Box::new(Type::Content)),
                default: Some(DefaultValue::None),
            },
            Parameter {
                name: "body".into(),
                ty: Type::Content,
                default: None,
            },
        ],
        trailing_content: Some("body".into()),
        result: Type::Content,
    }
}

/// The signature shared by inline content wrappers such as `strong` and `emph`.
pub fn inline_body_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![Parameter {
            name: "body".into(),
            ty: Type::Content,
            default: None,
        }],
        trailing_content: Some("body".into()),
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

/// The names and signatures of all built-in functions.
pub fn builtin_signatures() -> [(&'static str, FunctionSignature); 14] {
    [
        ("ref", ref_signature()),
        ("heading", heading_signature()),
        ("raw", raw_signature()),
        ("rule", empty_content_signature()),
        ("callout", callout_signature()),
        ("details", details_signature()),
        ("item", item_signature()),
        ("table-cell", table_cell_signature()),
        ("table", table_signature()),
        ("figure", figure_signature()),
        ("strong", inline_body_signature()),
        ("emph", inline_body_signature()),
        ("underline", inline_body_signature()),
        ("strike", inline_body_signature()),
    ]
}

#[cfg(test)]
mod tests {
    use super::Type;

    #[test]
    fn scalar_and_optional_types_apply_acceptance_rules() {
        // Int coerces into Float (D0002 coercion insertion).
        assert!(Type::Float.accepts(&Type::Int));
        assert!(!Type::Int.accepts(&Type::Float));
        // T? accepts T, T? and none alike (D0002 nullable rules).
        assert!(Type::Optional(Box::new(Type::String)).accepts(&Type::None));
        assert!(Type::Optional(Box::new(Type::String)).accepts(&Type::String));
        assert!(
            Type::Optional(Box::new(Type::String))
                .accepts(&Type::Optional(Box::new(Type::String)))
        );
        assert!(!Type::Optional(Box::new(Type::Int)).accepts(&Type::String));
        // The internal inferred marker accepts anything and is accepted
        // by anything (R07: inference is separate from the written surface).
        assert!(Type::Inferred.accepts(&Type::Content));
        assert!(Type::Content.accepts(&Type::Inferred));
    }
}
