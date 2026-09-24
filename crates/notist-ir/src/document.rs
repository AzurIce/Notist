//! Shared document formation. Values are cloned before contextual attributes are applied.
use crate::{
    Content, CreationOrigin, Diagnostic, DiagnosticCode, Env, Item, Location, OriginKind, Value,
};
use notist_model::{ContentMode, ElementModel};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct DocumentRules {
    elements: BTreeMap<String, ElementModel>,
}

impl Default for DocumentRules {
    fn default() -> Self {
        let mut rules = Self {
            elements: BTreeMap::new(),
        };
        for name in ["text", "space", "linebreak", "smartquote", "error", "image"] {
            rules
                .define(
                    name.into(),
                    ElementModel {
                        inline: true,
                        block_field: (name == "image").then(|| "block".into()),
                        slots: BTreeMap::new(),
                    },
                )
                .unwrap();
        }
        for name in ["strong", "em", "underline", "strike", "link"] {
            rules
                .define(
                    name.into(),
                    ElementModel {
                        inline: true,
                        block_field: None,
                        slots: BTreeMap::from([("body".into(), ContentMode::Inline)]),
                    },
                )
                .unwrap();
        }
        for name in ["raw", "math"] {
            rules
                .define(
                    name.into(),
                    ElementModel {
                        inline: true,
                        block_field: Some("block".into()),
                        slots: BTreeMap::new(),
                    },
                )
                .unwrap();
        }
        for name in ["paragraph", "heading"] {
            rules
                .define(
                    name.into(),
                    ElementModel {
                        inline: false,
                        block_field: None,
                        slots: BTreeMap::from([("body".into(), ContentMode::Inline)]),
                    },
                )
                .unwrap();
        }
        rules
            .define(
                "section".into(),
                ElementModel {
                    inline: false,
                    block_field: None,
                    slots: BTreeMap::from([
                        ("title".into(), ContentMode::Inline),
                        ("body".into(), ContentMode::Flow),
                    ]),
                },
            )
            .unwrap();
        for name in ["list-item", "term-item"] {
            rules
                .define(
                    name.into(),
                    ElementModel {
                        inline: false,
                        block_field: None,
                        slots: BTreeMap::from([
                            ("body".into(), ContentMode::Flow),
                            ("term".into(), ContentMode::Inline),
                        ]),
                    },
                )
                .unwrap();
        }
        for (name, slots) in [
            (
                "callout",
                vec![("title", ContentMode::Inline), ("body", ContentMode::Flow)],
            ),
            (
                "details",
                vec![
                    ("summary", ContentMode::Inline),
                    ("body", ContentMode::Flow),
                ],
            ),
            (
                "quote",
                vec![
                    ("attribution", ContentMode::Inline),
                    ("body", ContentMode::Flow),
                ],
            ),
            (
                "figure",
                vec![
                    ("caption", ContentMode::Inline),
                    ("body", ContentMode::Flow),
                ],
            ),
            ("table", vec![("body", ContentMode::Flow)]),
            ("table-cell", vec![("body", ContentMode::Flow)]),
        ] {
            rules
                .define(
                    name.into(),
                    ElementModel {
                        inline: false,
                        block_field: None,
                        slots: slots
                            .into_iter()
                            .map(|(field, mode)| (field.into(), mode))
                            .collect(),
                    },
                )
                .unwrap();
        }
        rules
            .define(
                "rule".into(),
                ElementModel {
                    inline: false,
                    block_field: None,
                    slots: BTreeMap::new(),
                },
            )
            .unwrap();
        rules
    }
}

