//! Data exchanged with plugins, independent of syntax trees and runtime state.
use crate::{ElementModel, Target, Type};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    pub functions: BTreeMap<String, Function>,
    #[serde(default)]
    pub elements: BTreeMap<String, ElementModel>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Function {
    pub export: String,
    pub params: Vec<Parameter>,
    pub result: Type,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Parameter {
    pub name: String,
    pub ty: Type,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

/// No closures, module handles or source locations cross the plugin boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Value {
    String(String),
    Int(i64),
    Bool(bool),
    None,
    List(Vec<Value>),
    Dict(BTreeMap<String, Value>),
    Content(Content),
    Target(Target),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub name: String,
    pub args: BTreeMap<String, Value>,
    pub attributes: BTreeMap<String, Value>,
}

/// Content always consists of one Item, including text, sequences and errors.
pub type Content = Item;

impl Item {
    pub fn new(name: impl Into<String>, args: BTreeMap<String, Value>) -> Self {
        Self {
            name: name.into(),
            args,
            attributes: BTreeMap::new(),
        }
    }
    pub fn text(text: impl Into<String>) -> Self {
        Self::new(
            "text",
            BTreeMap::from([("text".into(), Value::String(text.into()))]),
        )
    }
    pub fn seq(children: Vec<Content>) -> Self {
        Self::new(
            "seq",
            BTreeMap::from([(
                "children".into(),
                Value::List(children.into_iter().map(Value::Content).collect()),
            )]),
        )
    }
    pub fn error(message: impl Into<String>) -> Self {
        Self::new(
            "error",
            BTreeMap::from([("message".into(), Value::String(message.into()))]),
        )
    }
}
