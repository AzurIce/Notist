use std::fmt;

/// A static type understood by the Notist type checker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Type {
    /// The absence of a value.
    Unit,
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
    /// An ordered, immutable collection of values. `None` is the
    /// unparameterized form (element type unconstrained); `Some` pins it.
    /// Collections bind union-covariantly without numeric coercion:
    /// `Array<Int>` accepts `Array<Int | String>` but not `Array<Float>`
    /// (C2 invariant w.r.t. coercion).
    Array(Option<Box<Type>>),
    /// An ordered mapping of keys to values; annotation payloads are Dicts.
    /// `None` parameters are the unparameterized form (unconstrained).
    Dict(Option<Box<Type>>, Option<Box<Type>>),
    /// A callable value. Its concrete signature is carried by symbol metadata
    /// (D0002); the written form is `fn(parameters) -> R` (D0007).
    Function,
    /// A reference target: a module path plus an optional module-local
    /// selector (scope id or resource file name). External urls are
    /// represented as plain `String` values, never as `Target`.
    Target,
    /// A union of alternative types: `A | B`. Unions are normalized at
    /// construction (flattened, deduplicated, sorted) and never contain
    /// another `Union` or fewer than two members.
    Union(Vec<Type>),
    /// Either `none` or a value of the nested type.
    Optional(Box<Type>),
    /// The empty type: no runtime value and not writable in the surface
    /// grammar. It is the member type of empty collection literals and is
    /// contained in every type.
    Never,
    /// An inferred or unchecked type. This is an internal marker, never
    /// writable in the surface grammar: it stands wherever the checker has
    /// not determined a type yet (D0002 keeps inference separate from the
    /// written type surface, R07).
    Inferred,
}

impl Type {
    /// Builds a normalized union type. Single-member unions collapse to the
    /// member; nested unions flatten; duplicates are removed.
    pub fn union(members: impl IntoIterator<Item = Type>) -> Type {
        let mut flattened: Vec<Type> = Vec::new();
        for member in members {
            match member {
                Type::Union(inner) => flattened.extend(inner),
                other => flattened.push(other),
            }
        }
        flattened.sort_by(|left, right| left.to_string().cmp(&right.to_string()));
        flattened.dedup();
        match flattened.len() {
            0 => Type::Unit,
            1 => flattened.pop().expect("one member"),
            _ => Type::Union(flattened),
        }
    }

    /// Returns whether a value of the `actual` type can bind to this type.
    pub fn accepts(&self, actual: &Self) -> bool {
        if self == actual || matches!(actual, Self::Inferred) {
            return true;
        }
        match (self, actual) {
            (Self::Float, Self::Int) => true,
            // A union expectation accepts any member; a union actual must be
            // wholly acceptable to the expectation.
            (Self::Union(expected), actual) => expected.iter().any(|member| member.accepts(actual)),
            (expected, Self::Union(actual)) => actual.iter().all(|member| expected.accepts(member)),
            (Self::Optional(_), Self::Unit) => true,
            (Self::Optional(expected), Self::Optional(actual)) => expected.accepts(actual),
            (Self::Optional(expected), actual) => expected.accepts(actual),
            // Collections bind union-covariantly but without numeric
            // coercion, so `Array<Int>` → `Array<Float>` is rejected (C2).
            (Self::Array(expected), Self::Array(actual)) => contains_option(expected, actual),
            (Self::Dict(expected_key, expected_value), Self::Dict(actual_key, actual_value)) => {
                contains_option(expected_key, actual_key)
                    && contains_option(expected_value, actual_value)
            }
            // An inferred target accepts anything (D0002 static checking
            // keeps inference separate from the written surface).
            (Self::Inferred, _) => true,
            _ => false,
        }
    }

    /// Returns whether every value of `inner` is also a value of `self`:
    /// the coercion-free containment relation. Numeric coercion does not
    /// apply (`Int` is not contained in `Float`); unions and parameterized
    /// collections recurse structurally.
    pub fn contains(&self, inner: &Self) -> bool {
        if self == inner
            || matches!(inner, Self::Never | Self::Inferred)
            || matches!(self, Self::Inferred)
        {
            return true;
        }
        match (self, inner) {
            (Self::Union(members), Self::Union(inner_members)) => inner_members
                .iter()
                .all(|inner_member| members.iter().any(|member| member.contains(inner_member))),
            (Self::Union(members), single) => members.iter().any(|member| member.contains(single)),
            (single, Self::Union(inner_members)) => {
                inner_members.iter().all(|member| single.contains(member))
            }
            (Self::Optional(_), Self::Unit) => true,
            (Self::Optional(outer), Self::Optional(inner)) => outer.contains(inner),
            (Self::Optional(outer), other) => outer.contains(other),
            (Self::Array(outer), Self::Array(inner)) => contains_option(outer, inner),
            (Self::Dict(outer_key, outer_value), Self::Dict(inner_key, inner_value)) => {
                contains_option(outer_key, inner_key) && contains_option(outer_value, inner_value)
            }
            _ => false,
        }
    }
}

fn contains_option(outer: &Option<Box<Type>>, inner: &Option<Box<Type>>) -> bool {
    match (outer, inner) {
        (None, _) | (_, None) => true,
        (Some(outer), Some(inner)) => outer.contains(inner),
    }
}

