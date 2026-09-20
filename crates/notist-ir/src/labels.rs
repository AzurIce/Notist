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
    /// Pre-order Item occurrences; List/Dict/Content fields are traversed, attributes are not.
    pub fn labeled_items(&self) -> Vec<LabeledItem<'_>> {
        fn item<'a>(i: &'a Item, path: &mut Vec<String>, out: &mut Vec<LabeledItem<'a>>) {
            let label = i.label().ok().flatten();
            if let Some(label) = &label {
                path.push(label.clone());
            }
            if label.is_some() {
                out.push(LabeledItem {
                    item: i,
                    path: path.clone(),
                });
            }
            for field in i.args.values() {
                value(field, path, out);
            }
            if label.is_some() {
                path.pop();
            }
        }
        fn value<'a>(v: &'a Value, path: &mut Vec<String>, out: &mut Vec<LabeledItem<'a>>) {
            match v {
                Value::Item(i) => item(i, path, out),
                Value::Content(c) => content(c, path, out),
                Value::List(v) => {
                    for v in v {
                        value(v, path, out);
                    }
                }
                Value::Dict(v) => {
                    for v in v.values() {
                        value(v, path, out);
                    }
                }
                _ => {}
            }
        }
        fn content<'a>(c: &'a Content, path: &mut Vec<String>, out: &mut Vec<LabeledItem<'a>>) {
            match c {
                Content::Item(i) => item(i, path, out),
                Content::Sequence(v) => {
                    for c in v {
                        content(c, path, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        content(self, &mut Vec::new(), &mut out);
        out
    }

    /// Match an ordered subsequence of strict ancestors, ending at the target itself.
    /// Each occurrence appears once even if its ancestors admit several matching subsequences.
    pub fn label_matches(&self, labels: &[String]) -> Vec<LabeledItem<'_>> {
        let Some((last, ancestors)) = labels.split_last() else {
            return Vec::new();
        };
        self.labeled_items()
            .into_iter()
            .filter(|candidate| {
                let (target, path) = candidate.path.split_last().unwrap();
                if target != last {
                    return false;
                }
                let mut next = 0;
                for label in path {
                    if ancestors.get(next) == Some(label) {
                        next += 1;
                    }
                }
                next == ancestors.len()
            })
            .collect()
    }
}