impl DocumentRules {
    /// Repeated identical declarations are harmless; conflicting declarations are errors.
    pub fn define(&mut self, name: String, model: ElementModel) -> Result<(), String> {
        if name.is_empty() || matches!(name.as_str(), "seq" | "annotation" | "parbreak") {
            return Err(format!("reserved or empty element name `{name}`"));
        }
        if let Some(previous) = self.elements.get(&name) {
            return if previous == &model {
                Ok(())
            } else {
                Err(format!("conflicting content model for `{name}`"))
            };
        }
        self.elements.insert(name, model);
        Ok(())
    }
    pub fn model(&self, name: &str) -> Option<&ElementModel> {
        self.elements.get(name)
    }
    pub fn form(&self, raw: &Content) -> FormedDocument {
        let mut former = Former {
            rules: self,
            diagnostics: vec![],
            remaining: 100_000,
        };
        let content = former.node(raw, ContentMode::Flow, 0);
        FormedDocument {
            content,
            diagnostics: former.diagnostics,
        }
    }
    fn inline(&self, item: &Item, depth: usize) -> bool {
        if depth >= 128 {
            return false;
        }
        if item.name == "seq" {
            return item
                .children()
                .all(|c| c.name == "annotation" || self.inline(c, depth + 1));
        }
        self.model(&item.name).is_some_and(|m| {
            m.inline
                && !m
                    .block_field
                    .as_ref()
                    .is_some_and(|field| matches!(item.args.get(field), Some(Value::Bool(true))))
        })
    }
}

#[derive(Clone, Debug)]
pub struct FormedDocument {
    pub content: Content,
    pub diagnostics: Vec<Diagnostic>,
}

struct Former<'a> {
    rules: &'a DocumentRules,
    diagnostics: Vec<Diagnostic>,
    remaining: usize,
}

