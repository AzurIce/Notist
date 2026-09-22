//! Declarative document semantics shared by core and external packages.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentMode {
    Flow,
    Inline,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElementModel {
    pub inline: bool,
    /// A true Bool field makes this occurrence block-level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_field: Option<String>,
    #[serde(default)]
    pub slots: BTreeMap<String, ContentMode>,
}
