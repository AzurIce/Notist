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

/// The signature of the built-in `quote` function.
pub fn quote_signature() -> FunctionSignature {
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

/// The names and signatures of all built-in functions.
pub fn builtin_signatures() -> [(&'static str, FunctionSignature); 3] {
    [
        ("heading", heading_signature()),
        ("raw", raw_signature()),
        ("quote", quote_signature()),
    ]
}