impl Former<'_> {
    fn failure(&mut self, message: impl Into<String>, location: &Location) -> Content {
        let message = message.into();
        self.diagnostics.push(Diagnostic::error(
            DiagnosticCode::ContentConstraint,
            &message,
            Some(location.clone()),
        ));
        Content::error(message, DiagnosticCode::ContentConstraint, location.clone())
    }
    fn reject(&mut self, raw: &Content, message: impl Into<String>) -> Content {
        let mut error = self.failure(message, &raw.location);
        error.span = raw.span.clone();
        error.attributes = raw.attributes.clone();
        self.diagnostics.last_mut().unwrap().span = raw.span.clone();
        error
    }
    fn node(&mut self, raw: &Content, mode: ContentMode, depth: usize) -> Content {
        if depth >= 128 || self.remaining == 0 {
            return self.failure("document formation limit exceeded", &raw.location);
        }
        self.remaining -= 1;
        if raw.name == "seq" {
            return self.sequence(raw, mode, depth + 1);
        }
        let mut item = raw.clone();
        let slots = self
            .rules
            .model(&item.name)
            .map(|m| m.slots.clone())
            .unwrap_or_default();
        for (field, mode) in slots {
            if let Some(value) = item.args.get_mut(&field) {
                match value {
                    Value::Content(c) => {
                        // A content slot accepts any root Item, not just a literal seq.
                        *c = if c.name == "seq" {
                            self.node(c, mode, depth + 1)
                        } else {
                            self.sequence(
                                &Content::seq(vec![c.clone()]).at(&c.location),
                                mode,
                                depth + 1,
                            )
                        };
                    }
                    _ => {
                        *value = Value::Content(self.failure(
                            format!("{}.{} requires Content", item.name, field),
                            &item.location,
                        ));
                    }
                }
            }
        }
        if item.name == "table" {
            if let Some(Value::Content(body)) = item.args.get_mut("body") {
                if let Some(Value::List(children)) = body.args.get_mut("children") {
                    children.retain(|child| !matches!(child, Value::Content(item) if item.name == "space" && item.attributes.is_empty()));
                }
            }
            if let Err(message) = validate_table(&item) {
                return self.reject(&item, message);
            }
        }
        item
    }
    fn sequence(&mut self, raw: &Content, mode: ContentMode, depth: usize) -> Content {
        let mut output = vec![];
        let mut inline = vec![];
        let mut attributes = Env::new();
        let mut annotation_location: Option<Location> = None;
        let children = match raw.args.get("children") {
            Some(Value::List(children)) => children.clone(),
            _ => vec![Value::Content(self.failure(
                "seq.children requires a List of Content",
                &raw.location,
            ))],
        };
        for value in children {
            let child = match value {
                Value::Content(c) => c,
                _ => self.failure("seq.children requires Content values", &raw.location),
            };
            if child.name == "annotation" {
                if mode == ContentMode::Flow {
                    self.flush(
                        &mut inline,
                        &mut output,
                        &mut attributes,
                        &mut annotation_location,
                    );
                }
                if !child.attributes.is_empty() {
                    output.push(self.failure(
                        "attributes on a consumed annotation have no content target",
                        &child.location,
                    ));
                }
                match child.args.get("attributes") {
                    Some(Value::Dict(values)) => {
                        attributes.extend(values.clone());
                        annotation_location = Some(child.location.clone());
                    }
                    _ => output
                        .push(self.failure("annotation.attributes requires Dict", &child.location)),
                }
                continue;
            }
            if child.name == "parbreak" {
                if mode == ContentMode::Inline {
                    output.push(self.reject(&child, "parbreak may not occur in inline content"));
                } else {
                    self.flush(
                        &mut inline,
                        &mut output,
                        &mut attributes,
                        &mut annotation_location,
                    );
                }
                if !child.attributes.is_empty() {
                    output.push(self.failure(
                        "attributes on a consumed parbreak have no content target",
                        &child.location,
                    ));
                }
                continue;
            }
            let is_inline = self.rules.inline(&child, depth);
            if mode == ContentMode::Inline {
                let mut formed = if is_inline {
                    self.node(&child, ContentMode::Inline, depth)
                } else {
                    self.reject(
                        &child,
                        format!("{} may not occur in inline content", child.name),
                    )
                };
                if child.name != "space" {
                    attach(&mut formed, &mut attributes, &mut annotation_location);
                }
                output.push(formed);
            } else if is_inline {
                let mut formed = self.node(&child, ContentMode::Inline, depth);
                if inline.is_empty() && formed.name == "space" && formed.attributes.is_empty() {
                    continue;
                }
                // An explicit sequence is a target in its own right, including an empty one.
                if inline.is_empty() && formed.name == "seq" {
                    attach(&mut formed, &mut attributes, &mut annotation_location);
                }
                inline.push(formed);
            } else {
                self.flush(
                    &mut inline,
                    &mut output,
                    &mut attributes,
                    &mut annotation_location,
                );
                let mut formed = self.node(&child, ContentMode::Flow, depth);
                attach(&mut formed, &mut attributes, &mut annotation_location);
                output.push(formed);
            }
        }
        if mode == ContentMode::Flow {
            self.flush(
                &mut inline,
                &mut output,
                &mut attributes,
                &mut annotation_location,
            );
        }
        if let Some(location) = annotation_location {
            output.push(self.failure("Item annotation has no following Item", &location));
        }
        let mut result = raw.clone();
        result.args.insert(
            "children".into(),
            Value::List(output.into_iter().map(Value::Content).collect()),
        );
        result
    }
    fn flush(
        &mut self,
        inline: &mut Vec<Content>,
        output: &mut Vec<Content>,
        attributes: &mut Env,
        annotation_location: &mut Option<Location>,
    ) {
        while inline
            .last()
            .is_some_and(|c| c.name == "space" && c.attributes.is_empty())
        {
            inline.pop();
        }
        if inline.is_empty() {
            return;
        }
        if !inline.iter().any(substantive) {
            output.append(inline);
            return;
        }
        let location = inline[0].location.clone();
        let span = inline
            .iter()
            .filter_map(|c| c.span.as_ref())
            .filter(|s| s.source == location.source)
            .fold(None::<notist_model::SourceSpan>, |span, next| {
                Some(match span {
                    Some(mut span) => {
                        span.start = span.start.min(next.start);
                        span.end = span.end.max(next.end);
                        span
                    }
                    None => next.clone(),
                })
            });
        let body = Content::seq(std::mem::take(inline));
        let mut paragraph = Item::new(
            "paragraph",
            Env::from([("body".into(), Value::Content(body))]),
            location,
        );
        paragraph.origin = Some(CreationOrigin {
            node_id: None,
            kind: OriginKind::Formation,
        });
        paragraph.span = span;
        attach(&mut paragraph, attributes, annotation_location);
        output.push(paragraph);
    }
}

