//! HTML consumes the content model; component validation belongs to the renderer.
use notist_ir::{Content, Item, Value};

/// Browser renderer for serialized Content and package component resources.
pub const RENDERER_JS: &str = include_str!("../renderer.js");

/// Entry page for a bundle containing Content, attributes and component resources.
pub const BUNDLE_HTML: &str = include_str!("../bundle.html");

pub trait RenderHtml {
    fn html(&self) -> String;
}

impl RenderHtml for Content {
    fn html(&self) -> String {
        render(self)
    }
}

pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

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
            let target = target.to_string();
            format!("<a href=\"{}\">{}</a>", escape(&target), escape(&target))
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
    let attributes = if item.attributes.is_empty() {
        String::new()
    } else {
        let json = item
            .attributes
            .iter()
            .map(|(k, v)| (k, v.to_json()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let id = match item.attributes.get("id") {
            Some(Value::String(id)) => format!(" id=\"{}\"", escape(id)),
            _ => String::new(),
        };
        format!(
            "{id} data-notist-attributes=\"{}\"",
            escape(&serde_json::to_string(&json).unwrap())
        )
    };
    if item.name == "linebreak" {
        return format!("<br{attributes}>");
    }
    if item.name == "smartquote" {
        let double = matches!(item.args.get("double"), Some(Value::Bool(true)));
        let open = matches!(item.args.get("open"), Some(Value::Bool(true)));
        return match (double, open) {
            (true, true) => "\u{201c}",
            (true, false) => "\u{201d}",
            (false, true) => "\u{2018}",
            (false, false) => "\u{2019}",
        }
        .into();
    }
    if matches!(item.name.as_str(), "raw" | "math") {
        let Some(Value::String(content)) = item.args.get("content") else {
            return error("expected String content");
        };
        let block = matches!(item.args.get("block"), Some(Value::Bool(true)));
        let class = if item.name == "math" {
            "notist-math"
        } else {
            "notist-raw"
        };
        let lang = match item.args.get("lang") {
            Some(Value::String(lang)) => format!(" data-language=\"{}\"", escape(lang)),
            _ => String::new(),
        };
        return if block {
            format!(
                "<pre{attributes} class=\"{class}\"{lang}><code>{}</code></pre>",
                escape(content)
            )
        } else {
            format!(
                "<code{attributes} class=\"{class}\"{lang}>{}</code>",
                escape(content)
            )
        };
    }
    if item.name == "link" {
        let Some(Value::String(dest)) = item.args.get("dest") else {
            return error("expected String link destination");
        };
        if !safe_link(dest) {
            return error("unsupported link scheme");
        }
        let Some(body) = item.args.get("body") else {
            return error("missing link body");
        };
        return format!(
            "<a{attributes} href=\"{}\">{}</a>",
            escape(dest),
            render_value(body)
        );
    }
    if item.name == "term-item" {
        let (Some(term), Some(body)) = (item.args.get("term"), item.args.get("body")) else {
            return error("missing term or body");
        };
        return format!(
            "<div{attributes}><dt>{}</dt><dd>{}</dd></div>",
            render_value(term),
            render_value(body)
        );
    }
    let native = match item.name.as_str() {
        "terms" => Some("dl"),
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
        let number = if item.name == "list-item" {
            match item.args.get("number") {
                Some(Value::Int(n)) => format!(" value=\"{n}\""),
                _ => String::new(),
            }
        } else {
            String::new()
        };
        let tight = match item.args.get("tight") {
            Some(Value::Bool(tight)) => format!(" data-tight=\"{tight}\""),
            _ => String::new(),
        };
        return format!(
            "<{tag}{attributes}{number}{tight}>{title}{}</{tag}>",
            render_value(body)
        );
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
    format!(
        "<notist-{}{attributes}{attrs}></notist-{}>",
        item.name, item.name
    )
}

fn safe_link(dest: &str) -> bool {
    !dest.chars().any(char::is_control)
        && dest.split_once(':').is_none_or(|(scheme, _)| {
            matches!(
                scheme.to_ascii_lowercase().as_str(),
                "http" | "https" | "mailto"
            ) || scheme.contains('/')
                || scheme.starts_with('#')
        })
}
