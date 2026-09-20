//! Language values and content. Function values retain parsed bodies, not execution logic.
mod labels;
mod transport;
pub use labels::LabeledItem;

pub use notist_model::{Location, Target, Type};
use notist_syntax::{Expr, Param};
use serde_json::{Value as Json, json};
use std::{collections::BTreeMap, rc::Rc};

pub type Env = BTreeMap<String, Value>;

#[derive(Clone, Debug)]
pub enum Value {
    String(String),
    Int(i64),
    Bool(bool),
    None,
    List(Vec<Value>),
    Dict(Env),
    Content(Content),
    Item(Item),
    Module(String),
    Target(Target),
    Named(String),
    Closure(Rc<Closure>),
    External(Rc<External>),
}

/// A function value retains its lexical environment and parsed body.
#[derive(Clone, Debug)]
pub struct Closure {
    pub params: Vec<Param>,
    pub body: Expr,
    pub env: Env,
    pub source: String,
    pub name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct External {
    pub params: Vec<Param>,
    pub defaults: Env,
    pub path: String,
    pub export: String,
    pub result: Type,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub name: String,
    pub args: BTreeMap<String, Value>,
    pub attributes: BTreeMap<String, Value>,
    pub location: Location,
}

#[derive(Clone, Debug)]
pub enum Content {
    Text(String),
    Sequence(Vec<Content>),
    Item(Item),
    Link { target: Target, location: Location },
    Error { message: String, location: Location },
}

impl Value {
    pub fn ty(&self) -> Type {
        match self {
            Self::String(_) => Type::String,
            Self::Int(_) => Type::Int,
            Self::Bool(_) => Type::Bool,
            Self::None => Type::None,
            Self::List(_) => Type::List,
            Self::Dict(_) => Type::Dict,
            Self::Content(_) => Type::Content,
            Self::Item(_) => Type::Item,
            Self::Module(_) => Type::Module,
            Self::Target(_) => Type::Target,
            _ => Type::Function,
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Self::Content(Content::Error { .. }))
    }

    pub fn to_json(&self) -> Json {
        match self {
            Self::String(v) => json!(v),
            Self::Int(v) => json!(v),
            Self::Bool(v) => json!(v),
            Self::None => Json::Null,
            Self::List(v) => json!(v.iter().map(Self::to_json).collect::<Vec<_>>()),
            Self::Dict(v) => {
                json!({"dict": v.iter().map(|(k,v)| (k,v.to_json())).collect::<BTreeMap<_,_>>()})
            }
            Self::Content(v) => v.to_json(),
            Self::Item(v) => v.to_json(),
            Self::Module(v) => json!({"module":v}),
            Self::Target(target) => json!({"target": target.module, "labels": target.labels}),
            Self::Named(v) => json!({"function":v}),
            Self::Closure(_) | Self::External(_) => json!({"function":"<function>"}),
        }
    }

    pub fn serializable(&self) -> bool {
        match self {
            Self::Module(_)
            | Self::Target(_)
            | Self::Named(_)
            | Self::Closure(_)
            | Self::External(_) => false,
            Self::List(v) => v.iter().all(Self::serializable),
            Self::Dict(v) => v.values().all(Self::serializable),
            Self::Item(i) | Self::Content(Content::Item(i)) => i
                .args
                .values()
                .chain(i.attributes.values())
                .all(Self::serializable),
            Self::Content(Content::Sequence(v)) => {
                v.iter().all(|c| Self::Content(c.clone()).serializable())
            }
            _ => true,
        }
    }
}

impl Item {
    pub fn to_json(&self) -> Json {
        json!({"item": self.name, "args": self.args.iter().map(|(k,v)| (k, v.to_json())).collect::<BTreeMap<_,_>>(),
            "attributes": self.attributes.iter().map(|(k,v)| (k, v.to_json())).collect::<BTreeMap<_,_>>(),
            "source": self.location.source, "offset": self.location.offset, "label": self.label().ok().flatten()})
    }
}

impl Content {
    pub fn to_json(&self) -> Json {
        match self {
            Self::Text(text) => json!({"text": text}),
            Self::Sequence(children) => {
                json!({"sequence": children.iter().map(Self::to_json).collect::<Vec<_>>()})
            }
            Self::Item(item) => item.to_json(),
            Self::Link { target, location } => json!({"link": target.to_string(), "target": target,
                "source": location.source, "offset": location.offset}),
            Self::Error { message, location } => {
                json!({"error": message, "source": location.source, "offset": location.offset})
            }
        }
    }
    pub fn warnings(&self, out: &mut Vec<String>) {
        fn visit(value: &Value, out: &mut Vec<String>) {
            match value {
                Value::Content(c) => c.warnings(out),
                Value::Item(i) => {
                    if let Err(message) = i.label() {
                        out.push(format!(
                            "{}:{}: {message}",
                            i.location.source, i.location.offset
                        ));
                    }
                    for v in i.args.values().chain(i.attributes.values()) {
                        visit(v, out);
                    }
                }
                Value::List(values) => {
                    for v in values {
                        visit(v, out);
                    }
                }
                Value::Dict(fields) => {
                    for v in fields.values() {
                        visit(v, out);
                    }
                }
                _ => {}
            }
        }
        match self {
            Self::Error { message, location } => out.push(format!(
                "{}:{}: {message}",
                location.source, location.offset
            )),
            Self::Sequence(children) => {
                for c in children {
                    c.warnings(out);
                }
            }
            Self::Item(item) => {
                if let Err(message) = item.label() {
                    out.push(format!(
                        "{}:{}: {message}",
                        item.location.source, item.location.offset
                    ));
                }
                for v in item.args.values().chain(item.attributes.values()) {
                    visit(v, out);
                }
            }
            _ => {}
        }
    }
}
