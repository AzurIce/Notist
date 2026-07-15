use std::collections::HashMap;
use std::fmt;

use notist_model::{Content, TextRange};
use notist_syntax::{Argument, Expression, ExpressionKind, StringLiteralForm, StringLiteralStyle};

use crate::EvalDiagnostic;

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
            Self::Optional(inner) => write!(formatter, "{inner}?"),
        }
    }
}

/// A runtime value produced after expression evaluation.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
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
}

impl Value {
    /// Returns the static type of this value.
    pub fn ty(&self) -> Type {
        match self {
            Self::None => Type::None,
            Self::Bool(_) => Type::Bool,
            Self::Int(_) => Type::Int,
            Self::Float(_) => Type::Float,
            Self::String(_) => Type::String,
            Self::Content(_) => Type::Content,
        }
    }
}

/// Source information retained for a bound value when it came directly from syntax.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueOrigin {
    /// A signature-provided default value.
    Default,
    /// A literal expression written at the call site.
    Literal {
        /// The complete literal range in the source document.
        range: TextRange,
        /// The String payload range without prefixes or delimiters.
        payload_range: Option<TextRange>,
        /// The source form for String literals.
        string_form: Option<StringLiteralForm>,
        /// The escape behavior for String literals.
        string_style: Option<StringLiteralStyle>,
    },
    /// A trailing Content literal bound through `#name[...]` syntax.
    TrailingContent {
        /// The Content payload range.
        range: TextRange,
    },
}

/// A runtime value together with its call-site origin.
#[derive(Clone, Debug, PartialEq)]
struct BoundValue {
    value: Value,
    origin: ValueOrigin,
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
    fn to_value(&self) -> Value {
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
    /// Parameters accepted by the function.
    pub parameters: Vec<Parameter>,
    /// The Content parameter populated by trailing `[...]` syntax, when supported.
    pub trailing_content: Option<&'static str>,
    /// The function result type.
    pub result: Type,
}

/// Arguments after positional/named binding and literal evaluation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundArguments {
    values: HashMap<&'static str, BoundValue>,
}

impl BoundArguments {
    /// Returns a bound value by parameter name.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.values.get(name).map(|bound| &bound.value)
    }

    /// Returns the call-site origin retained for a bound value.
    pub fn origin(&self, name: &str) -> Option<ValueOrigin> {
        self.values.get(name).map(|bound| bound.origin)
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

    /// Returns a required String after successful signature binding.
    pub fn string(&self, name: &str) -> &str {
        match self.get(name) {
            Some(Value::String(value)) => value,
            _ => unreachable!("signature binding guarantees a String value"),
        }
    }

    /// Returns the literal source form for a directly written String argument.
    pub fn string_form(&self, name: &str) -> Option<StringLiteralForm> {
        match self.values.get(name).map(|bound| bound.origin) {
            Some(ValueOrigin::Literal {
                string_form: Some(form),
                ..
            }) => Some(form),
            _ => None,
        }
    }

    /// Removes and returns a required Content value.
    pub fn take_content(&mut self, name: &str) -> Content {
        match self.values.remove(name).map(|bound| bound.value) {
            Some(Value::Content(content)) => content,
            _ => unreachable!("signature binding guarantees a Content value"),
        }
    }
}

pub(crate) fn bind_arguments(
    signature: &FunctionSignature,
    arguments: &[Argument],
    trailing_content: Option<(Content, TextRange)>,
    call_name_range: TextRange,
    base_offset: usize,
) -> Result<BoundArguments, Vec<EvalDiagnostic>> {
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
        let (value, origin) = evaluate_literal(&argument.expression, base_offset);
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
        values.insert(parameter.name, BoundValue { value, origin });
    }

    if let Some((content, range)) = trailing_content {
        let Some(parameter_name) = signature.trailing_content else {
            diagnostics.push(EvalDiagnostic {
                message: "function does not accept trailing Content".into(),
                range: range.shifted(base_offset),
            });
            return Err(diagnostics);
        };
        let Some(parameter) = signature
            .parameters
            .iter()
            .find(|parameter| parameter.name == parameter_name)
        else {
            diagnostics.push(EvalDiagnostic {
                message: format!(
                    "invalid function signature: trailing Content parameter `{parameter_name}` does not exist"
                ),
                range: call_name_range.shifted(base_offset),
            });
            return Err(diagnostics);
        };
        if parameter.ty != Type::Content {
            diagnostics.push(EvalDiagnostic {
                message: format!(
                    "invalid function signature: trailing parameter `{parameter_name}` must have type Content"
                ),
                range: call_name_range.shifted(base_offset),
            });
        } else if values.contains_key(parameter.name) {
            diagnostics.push(EvalDiagnostic {
                message: format!("argument `{}` was provided more than once", parameter.name),
                range: range.shifted(base_offset),
            });
        } else {
            values.insert(
                parameter.name,
                BoundValue {
                    value: Value::Content(content),
                    origin: ValueOrigin::TrailingContent {
                        range: range.shifted(base_offset),
                    },
                },
            );
        }
    }

    for parameter in &signature.parameters {
        if values.contains_key(parameter.name) {
            continue;
        }
        if let Some(default) = &parameter.default {
            values.insert(
                parameter.name,
                BoundValue {
                    value: default.to_value(),
                    origin: ValueOrigin::Default,
                },
            );
        } else {
            diagnostics.push(EvalDiagnostic {
                message: format!("missing required argument `{}`", parameter.name),
                range: call_name_range.shifted(base_offset),
            });
        }
    }

    if diagnostics.is_empty() {
        Ok(BoundArguments { values })
    } else {
        Err(diagnostics)
    }
}

fn evaluate_literal(expression: &Expression, base_offset: usize) -> (Value, ValueOrigin) {
    let value = match &expression.kind {
        ExpressionKind::None => Value::None,
        ExpressionKind::Bool(value) => Value::Bool(*value),
        ExpressionKind::Int(value) => Value::Int(*value),
        ExpressionKind::Float(value) => Value::Float(*value),
        ExpressionKind::String(literal) => Value::String(literal.value.clone()),
    };
    let (payload_range, string_form, string_style) = match &expression.kind {
        ExpressionKind::String(literal) => (
            Some(literal.payload_range.shifted(base_offset)),
            Some(literal.form),
            Some(literal.style),
        ),
        _ => (None, None, None),
    };
    (
        value,
        ValueOrigin::Literal {
            range: expression.range.shifted(base_offset),
            payload_range,
            string_form,
            string_style,
        },
    )
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
                Parameter {
                    name: "body",
                    ty: Type::Content,
                    default: None,
                },
            ],
            trailing_content: Some("body"),
            result: Type::Content,
        };
        let body_range = call.body.as_ref().unwrap().payload_range;
        let bound = bind_arguments(
            &signature,
            &call.arguments,
            Some((Content::new(), body_range)),
            call.name.range,
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
            Some((Content::new(), call.body.as_ref().unwrap().payload_range)),
            call.name.range,
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
