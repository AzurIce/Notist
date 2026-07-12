use std::collections::HashMap;
use std::fmt;

use notist_model::{Content, TextRange};
use notist_syntax::{Argument, Expression, ExpressionKind};

use crate::{CallBody, EvalDiagnostic, RawSource};

/// A static type understood by the first-stage Notist type checker.
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
    /// Source text intentionally hidden from the Notist parser.
    RawSource,
    /// Either `none` or a value of the nested type.
    Optional(Box<Type>),
}

impl Type {
    fn accepts(&self, actual: &Self) -> bool {
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
            Self::RawSource => formatter.write_str("RawSource"),
            Self::Optional(inner) => write!(formatter, "{inner}?"),
        }
    }
}

/// A runtime value produced after expression evaluation.
#[derive(Clone, Debug, PartialEq)]
pub enum Value<'a> {
    /// The `none` value.
    None,
    /// A boolean value.
    Bool(bool),
    /// A signed integer value.
    Int(i64),
    /// A floating-point value.
    Float(f64),
    /// An owned string value.
    String(String),
    /// Evaluated Notist content.
    Content(Content),
    /// Borrowed raw source.
    RawSource(RawSource<'a>),
}

impl Value<'_> {
    /// Returns the static type of this value.
    pub fn ty(&self) -> Type {
        match self {
            Self::None => Type::None,
            Self::Bool(_) => Type::Bool,
            Self::Int(_) => Type::Int,
            Self::Float(_) => Type::Float,
            Self::String(_) => Type::String,
            Self::Content(_) => Type::Content,
            Self::RawSource(_) => Type::RawSource,
        }
    }
}

/// A literal default value declared by a native function signature.
#[derive(Clone, Debug, PartialEq)]
pub enum DefaultValue {
    /// The `none` value.
    None,
    /// A boolean default.
    Bool(bool),
    /// An integer default.
    Int(i64),
    /// A floating-point default.
    Float(f64),
    /// A static string default.
    String(&'static str),
}

impl DefaultValue {
    fn to_value(&self) -> Value<'static> {
        match self {
            Self::None => Value::None,
            Self::Bool(value) => Value::Bool(*value),
            Self::Int(value) => Value::Int(*value),
            Self::Float(value) => Value::Float(*value),
            Self::String(value) => Value::String((*value).to_owned()),
        }
    }
}

/// One named parameter in a function signature.
#[derive(Clone, Debug, PartialEq)]
pub struct Parameter {
    /// The parameter name used for named binding.
    pub name: &'static str,
    /// The accepted value type.
    pub ty: Type,
    /// The default value, or `None` when the parameter is required.
    pub default: Option<DefaultValue>,
}

/// The statically checkable interface of a content-producing function.
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionSignature {
    /// Parameters accepted before the trailing body.
    pub parameters: Vec<Parameter>,
    /// The required trailing body type.
    pub body: Type,
    /// The function result type.
    pub result: Type,
}

/// Arguments after positional/named binding and literal evaluation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundArguments<'a> {
    values: HashMap<&'static str, Value<'a>>,
}

impl<'a> BoundArguments<'a> {
    /// Returns a bound value by parameter name.
    pub fn get(&self, name: &str) -> Option<&Value<'a>> {
        self.values.get(name)
    }

    /// Returns a required integer after successful signature binding.
    pub fn int(&self, name: &str) -> i64 {
        match self.get(name) {
            Some(Value::Int(value)) => *value,
            _ => unreachable!("signature binding guarantees an integer value"),
        }
    }

    /// Returns an optional string after successful signature binding.
    pub fn optional_string(&self, name: &str) -> Option<&str> {
        match self.get(name) {
            Some(Value::None) => None,
            Some(Value::String(value)) => Some(value),
            _ => unreachable!("signature binding guarantees an optional string value"),
        }
    }
}

