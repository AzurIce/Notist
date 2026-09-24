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
    render_item(content)
}

fn render_sequence(children: &[&Content]) -> String {
    let mut output = String::new();
    let mut index = 0;
    while index < children.len() {
        let child = children[index];
        let tag = match child.name.as_str() {
            "list-item" => Some(
                if matches!(child.args.get("ordered"), Some(Value::Bool(true))) {
                    "ol"
                } else {
                    "ul"
                },
            ),
            "term-item" => Some("dl"),
            _ => None,
        };
        if let Some(tag) = tag {
            let start = if tag == "ol" {
                match child.args.get("number") {
                    Some(Value::Int(n)) => format!(" start=\"{n}\""),
                    _ => String::new(),
                }
            } else {
                String::new()
            };
            output.push_str(&format!("<{tag}{start}>"));
            while index < children.len() {
                let next = children[index];
                let same = if tag == "dl" {
                    next.name == "term-item"
                } else {
                    next.name == "list-item"
                        && matches!(next.args.get("ordered"), Some(Value::Bool(true)))
                            == (tag == "ol")
                };
                if !same {
                    break;
                }
                output.push_str(&render_item(next));
                index += 1;
            }
            output.push_str(&format!("</{tag}>"));
        } else {
            output.push_str(&render_item(child));
            index += 1;
        }
    }
    output
}

fn render_value(value: &Value) -> String {
    match value {
        Value::Content(c) => render(c),
        Value::List(v) => v.iter().map(render_value).collect(),
        Value::Unit => String::new(),
        _ => error("expected child Content"),
    }
}

