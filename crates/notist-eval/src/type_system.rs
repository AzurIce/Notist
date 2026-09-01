use std::collections::HashMap;
use std::fmt;

use notist_model::{Node, TextRange};
use notist_syntax::{
    Argument, Expression, ExpressionKind, StringLiteralForm, StringLiteralStyle, UserParameter,
};

pub use notist_model::{DefaultValue, FunctionSignature, Parameter, Type};

use crate::EvalDiagnostic;

#[derive(Clone, Debug)]
pub enum Value {
    Unit,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    /// A call forest: content values are `Node` forests, on both the input
    /// side (always fully reduced) and the output side (may still carry
    /// handler-addressed calls that re-enter the fixpoint).
    Content(Vec<Node>),
    Function(Box<FunctionValue>),
    /// A structured reference target: a module reference plus an optional
    /// module-local selector. Inserting a Target into Markup produces a
    /// `core::reference` element.
    Target(notist_model::Target),
    /// An ordered collection of values; `..` spread splices Arrays.
    Array(Vec<Value>),
    /// An ordered key-to-value mapping: annotation payloads are Dicts.
    /// Equality ignores insertion order (C6).
    Dict(Vec<(DictKey, Value)>),
}

/// A Dict key: identifier keys are sugar for String keys (C5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DictKey {
    Unit,
    Bool(bool),
    Int(i64),
    String(String),
}

impl std::fmt::Display for DictKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unit => formatter.write_str("()"),
            Self::Bool(value) => write!(formatter, "{value}"),
            Self::Int(value) => write!(formatter, "{value}"),
            Self::String(value) => formatter.write_str(value),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Unit, Self::Unit) => true,
            (Self::Bool(left), Self::Bool(right)) => left == right,
            (Self::Int(left), Self::Int(right)) => left == right,
            (Self::Float(left), Self::Float(right)) => left == right,
            (Self::String(left), Self::String(right)) => left == right,
            (Self::Content(left), Self::Content(right)) => left == right,
            (Self::Function(left), Self::Function(right)) => left == right,
            (Self::Target(left), Self::Target(right)) => left == right,
            (Self::Array(left), Self::Array(right)) => left == right,
            (Self::Dict(left), Self::Dict(right)) => {
                left.len() == right.len()
                    && left.iter().all(|(key, value)| {
                        right.iter().any(|(other_key, other_value)| {
                            key == other_key && value == other_value
                        })
                    })
            }
            _ => false,
        }
    }
}

