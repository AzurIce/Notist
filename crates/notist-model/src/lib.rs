//! Shared language data. Lexical rules and host path policies live with their owners.
pub mod abi;

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub source: String,
    pub offset: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub module: String,
    pub item: Option<String>,
}

impl Target {
    pub fn new(module: impl Into<String>, item: Option<String>) -> Self {
        Self {
            module: module.into(),
            item,
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.module)?;
        if let Some(item) = &self.item {
            write!(f, "#{item}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "inner",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Type {
    String,
    Int,
    Bool,
    Content,
    Item,
    Module,
    Target,
    List,
    Dict,
    Function,
    Any,
    None,
    Optional(Box<Type>),
}
