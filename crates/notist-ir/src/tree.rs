//! Occurrences in immutable output. Routes reference values without copying subtrees.
use crate::{Content, Item, Value};

#[derive(Clone, Debug)]
enum Step {
    Sequence(usize),
    ContentItem,
    Field(String),
    List(usize),
    Dict(String),
    ValueContent,
    ValueItem,
}
#[derive(Clone, Debug)]
pub struct ItemNode {
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub label: Option<String>,
    pub subtree_end: usize,
    route: Vec<Step>,
}
#[derive(Clone, Debug, Default)]
pub struct ItemIndex {
    nodes: Vec<ItemNode>,
    roots: Vec<usize>,
}
impl ItemIndex {
    pub fn new(content: &Content) -> Self {
        let mut index = Self::default();
        index.content(content, None, &mut vec![]);
        index
    }
    pub fn nodes(&self) -> &[ItemNode] {
        &self.nodes
    }
    pub fn roots(&self) -> &[usize] {
        &self.roots
    }
    pub fn item<'a>(&self, content: &'a Content, id: usize) -> &'a Item {
        enum Cursor<'a> {
            Content(&'a Content),
            Value(&'a Value),
            Item(&'a Item),
        }
        let mut cursor = Cursor::Content(content);
        for step in &self.nodes[id].route {
            cursor = match (cursor, step) {
                (Cursor::Content(Content::Sequence(v)), Step::Sequence(i)) => {
                    Cursor::Content(&v[*i])
                }
                (Cursor::Content(Content::Item(v)), Step::ContentItem) => Cursor::Item(v),
                (Cursor::Item(v), Step::Field(k)) => Cursor::Value(&v.args[k]),
                (Cursor::Value(Value::List(v)), Step::List(i)) => Cursor::Value(&v[*i]),
                (Cursor::Value(Value::Dict(v)), Step::Dict(k)) => Cursor::Value(&v[k]),
                (Cursor::Value(Value::Content(v)), Step::ValueContent) => Cursor::Content(v),
                (Cursor::Value(Value::Item(v)), Step::ValueItem) => Cursor::Item(v),
                _ => unreachable!("index and immutable output must belong together"),
            };
        }
        match cursor {
            Cursor::Item(item) => item,
            _ => unreachable!(),
        }
    }
    pub fn label_path(&self, id: usize) -> Vec<String> {
        let mut result = vec![];
        let mut next = Some(id);
        while let Some(i) = next {
            if let Some(label) = &self.nodes[i].label {
                result.push(label.clone());
            }
            next = self.nodes[i].parent;
        }
        result.reverse();
        result
    }
    pub fn matches(&self, labels: &[String]) -> Vec<usize> {
        let Some((last, ancestors)) = labels.split_last() else {
            return vec![];
        };
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(id, node)| {
                if node.label.as_ref() != Some(last) {
                    return None;
                }
                let path = self.label_path(id);
                let mut next = 0;
                for label in &path[..path.len() - 1] {
                    if ancestors.get(next) == Some(label) {
                        next += 1;
                    }
                }
                (next == ancestors.len()).then_some(id)
            })
            .collect()
    }
    fn push_item(&mut self, item: &Item, parent: Option<usize>, route: &mut Vec<Step>) {
        let id = self.nodes.len();
        self.nodes.push(ItemNode {
            parent,
            children: vec![],
            label: item.label().ok().flatten(),
            subtree_end: 0,
            route: route.clone(),
        });
        if let Some(parent) = parent {
            self.nodes[parent].children.push(id);
        } else {
            self.roots.push(id);
        }
        for (key, value) in &item.args {
            route.push(Step::Field(key.clone()));
            self.value(value, Some(id), route);
            route.pop();
        }
        self.nodes[id].subtree_end = self.nodes.len();
    }
    fn content(&mut self, c: &Content, parent: Option<usize>, route: &mut Vec<Step>) {
        match c {
            Content::Item(item) => {
                route.push(Step::ContentItem);
                self.push_item(item, parent, route);
                route.pop();
            }
            Content::Sequence(parts) => {
                for (i, c) in parts.iter().enumerate() {
                    route.push(Step::Sequence(i));
                    self.content(c, parent, route);
                    route.pop();
                }
            }
            _ => {}
        }
    }
    fn value(&mut self, v: &Value, parent: Option<usize>, route: &mut Vec<Step>) {
        match v {
            Value::Item(item) => {
                route.push(Step::ValueItem);
                self.push_item(item, parent, route);
                route.pop();
            }
            Value::Content(c) => {
                route.push(Step::ValueContent);
                self.content(c, parent, route);
                route.pop();
            }
            Value::List(v) => {
                for (i, v) in v.iter().enumerate() {
                    route.push(Step::List(i));
                    self.value(v, parent, route);
                    route.pop();
                }
            }
            Value::Dict(v) => {
                for (k, v) in v {
                    route.push(Step::Dict(k.clone()));
                    self.value(v, parent, route);
                    route.pop();
                }
            }
            _ => {}
        }
    }
}