fn render_item(item: &Item) -> String {
    let attributes = item_attributes(item);
    if item.name == "seq" {
        let body = render_sequence(&item.children().collect::<Vec<_>>());
        return if attributes.is_empty() {
            body
        } else {
            format!("<notist-seq style=\"display:contents\"{attributes}>{body}</notist-seq>")
        };
    }
    if matches!(item.name.as_str(), "text" | "space") {
        let body = if item.name == "space" {
            " ".into()
        } else {
            escape(item.string("text").unwrap_or_default())
        };
        return if attributes.is_empty() {
            body
        } else {
            format!("<span{attributes}>{body}</span>")
        };
    }
    if item.name == "error" {
        return format!(
            "<notist-error role=\"note\"{attributes}>{}</notist-error>",
            escape(item.string("message").unwrap_or("invalid error Item"))
        );
    }
    if item.name == "linebreak" {
        return format!("<br{attributes}>");
    }
    if item.name == "smartquote" {
        let double = matches!(item.args.get("double"), Some(Value::Bool(true)));
        let open = matches!(item.args.get("open"), Some(Value::Bool(true)));
        let quote = match (double, open) {
            (true, true) => "\u{201c}",
            (true, false) => "\u{201d}",
            (false, true) => "\u{2018}",
            (false, false) => "\u{2019}",
        };
        return if attributes.is_empty() {
            quote.into()
        } else {
            format!("<span{attributes}>{quote}</span>")
        };
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
    render_structural_item(item, &attributes)
}

fn item_attributes(item: &Item) -> String {
    let label = item
        .label()
        .ok()
        .flatten()
        .map(|label| format!(" data-notist-label=\"{}\"", escape(&label)))
        .unwrap_or_default();
    if item.attributes.is_empty() {
        label.clone()
    } else {
        let json = item
            .attributes
            .iter()
            .map(|(k, v)| (k, v.to_json()))
            .collect::<std::collections::BTreeMap<_, _>>();
        format!(
            "{label} data-notist-attributes=\"{}\"",
            escape(&serde_json::to_string(&json).unwrap())
        )
    }
}

fn render_structural_item(item: &Item, attributes: &str) -> String {
    if item.name == "link" {
        if let Some(Value::Target(target)) = item.args.get("target") {
            let text = escape(&target.to_string());
            return format!("<a{attributes} href=\"{text}\">{text}</a>");
        }

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
    if item.name == "rule" {
        return format!("<hr{attributes}>");
    }
    if item.name == "image" {
        let (Some(Value::String(source)), Some(Value::String(alt))) =
            (item.args.get("source"), item.args.get("alt"))
        else {
            return error("image requires String source and alt");
        };
        if !safe_image(source) {
            return error("unsupported image source");
        }
        let block = if matches!(item.args.get("block"), Some(Value::Bool(true))) {
            " style=\"display:block\""
        } else {
            ""
        };
        return format!(
            "<img{attributes} src=\"{}\" alt=\"{}\"{block}>",
            escape(source),
            escape(alt)
        );
    }
    if matches!(
        item.name.as_str(),
        "callout" | "details" | "quote" | "figure"
    ) {
        let Some(Value::Content(body)) = item.args.get("body") else {
            return error("missing body Content");
        };
        let body = render(body);
        return match item.name.as_str() {
            "callout" => {
                let kind = item.string("kind").unwrap_or("note");
                let title = item
                    .args
                    .get("title")
                    .map(|v| format!("<header>{}</header>", render_value(v)))
                    .unwrap_or_default();
                format!(
                    "<aside{attributes} class=\"notist-callout\" data-kind=\"{}\">{title}{body}</aside>",
                    escape(kind)
                )
            }
            "details" => {
                let open = if matches!(item.args.get("open"), Some(Value::Bool(true))) {
                    " open"
                } else {
                    ""
                };
                let summary = item
                    .args
                    .get("summary")
                    .map(|v| format!("<summary>{}</summary>", render_value(v)))
                    .unwrap_or_default();
                format!("<details{attributes}{open}>{summary}{body}</details>")
            }
            "quote" => {
                let attribution = item
                    .args
                    .get("attribution")
                    .map(|v| format!("<footer>{}</footer>", render_value(v)))
                    .unwrap_or_default();
                format!("<blockquote{attributes}>{body}{attribution}</blockquote>")
            }
            _ => {
                let kind = item.string("kind").unwrap_or("figure");
                let caption = item
                    .args
                    .get("caption")
                    .map(|v| format!("<figcaption>{}</figcaption>", render_value(v)))
                    .unwrap_or_default();
                format!(
                    "<figure{attributes} data-kind=\"{}\">{body}{caption}</figure>",
                    escape(kind)
                )
            }
        };
    }
    if item.name == "table" {
        return render_table(item, &attributes);
    }
    if item.name == "table-cell" {
        return render_table_cell(item, false, "default");
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
        "underline" => Some("u"),
        "strike" => Some("s"),
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

fn safe_image(source: &str) -> bool {
    !source.is_empty()
        && !source.chars().any(char::is_control)
        && source.split_once(':').is_none_or(|(scheme, _)| {
            matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https")
        })
}

fn render_table_cell(cell: &Item, header: bool, align: &str) -> String {
    let Some(Value::Content(body)) = cell.args.get("body") else {
        return error("table-cell.body requires Content");
    };
    let tag = if header { "th" } else { "td" };
    let scope = if header { " scope=\"col\"" } else { "" };
    let span = |field: &str| match cell.args.get(field) {
        None | Some(Value::Int(1)) => String::new(),
        Some(Value::Int(n)) if *n > 1 => format!(" {field}=\"{n}\""),
        _ => String::new(),
    };
    let attributes = item_attributes(cell);
    let alignment = match align {
        "left" | "center" | "right" => format!(" style=\"text-align:{align}\""),
        _ => String::new(),
    };
    format!(
        "<{tag}{attributes}{scope}{alignment}{}{}>{}</{tag}>",
        span("colspan"),
        span("rowspan"),
        render(body)
    )
}

fn render_table(table: &Item, attributes: &str) -> String {
    let Some(Value::Int(columns @ 1..=256)) = table.args.get("columns") else {
        return error("table.columns must be between 1 and 256");
    };
    let Some(Value::Content(body)) = table.args.get("body") else {
        return error("table.body requires Content");
    };
    if body.name != "seq" {
        return error("table.body requires a sequence of table cells");
    }
    let columns = *columns as usize;
    let alignments = match table.args.get("align") {
        None | Some(Value::Unit) => vec!["default"; columns],
        Some(Value::String(value)) => {
            let entries = value.split(',').map(str::trim).collect::<Vec<_>>();
            if entries.len() != columns
                || !entries
                    .iter()
                    .all(|entry| matches!(*entry, "" | "default" | "left" | "center" | "right"))
            {
                return error("invalid table alignment");
            }
            entries
        }
        _ => return error("invalid table alignment"),
    };
    let mut occupied = vec![0usize; columns];
    let mut cursor = 0;
    let mut row = String::new();
    let mut rows = Vec::new();
    let header = matches!(table.args.get("header"), Some(Value::Bool(true)));
    for cell in body.children() {
        if cell.name != "table-cell" {
            return error("table.body requires table-cell Items");
        }
        let colspan = match cell.args.get("colspan") {
            None => 1,
            Some(Value::Int(n @ 1..=256)) => *n as usize,
            _ => return error("invalid table-cell.colspan"),
        };
        let rowspan = match cell.args.get("rowspan") {
            None => 1,
            Some(Value::Int(n @ 1..=256)) => *n as usize,
            _ => return error("invalid table-cell.rowspan"),
        };
        while occupied.iter().all(|n| *n > 0) {
            rows.push(std::mem::take(&mut row));
            occupied.iter_mut().for_each(|n| *n -= 1);
            cursor = 0;
        }
        let Some(start) = (cursor..columns).find(|i| occupied[*i] == 0) else {
            return error("table cells exceed columns");
        };
        if start + colspan > columns || occupied[start..start + colspan].iter().any(|n| *n > 0) {
            return error("table-cell overlaps another cell or exceeds columns");
        }
        occupied[start..start + colspan].fill(rowspan);
        cursor = start + colspan;
        row.push_str(&render_table_cell(
            cell,
            header && rows.is_empty(),
            alignments[start],
        ));
    }
    while occupied.iter().all(|n| *n > 0) {
        rows.push(std::mem::take(&mut row));
        occupied.iter_mut().for_each(|n| *n -= 1);
    }
    if occupied.iter().any(|n| *n > 0) {
        return error("table ends with an incomplete row");
    }
    let rows = rows
        .into_iter()
        .map(|row| format!("<tr>{row}</tr>"))
        .collect::<String>();
    format!("<table{attributes}>{rows}</table>")
}