/// A first-class function value (D0002): a closure carrying its callable
/// signature, its implementation, and the environment captured at definition
/// time.
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionValue {
    pub signature: FunctionSignature,
    pub implementation: FunctionImplementation,
    pub captured: HashMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FunctionImplementation {
    /// A registered builtin constructor, dispatched by name.
    Builtin(String),
    /// A user-defined body evaluated in the closure's captured environment.
    User {
        parameters: Vec<UserParameter>,
        result: Type,
        body: Expression,
    },
}

impl Value {
    pub fn ty(&self) -> Type {
        match self {
            Self::Unit => Type::Unit,
            Self::Bool(_) => Type::Bool,
            Self::Int(_) => Type::Int,
            Self::Float(_) => Type::Float,
            Self::String(_) => Type::String,
            Self::Content(_) => Type::Content,
            Self::Function(_) => Type::Function,
            Self::Target(_) => Type::Target,
            Self::Array(_) => Type::Array(None),
            Self::Dict(_) => Type::Dict(None, None),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueOrigin {
    Default,
    Literal {
        range: TextRange,
        payload_range: Option<TextRange>,
        string_form: Option<StringLiteralForm>,
        string_style: Option<StringLiteralStyle>,
    },
    ContentLiteral {
        range: TextRange,
    },
}

#[derive(Clone, Debug, PartialEq)]
struct BoundValue {
    value: Value,
    origin: ValueOrigin,
}

pub(crate) fn default_to_value(default: &DefaultValue) -> Value {
    match default {
        DefaultValue::None => Value::Unit,
        DefaultValue::Bool(value) => Value::Bool(*value),
        DefaultValue::Int(value) => Value::Int(*value),
        DefaultValue::Float(value) => Value::Float(*value),
        DefaultValue::String(value) => Value::String((*value).to_owned()),
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundArguments {
    values: HashMap<String, BoundValue>,
}

impl BoundArguments {
    /// Creates bound arguments directly from evaluated values, used by the
    /// uniform call reduction path.
    pub fn from_values(values: std::collections::HashMap<String, Value>) -> Self {
        Self {
            values: values
                .into_iter()
                .map(|(name, value)| {
                    (
                        name,
                        BoundValue {
                            value,
                            origin: ValueOrigin::Default,
                        },
                    )
                })
                .collect(),
        }
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.values.get(name).map(|bound| &bound.value)
    }

    /// Iterates over bound values in unspecified order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.values
            .iter()
            .map(|(name, bound)| (name.as_str(), &bound.value))
    }

    pub fn origin(&self, name: &str) -> Option<ValueOrigin> {
        self.values.get(name).map(|bound| bound.origin)
    }

    pub fn int(&self, name: &str) -> i64 {
        match self.get(name) {
            Some(Value::Int(value)) => *value,
            _ => unreachable!("signature binding guarantees an integer value"),
        }
    }

    pub fn bool(&self, name: &str) -> bool {
        match self.get(name) {
            Some(Value::Bool(value)) => *value,
            _ => unreachable!("signature binding guarantees a boolean value"),
        }
    }

    pub fn float(&self, name: &str) -> f64 {
        match self.get(name) {
            Some(Value::Float(value)) => *value,
            _ => unreachable!("signature binding guarantees a floating-point value"),
        }
    }

    pub fn optional_string(&self, name: &str) -> Option<&str> {
        match self.get(name) {
            Some(Value::Unit) => None,
            Some(Value::String(value)) => Some(value),
            _ => unreachable!("signature binding guarantees an optional string value"),
        }
    }

    /// Returns an optional integer argument after signature validation.
    pub fn optional_int(&self, name: &str) -> Option<i64> {
        match self.get(name) {
            Some(Value::Unit) => None,
            Some(Value::Int(value)) => Some(*value),
            _ => unreachable!("signature binding guarantees an optional integer value"),
        }
    }

    pub fn string(&self, name: &str) -> &str {
        match self.get(name) {
            Some(Value::String(value)) => value,
            _ => unreachable!("signature binding guarantees a String value"),
        }
    }

    pub fn string_form(&self, name: &str) -> Option<StringLiteralForm> {
        match self.values.get(name).map(|bound| bound.origin) {
            Some(ValueOrigin::Literal {
                string_form: Some(form),
                ..
            }) => Some(form),
            _ => None,
        }
    }

    pub fn take_content(&mut self, name: &str) -> Vec<Node> {
        match self.values.remove(name).map(|bound| bound.value) {
            Some(Value::Content(content)) => content,
            _ => unreachable!("signature binding guarantees a Content value"),
        }
    }

    /// Removes and returns an optional Content argument after signature validation.
    pub fn take_optional_content(&mut self, name: &str) -> Option<Vec<Node>> {
        match self.values.remove(name).map(|bound| bound.value) {
            Some(Value::Unit) => None,
            Some(Value::Content(content)) => Some(content),
            _ => unreachable!("signature binding guarantees an optional Content value"),
        }
    }

    pub(crate) fn into_values(self) -> HashMap<String, Value> {
        self.values
            .into_iter()
            .map(|(name, bound)| (name, bound.value))
            .collect()
    }
}

pub(crate) fn bind_arguments(
    signature: &FunctionSignature,
    arguments: &[Argument],
    trailing_content: Vec<(Vec<Node>, TextRange)>,
    call_name_range: TextRange,
    base_offset: usize,
    mut evaluate: impl FnMut(&Expression) -> Result<(Value, ValueOrigin), Vec<EvalDiagnostic>>,
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
        } else if saw_named && matches!(argument.expression.kind, ExpressionKind::Content(_)) {
            // R05: the trailing Content block is the one positional argument
            // allowed after named arguments; it binds the declared trailing
            // parameter.
            let trailing = signature.trailing_content.as_deref().and_then(|name| {
                signature
                    .parameters
                    .iter()
                    .find(|parameter| parameter.name == name)
            });
            if trailing.is_none() {
                diagnostics.push(EvalDiagnostic {
                    message: "positional arguments cannot follow named arguments".into(),
                    range: argument.range.shifted(base_offset),
                });
            }
            trailing
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

        let Some(parameter) = parameter else { continue };
        if values.contains_key(&parameter.name) {
            diagnostics.push(EvalDiagnostic {
                message: format!("argument `{}` was provided more than once", parameter.name),
                range: argument.range.shifted(base_offset),
            });
            continue;
        }
        let (mut value, origin) = match evaluate(&argument.expression) {
            Ok(evaluated) => evaluated,
            Err(mut errors) => {
                diagnostics.append(&mut errors);
                continue;
            }
        };
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
        if parameter.ty == Type::Float
            && let Value::Int(integer) = value
        {
            value = Value::Float(integer as f64);
        }
        values.insert(parameter.name.clone(), BoundValue { value, origin });
    }

    for (content, range) in trailing_content {
        let Some(parameter_name) = signature.trailing_content.as_deref() else {
            diagnostics.push(EvalDiagnostic {
                message: "function does not accept trailing Content".into(),
                range: range.shifted(base_offset),
            });
            continue;
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
            continue;
        };
        if parameter.ty != Type::Content {
            diagnostics.push(EvalDiagnostic {
                message: format!(
                    "invalid function signature: trailing parameter `{parameter_name}` must have type Content"
                ),
                range: call_name_range.shifted(base_offset),
            });
        } else if values.contains_key(&parameter.name) {
            diagnostics.push(EvalDiagnostic {
                message: format!("argument `{}` was provided more than once", parameter.name),
                range: range.shifted(base_offset),
            });
        } else {
            values.insert(
                parameter.name.clone(),
                BoundValue {
                    value: Value::Content(content),
                    origin: ValueOrigin::ContentLiteral {
                        range: range.shifted(base_offset),
                    },
                },
            );
        }
    }

    for parameter in &signature.parameters {
        if values.contains_key(&parameter.name) {
            continue;
        }
        if let Some(default) = &parameter.default {
            let mut value = default_to_value(default);
            if parameter.ty == Type::Float
                && let Value::Int(integer) = value
            {
                value = Value::Float(integer as f64);
            }
            values.insert(
                parameter.name.clone(),
                BoundValue {
                    value,
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

pub(crate) fn evaluate_literal(
    expression: &Expression,
    base_offset: usize,
) -> Option<(Value, ValueOrigin)> {
    let value = match &expression.kind {
        ExpressionKind::Unit => Value::Unit,
        ExpressionKind::Bool(value) => Value::Bool(*value),
        ExpressionKind::Int(value) => Value::Int(*value),
        ExpressionKind::Float(value) => Value::Float(*value),
        ExpressionKind::String(literal) => Value::String(literal.value.clone()),
        _ => return None,
    };
    let (payload_range, string_form, string_style) = match &expression.kind {
        ExpressionKind::String(literal) => (
            Some(literal.payload_range.shifted(base_offset)),
            Some(literal.form),
            Some(literal.style),
        ),
        _ => (None, None, None),
    };
    Some((
        value,
        ValueOrigin::Literal {
            range: expression.range.shifted(base_offset),
            payload_range,
            string_form,
            string_style,
        },
    ))
}
