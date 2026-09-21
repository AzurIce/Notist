//! Labels address Item occurrences in evaluated Content, independently of source locations.
use crate::{Content, Item, Value};

/// One occurrence in the output tree. `path` contains its labeled ancestors and itself.
#[derive(Clone, Debug)]
pub struct LabeledItem<'a> {
    pub item: &'a Item,
    pub path: Vec<String>,
}

impl Item {
    /// Explicit labels override the section title. Unlabeled Items remain structural ancestors.
    pub fn label(&self) -> Result<Option<String>, String> {
        if let Some(value) = self.attributes.get("label") {
            return match value {
                Value::String(label) if !label.is_empty() => Ok(Some(label.clone())),
                _ => Err("Item label must be a non-empty String".into()),
            };
        }
        if self.name == "section" {
            let title = self.args.get("title").map(text).unwrap_or_default();
            return Ok((!title.is_empty()).then_some(title));
        }
        Ok(None)
    }
}

fn text(value: &Value) -> String {
    fn content(value: &Content) -> String {
        match value {
            Content::Text(s) => s.clone(),
            Content::Sequence(parts) => parts.iter().map(content).collect(),
            Content::Item(item) => item_text(item),
            Content::Link { target, .. } => target.to_string(),
            Content::Error { .. } => String::new(),
        }
    }
    fn item_text(item: &Item) -> String {
        if item.name == "linebreak" {
            return "\n".into();
        }
        if item.name == "smartquote" {
            let double = matches!(item.args.get("double"), Some(Value::Bool(true)));
            let open = matches!(item.args.get("open"), Some(Value::Bool(true)));
            return match (double, open) {
                (true, true) => "“",
                (true, false) => "”",
                (false, true) => "‘",
                (false, false) => "’",
            }
            .into();
        }
        item.args
            .get("body")
            .or_else(|| item.args.get("content"))
            .or_else(|| item.args.get("title"))
            .map(text)
            .unwrap_or_default()
    }
    match value {
        Value::String(s) => s.clone(),
        Value::Content(c) => content(c),
        Value::Item(i) => item_text(i),
        Value::List(values) => values.iter().map(text).collect(),
        _ => String::new(),
    }
}

impl Content {
    pub fn labeled_items(&self) -> Vec<LabeledItem<'_>> {
        let index = crate::ItemIndex::new(self);
        index
            .nodes()
            .iter()
            .enumerate()
            .filter(|(_, n)| n.label.is_some())
            .map(|(id, _)| LabeledItem {
                item: index.item(self, id),
                path: index.label_path(id),
            })
            .collect()
    }

    pub fn label_matches(&self, labels: &[String]) -> Vec<LabeledItem<'_>> {
        let index = crate::ItemIndex::new(self);
        index
            .matches(labels)
            .into_iter()
            .map(|id| LabeledItem {
                item: index.item(self, id),
                path: index.label_path(id),
            })
            .collect()
    }
}
