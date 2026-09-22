//! Language values and immutable-by-convention content. Every content node is an Item.
mod diagnostics;
pub mod document;
mod labels;
mod transport;
mod tree;
pub use labels::LabeledItem;
pub use tree::{ItemIndex, ItemNode};

pub use notist_model::{
    CreationOrigin, Diagnostic, DiagnosticCode, Location, OriginKind, SourceSpan, Target, Type,
};
use notist_syntax::{Expr, Param};
use serde_json::{Value as Json, json};
use std::{collections::BTreeMap, rc::Rc};

pub type Env = BTreeMap<String, Value>;
pub type Content = Item;

#[derive(Clone, Debug)]
pub enum Value {
    String(String),
    Int(i64),
    Bool(bool),
    None,
    List(Vec<Value>),
    Dict(Env),
    Content(Content),
    Module(String),
    Target(Target),
    Named(String),
    Closure(Rc<Closure>),
    External(Rc<External>),
}

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
    pub args: Env,
    pub attributes: Env,
    pub location: Location,
    pub origin: Option<CreationOrigin>,
    pub span: Option<SourceSpan>,
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
            Self::Module(_) => Type::Module,
            Self::Target(_) => Type::Target,
            _ => Type::Function,
        }
    }
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Content(c) if c.name == "error")
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
            Self::Module(v) => json!({"module":v}),
            Self::Target(t) => json!({"target":t.module,"labels":t.labels}),
            Self::Named(v) => json!({"function":v}),
            Self::Closure(_) | Self::External(_) => json!({"function":"<function>"}),
        }
    }
    pub fn serializable(&self) -> bool {
        match self {
            Self::Module(_) | Self::Named(_) | Self::Closure(_) | Self::External(_) => false,
            Self::List(v) => v.iter().all(Self::serializable),
            Self::Dict(v) => v.values().all(Self::serializable),
            Self::Content(i) => i
                .args
                .values()
                .chain(i.attributes.values())
                .all(Self::serializable),
            _ => true,
        }
    }
}

impl Item {
    pub fn new(name: impl Into<String>, args: Env, location: Location) -> Self {
        Self {
            name: name.into(),
            args,
            attributes: Env::new(),
            location,
            origin: None,
            span: None,
        }
    }
    pub fn at(mut self, location: &Location) -> Self {
        self.location = location.clone();
        self
    }
    pub fn text(text: impl Into<String>) -> Self {
        Self::new(
            "text",
            Env::from([("text".into(), Value::String(text.into()))]),
            Location {
                source: String::new(),
                offset: 0,
            },
        )
    }
    pub fn seq(children: Vec<Content>) -> Self {
        let location = children
            .first()
            .map(|c| c.location.clone())
            .unwrap_or(Location {
                source: String::new(),
                offset: 0,
            });
        Self::new(
            "seq",
            Env::from([(
                "children".into(),
                Value::List(children.into_iter().map(Value::Content).collect()),
            )]),
            location,
        )
    }
    pub fn link(target: Target, location: Location) -> Self {
        Self::new(
            "link",
            Env::from([("target".into(), Value::Target(target))]),
            location,
        )
    }
    pub fn error(message: impl Into<String>, code: DiagnosticCode, location: Location) -> Self {
        Self::new(
            "error",
            Env::from([
                ("message".into(), Value::String(message.into())),
                ("code".into(), Value::String(code.as_str().into())),
            ]),
            location,
        )
    }
    pub fn string(&self, field: &str) -> Option<&str> {
        match self.args.get(field) {
            Some(Value::String(s)) => Some(s),
            _ => None,
        }
    }
    pub fn children(&self) -> impl Iterator<Item = &Content> {
        let values = match self.args.get("children") {
            Some(Value::List(v)) => v.as_slice(),
            _ => &[],
        };
        values.iter().filter_map(|v| match v {
            Value::Content(c) => Some(c),
            _ => None,
        })
    }
    pub fn apply_attributes(&mut self, attributes: Env) {
        self.attributes.extend(attributes);
    }
    pub fn to_json(&self) -> Json {
        let mut value = json!({"item": self.name, "args": self.args.iter().map(|(k,v)| (k,v.to_json())).collect::<BTreeMap<_,_>>(),
            "attributes": self.attributes.iter().map(|(k,v)| (k,v.to_json())).collect::<BTreeMap<_,_>>(),
            "source": self.location.source, "offset": self.location.offset, "label": self.label().ok().flatten()});
        if let Some(span) = &self.span {
            value["span"] = json!(span);
        }
        value
    }
    pub fn warnings(&self, out: &mut Vec<String>) {
        out.extend(self.diagnostics().iter().map(ToString::to_string));
    }
}