impl fmt::Display for Type {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unit => formatter.write_str("Unit"),
            Self::Bool => formatter.write_str("Bool"),
            Self::Int => formatter.write_str("Int"),
            Self::Float => formatter.write_str("Float"),
            Self::String => formatter.write_str("String"),
            Self::Content => formatter.write_str("Item"),
            Self::Array(None) => formatter.write_str("Array"),
            Self::Array(Some(element)) => write!(formatter, "Array<{element}>"),
            Self::Dict(None, None) => formatter.write_str("Dict"),
            Self::Dict(Some(key), Some(value)) => write!(formatter, "Dict<{key}, {value}>"),
            Self::Dict(..) => formatter.write_str("Dict"),
            Self::Function => formatter.write_str("Function"),
            Self::Target => formatter.write_str("Target"),
            // `T | Unit` reads as the optional sugar (C4).
            Self::Union(members)
                if members.len() == 2 && members.iter().any(|member| matches!(member, Type::Unit)) =>
            {
                let other = members
                    .iter()
                    .find(|member| !matches!(member, Type::Unit))
                    .expect("one non-Unit member");
                write!(formatter, "{other}?")
            }
            Self::Union(members) => {
                for (index, member) in members.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(" | ")?;
                    }
                    write!(formatter, "{member}")?;
                }
                Ok(())
            }
            Self::Optional(inner) => write!(formatter, "{inner}?"),
            Self::Never => formatter.write_str("Never"),
            Self::Inferred => formatter.write_str("Inferred"),
        }
    }
}

/// A literal default value declared by a function signature.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
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
            Self::None => Type::Unit,
            Self::Bool(_) => Type::Bool,
            Self::Int(_) => Type::Int,
            Self::Float(_) => Type::Float,
            Self::String(_) => Type::String,
        }
    }
}

impl From<bool> for DefaultValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for DefaultValue {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<f64> for DefaultValue {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<&str> for DefaultValue {
    fn from(value: &str) -> Self {
        Self::String(value.into())
    }
}

impl From<String> for DefaultValue {
    fn from(value: String) -> Self {
        Self::String(value)
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

/// The signature of the unified built-in `link` function: a vault `Target`
/// or an external url `String`.
pub fn link_signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![Parameter {
            name: "target".into(),
            ty: Type::union([Type::Target, Type::String]),
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
        ("link", link_signature()),
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
        assert!(Type::Optional(Box::new(Type::String)).accepts(&Type::Unit));
        assert!(Type::Optional(Box::new(Type::String)).accepts(&Type::String));
        assert!(
            Type::Optional(Box::new(Type::String)).accepts(&Type::Optional(Box::new(Type::String)))
        );
        assert!(!Type::Optional(Box::new(Type::Int)).accepts(&Type::String));
        // The internal inferred marker accepts anything and is accepted
        // by anything (R07: inference is separate from the written surface).
        assert!(Type::Inferred.accepts(&Type::Content));
        assert!(Type::Content.accepts(&Type::Inferred));
    }

    #[test]
    fn collections_bind_union_covariantly_without_coercion() {
        // C2 invariant w.r.t. coercion: Array<Int> does not bind to
        // Array<Float> even though Float accepts Int.
        let int_array = Type::Array(Some(Box::new(Type::Int)));
        let float_array = Type::Array(Some(Box::new(Type::Float)));
        assert!(!float_array.accepts(&int_array));
        // Union covariance: the narrower actual binds to the wider union.
        let join_array = Type::Array(Some(Box::new(Type::union([Type::Int, Type::String]))));
        assert!(join_array.accepts(&int_array));
        assert!(!int_array.accepts(&join_array));
        // Dict recurses over both parameters.
        let dict = Type::Dict(
            Some(Box::new(Type::String)),
            Some(Box::new(Type::union([Type::Int, Type::Bool]))),
        );
        assert!(dict.accepts(&Type::Dict(
            Some(Box::new(Type::String)),
            Some(Box::new(Type::Int))
        )));
        assert!(!dict.accepts(&Type::Dict(
            Some(Box::new(Type::String)),
            Some(Box::new(Type::String))
        )));
        // The unparameterized form is the unconstrained wildcard.
        assert!(Type::Array(None).accepts(&int_array));
        assert!(int_array.accepts(&Type::Array(None)));
    }

    #[test]
    fn containment_is_coercion_free_and_never_fills_everything() {
        // contains never applies numeric coercion.
        assert!(!Type::Float.contains(&Type::Int));
        assert!(Type::Int.contains(&Type::Int));
        // Unions contain their members; a union is not contained in one
        // of its members.
        let join = Type::union([Type::Int, Type::String]);
        assert!(join.contains(&Type::Int));
        assert!(!Type::Int.contains(&join));
        // Never is contained in everything; nothing but Never is contained
        // in Never.
        assert!(Type::String.contains(&Type::Never));
        assert!(!Type::Never.contains(&Type::String));
        // Dict containment recurses.
        let domain = Type::Dict(
            Some(Box::new(Type::String)),
            Some(Box::new(Type::union([Type::Int, Type::String]))),
        );
        assert!(domain.contains(&Type::Dict(
            Some(Box::new(Type::String)),
            Some(Box::new(Type::Int))
        )));
    }

    #[test]
    fn display_uses_surface_names_and_optional_sugar() {
        assert_eq!(Type::Content.to_string(), "Item");
        assert_eq!(
            Type::Array(Some(Box::new(Type::Int))).to_string(),
            "Array<Int>"
        );
        assert_eq!(Type::Array(None).to_string(), "Array");
        assert_eq!(
            Type::Dict(Some(Box::new(Type::String)), Some(Box::new(Type::Int))).to_string(),
            "Dict<String, Int>"
        );
        assert_eq!(Type::Never.to_string(), "Never");
        // `T | Unit` reads as the optional sugar (C4).
        assert_eq!(
            Type::union([Type::String, Type::Unit]).to_string(),
            "String?"
        );
        assert_eq!(
            Type::union([Type::Int, Type::String]).to_string(),
            "Int | String"
        );
    }
}
