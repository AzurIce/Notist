//! Shared language data. Lexical rules and host path policies live with their owners.
pub mod abi;
mod query;
pub use query::*;

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub source: String,
    pub offset: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub module: String,
    /// Ordered label constraints ending at the target item. Empty means the module itself.
    pub labels: Vec<String>,
}

impl Target {
    pub fn new(module: impl Into<String>, labels: Vec<String>) -> Self {
        Self {
            module: module.into(),
            labels,
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.module)?;
        for label in &self.labels {
            write!(
                f,
                "::{}",
                serde_json::to_string(label).map_err(|_| fmt::Error)?
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Target;

    #[test]
    fn target_display_preserves_quoted_label_segments() {
        let target = Target::new(
            "vault::guide",
            vec!["第一章".into(), "例子::\"quoted\"]]\\\n".into()],
        );
        assert_eq!(
            target.to_string(),
            r#"vault::guide::"第一章"::"例子::\"quoted\"]]\\\n""#
        );
        assert_eq!(
            Target::new("vault::guide", vec![]).to_string(),
            "vault::guide"
        );
    }

    #[test]
    fn target_serialization_uses_label_path() {
        let target = Target::new("vault::guide", vec!["安装".into(), "例子".into()]);
        let json = serde_json::json!({
            "module": "vault::guide",
            "labels": ["安装", "例子"]
        });
        assert_eq!(serde_json::to_value(&target).unwrap(), json);
        assert_eq!(serde_json::from_value::<Target>(json).unwrap(), target);
        assert!(serde_json::from_str::<Target>(r#"{"module":"vault","item":"old"}"#).is_err());
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