fn validate_table(table: &Item) -> Result<(), String> {
    let Some(Value::Int(columns)) = table.args.get("columns") else {
        return Err("table.columns requires a positive Int".into());
    };
    let Ok(columns) = usize::try_from(*columns) else {
        return Err("table.columns requires a positive Int".into());
    };
    if columns == 0 || columns > 256 {
        return Err("table.columns must be between 1 and 256".into());
    }
    if !matches!(table.args.get("header"), None | Some(Value::Bool(_))) {
        return Err("table.header requires Bool".into());
    }
    match table.args.get("align") {
        None | Some(Value::Unit) => {}
        Some(Value::String(align)) => {
            let values = align.split(',').map(str::trim).collect::<Vec<_>>();
            if values.len() != columns
                || !values
                    .iter()
                    .all(|value| matches!(*value, "" | "default" | "left" | "center" | "right"))
            {
                return Err("table.align requires one alignment per column".into());
            }
        }
        _ => return Err("table.align requires String or Unit".into()),
    }
    let Some(Value::Content(body)) = table.args.get("body") else {
        return Err("table.body requires Content".into());
    };
    let Some(Value::List(children)) = body.args.get("children") else {
        return Err("table.body requires a sequence of table cells".into());
    };
    if children.is_empty() {
        return Err("table requires at least one table-cell".into());
    }
    let mut occupied = vec![0usize; columns];
    let mut cursor = 0;
    for child in children {
        let Value::Content(cell) = child else {
            return Err("table.body requires table-cell Items".into());
        };
        if cell.name != "table-cell" {
            return Err("table.body requires table-cell Items".into());
        }
        let span = |field: &str| match cell.args.get(field) {
            None => Ok(1),
            Some(Value::Int(n)) if (1..=256).contains(n) => Ok(*n as usize),
            _ => Err(format!("table-cell.{field} must be between 1 and 256")),
        };
        let colspan = span("colspan")?;
        let rowspan = span("rowspan")?;
        while occupied.iter().all(|n| *n > 0) {
            occupied.iter_mut().for_each(|n| *n -= 1);
            cursor = 0;
        }
        let Some(start) = (cursor..columns).find(|i| occupied[*i] == 0) else {
            return Err("table cells exceed the declared columns".into());
        };
        if start + colspan > columns || occupied[start..start + colspan].iter().any(|n| *n > 0) {
            return Err("table-cell overlaps another cell or exceeds the declared columns".into());
        }
        occupied[start..start + colspan].fill(rowspan);
        cursor = start + colspan;
    }
    while occupied.iter().all(|n| *n > 0) {
        occupied.iter_mut().for_each(|n| *n -= 1);
    }
    if occupied.iter().any(|n| *n > 0) {
        return Err("table ends with an incomplete row".into());
    }
    Ok(())
}

fn attach(item: &mut Item, attributes: &mut Env, location: &mut Option<Location>) {
    item.apply_attributes(std::mem::take(attributes));
    *location = None;
}
fn substantive(item: &Item) -> bool {
    match item.name.as_str() {
        "seq" => item.children().any(substantive),
        "text" => !item.string("text").unwrap_or_default().is_empty(),
        "space" => !item.attributes.is_empty(),
        _ => true,
    }
}

impl Item {
    /// Ordered document blocks, looking through grouping sequences without copying them.
    pub fn flow_blocks(&self) -> Vec<&Content> {
        if self.name == "seq" {
            self.children().flat_map(Item::flow_blocks).collect()
        } else {
            vec![self]
        }
    }
    pub fn main_content(&self) -> Option<&Content> {
        match self.args.get("body") {
            Some(Value::Content(body)) => body.flow_blocks().first().copied(),
            _ => None,
        }
    }
    pub fn detail_content(&self) -> Vec<&Content> {
        match self.args.get("body") {
            Some(Value::Content(body)) => body.flow_blocks().into_iter().skip(1).collect(),
            _ => vec![],
        }
    }
}
