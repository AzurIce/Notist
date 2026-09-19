//! HTML consumes the content model; component validation belongs to the renderer.
use crate::{
    content::{Content, Item, escape},
    runtime::Value,
};

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn error(message: &str) -> String {
    format!(
        "<notist-error role=\"note\">{}</notist-error>",
        escape(message)
    )
}

pub fn render(content: &Content) -> String {
    match content {
        Content::Text(text) => escape(text),
        Content::Sequence(children) => children.iter().map(render).collect(),
        Content::Error { message, .. } => error(message),
        Content::Item(item) => render_item(item),
        Content::Link { target } => {
            format!("<a href=\"{}\">{}</a>", escape(target), escape(target))
        }
    }
}

fn render_value(value: &Value) -> String {
    match value {
        Value::Content(c) => render(c),
        Value::Item(i) => render_item(i),
        Value::List(v) => v.iter().map(render_value).collect(),
        Value::None => String::new(),
        _ => error("expected child Content"),
    }
}

fn render_item(item: &Item) -> String {
    let native = match item.name.as_str() {
        "paragraph" => Some("p"),
        "section" => Some("section"),
        "strong" => Some("strong"),
        "em" => Some("em"),
        "list" => Some(
            if matches!(item.args.get("ordered"), Some(Value::Bool(true))) {
                "ol"
            } else {
                "ul"
            },
        ),
        "list-item" => Some("li"),
        "heading" => Some(match item.args.get("level") {
            Some(Value::Int(1)) => "h1",
            Some(Value::Int(2)) | None => "h2",
            Some(Value::Int(3)) => "h3",
            Some(Value::Int(4)) => "h4",
            Some(Value::Int(5)) => "h5",
            Some(Value::Int(6)) => "h6",
            _ => return error("heading level must be an integer from 1 to 6"),
        }),
        _ => None,
    };
    if let Some(tag) = native {
        let Some(body) = item.args.get("body") else {
            return error("missing body Content");
        };
        let title = if item.name == "section" {
            item.args
                .get("title")
                .map(|title| {
                    let level = match item.args.get("level") {
                        Some(Value::Int(n)) => (*n).clamp(1, 6),
                        _ => 1,
                    };
                    format!("<h{level}>{}</h{level}>", render_value(title))
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
        return format!("<{tag}>{title}{}</{tag}>", render_value(body));
    }
    if !valid_name(&item.name) {
        return error("Item name cannot be represented as an HTML component tag");
    }
    let mut attrs = String::new();
    for (key, value) in &item.args {
        if !valid_name(key) {
            return error("Item field cannot be represented as a data attribute");
        }
        if !value.serializable() {
            return error("Item field is not serializable");
        }
        attrs.push_str(&format!(
            " data-{key}=\"{}\"",
            escape(&value.to_json().to_string())
        ));
    }
    format!("<notist-{}{attrs}></notist-{}>", item.name, item.name)
}
