use crate::runtime::Value;
use serde_json::{Value as Json, json};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Location {
    pub source: String,
    pub offset: usize,
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
    Link { target: String },
    Error { message: String, location: Location },
}

impl Item {
    pub fn to_json(&self) -> Json {
        json!({"item": self.name, "args": self.args.iter().map(|(k,v)| (k, v.to_json())).collect::<BTreeMap<_,_>>(),
            "attributes": self.attributes.iter().map(|(k,v)| (k, v.to_json())).collect::<BTreeMap<_,_>>(),
            "source": self.location.source, "offset": self.location.offset})
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
            Self::Link { target } => json!({"link": target}),
            Self::Error { message, location } => {
                json!({"error": message, "source": location.source, "offset": location.offset})
            }
        }
    }
    pub fn html(&self) -> String {
        crate::html::render(self)
    }
    pub fn warnings(&self, out: &mut Vec<String>) {
        fn visit(value: &Value, out: &mut Vec<String>) {
            match value {
                Value::Content(c) => c.warnings(out),
                Value::Item(i) => {
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
                for v in item.args.values().chain(item.attributes.values()) {
                    visit(v, out);
                }
            }
            _ => {}
        }
    }
}

pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