pub(crate) fn bind_arguments<'a>(
    signature: &FunctionSignature,
    arguments: &[Argument],
    body: &CallBody<'a>,
    call_name_range: TextRange,
    body_range: TextRange,
    base_offset: usize,
) -> Result<BoundArguments<'a>, Vec<EvalDiagnostic>> {
    let mut diagnostics = Vec::new();
    let mut values = HashMap::new();
    let mut positional_index = 0usize;
    let mut saw_named = false;

    for argument in arguments {
        let parameter = if let Some(name) = &argument.name {
            saw_named = true;
            signature
                .parameters
                .iter()
                .find(|parameter| parameter.name == name.value)
                .or_else(|| {
                    diagnostics.push(EvalDiagnostic {
                        message: format!("unknown argument `{}`", name.value),
                        range: name.range.shifted(base_offset),
                    });
                    None
                })
        } else if saw_named {
            diagnostics.push(EvalDiagnostic {
                message: "positional arguments cannot follow named arguments".into(),
                range: argument.range.shifted(base_offset),
            });
            None
        } else {
            let parameter = signature.parameters.get(positional_index);
            positional_index += 1;
            if parameter.is_none() {
                diagnostics.push(EvalDiagnostic {
                    message: "too many positional arguments".into(),
                    range: argument.range.shifted(base_offset),
                });
            }
            parameter
        };

        let Some(parameter) = parameter else {
            continue;
        };
        if values.contains_key(parameter.name) {
            diagnostics.push(EvalDiagnostic {
                message: format!("argument `{}` was provided more than once", parameter.name),
                range: argument.range.shifted(base_offset),
            });
            continue;
        }
        let value = evaluate_literal(&argument.expression);
        let actual = value.ty();
        if !parameter.ty.accepts(&actual) {
            diagnostics.push(EvalDiagnostic {
                message: format!(
                    "type mismatch for argument `{}`: expected {}, found {}",
                    parameter.name, parameter.ty, actual
                ),
                range: argument.expression.range.shifted(base_offset),
            });
            continue;
        }
        values.insert(parameter.name, value);
    }

    for parameter in &signature.parameters {
        if values.contains_key(parameter.name) {
            continue;
        }
        if let Some(default) = &parameter.default {
            values.insert(parameter.name, default.to_value());
        } else {
            diagnostics.push(EvalDiagnostic {
                message: format!("missing required argument `{}`", parameter.name),
                range: call_name_range.shifted(base_offset),
            });
        }
    }

    let actual_body = match body {
        CallBody::Content(_) => Type::Content,
        CallBody::Raw(_) => Type::RawSource,
    };
    if !signature.body.accepts(&actual_body) {
        diagnostics.push(EvalDiagnostic {
            message: format!(
                "body type mismatch: expected {}, found {}",
                signature.body, actual_body
            ),
            range: body_range.shifted(base_offset),
        });
    }

    if diagnostics.is_empty() {
        Ok(BoundArguments { values })
    } else {
        Err(diagnostics)
    }
}

fn evaluate_literal(expression: &Expression) -> Value<'static> {
    match &expression.kind {
        ExpressionKind::None => Value::None,
        ExpressionKind::Bool(value) => Value::Bool(*value),
        ExpressionKind::Int(value) => Value::Int(*value),
        ExpressionKind::Float(value) => Value::Float(*value),
        ExpressionKind::String(value) => Value::String(value.clone()),
    }
}

#[cfg(test)]
mod tests {
    use notist_syntax::parse;

    use super::*;

    #[test]
    fn binds_named_literals_defaults_and_reports_type_errors() {
        let parsed = parse("#test(count=2, label=\"ok\")[body]");
        let call = &parsed.calls[0];
        let signature = FunctionSignature {
            parameters: vec![
                Parameter {
                    name: "count",
                    ty: Type::Int,
                    default: None,
                },
                Parameter {
                    name: "label",
                    ty: Type::Optional(Box::new(Type::String)),
                    default: Some(DefaultValue::None),
                },
            ],
            body: Type::Content,
            result: Type::Content,
        };
        let body = CallBody::Content(Content::new());
        let bound = bind_arguments(
            &signature,
            &call.arguments,
            &body,
            call.name.range,
            call.body_range,
            0,
        )
        .unwrap();
        assert_eq!(bound.int("count"), 2);
        assert_eq!(bound.optional_string("label"), Some("ok"));

        let parsed = parse("#test(count=\"two\")[body]");
        let call = &parsed.calls[0];
        let diagnostics = bind_arguments(
            &signature,
            &call.arguments,
            &body,
            call.name.range,
            call.body_range,
            0,
        )
        .unwrap_err();
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("expected Int, found String"))
        );
    }
}
