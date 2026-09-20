use super::{Document, types::InferredType};
use notist_syntax::{self as syntax, Expr, ExprKind, Statement, Type};

pub(super) struct SymbolInfo {
    pub signature: String,
    pub documentation: String,
}

impl Document {
    pub(super) fn module_docs(&self) -> String {
        let mut end = 0;
        let mut lines = Vec::new();
        for token in &self.parsed.tokens {
            if token.kind != "comment" || !self.text[end..token.start].trim().is_empty() {
                break;
            }
            if let Some(doc) = self.text[token.start..token.end].strip_prefix("//!") {
                lines.push(doc.strip_prefix(' ').unwrap_or(doc).trim_end_matches('\r'));
            }
            end = token.end;
        }
        lines.join("\n")
    }
    pub(super) fn symbol_info(&self, at: usize, inferred: &InferredType) -> Option<SymbolInfo> {
        let mut declarations = self
            .parsed
            .statements
            .iter()
            .zip(&self.parsed.stmt_ranges)
            .filter_map(|(s, span)| match s {
                Statement::Let(name, value) => Some((name, value, span.0, span.1)),
                _ => None,
            })
            .collect::<Vec<_>>();
        for e in self.expressions() {
            if let ExprKind::Declaration(s) = &e.kind
                && let Statement::Let(name, value) = s.as_ref()
            {
                declarations.push((name, value, e.offset, e.end));
            }
        }
        for (name, value, start, _) in declarations {
            let token = self.parsed.tokens.iter().find(|t| {
                t.kind == "name"
                    && t.start >= start
                    && t.end <= value.offset
                    && &self.text[t.start..t.end] == name
            });
            if !token.is_some_and(|t| t.start <= at && at < t.end) {
                continue;
            }
            let signature_value = if let ExprKind::Typed(_, inner) = &value.kind {
                inner.as_ref()
            } else {
                value
            };
            let signature = if let ExprKind::Lambda(params, _) = &signature_value.kind {
                let params = params
                    .iter()
                    .map(|p| {
                        let default = p
                            .default
                            .as_ref()
                            .map(|e| format!(" = {}", compact(&self.text[e.offset..e.end], 120)))
                            .unwrap_or_default();
                        format!("{}: {}{default}", p.name, type_name(&p.ty))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("let {name}: ({params}) -> {}", inferred.result())
            } else {
                format!("let {name}: {inferred}")
            };
            return Some(SymbolInfo {
                signature,
                documentation: self.declaration_docs(start),
            });
        }
        None
    }

    fn declaration_docs(&self, start: usize) -> String {
        let line = self.text[..start].rfind('\n').map_or(0, |p| p + 1);
        if !self.text[line..start].trim().is_empty() {
            return String::new();
        }
        let mut cursor = start;
        let mut lines = Vec::new();
        for token in self.parsed.tokens.iter().rev().filter(|t| t.end <= start) {
            let gap = &self.text[token.end..cursor];
            if !gap.chars().all(char::is_whitespace) || gap.matches('\n').count() > 1 {
                break;
            }
            let line_start = self.text[..token.start].rfind('\n').map_or(0, |p| p + 1);
            if token.kind != "comment" || !self.text[line_start..token.start].trim().is_empty() {
                break;
            }
            let Some(doc) = self.text[token.start..token.end]
                .strip_prefix("///")
                .filter(|s| !s.starts_with('/'))
            else {
                break;
            };
            lines.push(doc.strip_prefix(' ').unwrap_or(doc).trim_end_matches('\r'));
            cursor = token.start;
        }
        lines.reverse();
        lines.join("\n")
    }
}

fn type_name(ty: &Type) -> String {
    match ty {
        Type::Optional(inner) => format!("{}?", type_name(inner)),
        Type::String => "String".into(),
        Type::Int => "Int".into(),
        Type::Bool => "Bool".into(),
        Type::Content => "Content".into(),
        Type::Item => "Item".into(),
        Type::Module => "Module".into(),
        Type::Target => "Target".into(),
        Type::List => "List".into(),
        Type::Dict => "Dict".into(),
        Type::Function => "Function".into(),
        Type::Any => "Any".into(),
        Type::None => "None".into(),
    }
}

fn compact(source: &str, limit: usize) -> String {
    if source.chars().count() <= limit {
        return source.into();
    }
    format!("{}...", source.chars().take(limit).collect::<String>())
}

pub(super) fn hover(signature: &str, docs: &str, markdown: bool) -> String {
    let header = if markdown {
        code(signature, true)
    } else {
        signature.into()
    };
    if docs.is_empty() {
        header
    } else {
        let separator = if markdown { "\n\n---\n\n" } else { "\n\n" };
        format!("{header}{separator}{}", render(docs, markdown))
    }
}

fn escape(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if "\\`*_{}[]<>()#+-.!|&".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

pub(super) fn code(text: &str, block: bool) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let ticks = "`".repeat((longest + 1).max(if block { 3 } else { 1 }));
    if block {
        format!("{ticks}\n{text}\n{ticks}")
    } else {
        format!("{ticks} {text} {ticks}")
    }
}

/// Render only declarative markup nodes, without calling the evaluator.
pub(super) fn render(source: &str, markdown: bool) -> String {
    fn all(values: &[Expr], markdown: bool) -> String {
        values.iter().map(|e| node(e, markdown)).collect()
    }
    fn node(e: &Expr, markdown: bool) -> String {
        match &e.kind {
            ExprKind::String(s) => {
                if markdown {
                    escape(s)
                } else {
                    s.clone()
                }
            }
            ExprKind::Content(v) | ExprKind::List(v) => all(v, markdown),
            ExprKind::Styled(kind, v) => {
                let body = all(v, markdown);
                let delimiter = if !markdown {
                    ""
                } else if *kind == "strong" {
                    "**"
                } else {
                    "*"
                };
                format!("{delimiter}{body}{delimiter}")
            }
            ExprKind::Section(level, title, body) => {
                let prefix = if markdown {
                    format!("{} ", "#".repeat((*level).min(6)))
                } else {
                    String::new()
                };
                format!(
                    "{prefix}{}\n\n{}",
                    all(title, markdown),
                    all(body, markdown)
                )
            }
            ExprKind::Target(path, item) => {
                let target = format!(
                    "{}{}",
                    path.join("::"),
                    item.as_ref().map(|id| format!("#{id}")).unwrap_or_default()
                );
                if markdown {
                    code(&target, false)
                } else {
                    target
                }
            }
            ExprKind::Element(name, fields) => {
                let field = |name: &str| fields.iter().find(|(key, _)| key == name).map(|(_, e)| e);
                let content =
                    |name: &str| field(name).map(|e| node(e, markdown)).unwrap_or_default();
                let flag = |name: &str| {
                    field(name).is_some_and(|e| matches!(e.kind, ExprKind::Bool(true)))
                };
                match name.as_str() {
                    "paragraph" => format!("{}\n\n", content("body")),
                    "linebreak" => {
                        if markdown {
                            "  \n".into()
                        } else {
                            "\n".into()
                        }
                    }
                    "parbreak" => "\n\n".into(),
                    "raw" | "math" => {
                        let raw = field("content")
                            .and_then(|e| {
                                if let ExprKind::String(s) = &e.kind {
                                    Some(s.as_str())
                                } else {
                                    None
                                }
                            })
                            .unwrap_or("");
                        let rendered = if markdown {
                            code(raw, flag("block"))
                        } else {
                            raw.into()
                        };
                        if flag("block") {
                            format!("{rendered}\n\n")
                        } else {
                            rendered
                        }
                    }
                    "list" | "terms" => {
                        let Some(Expr {
                            kind: ExprKind::Content(items),
                            ..
                        }) = field("body")
                        else {
                            return String::new();
                        };
                        let mut out = String::new();
                        let mut number = 1;
                        for item in items {
                            if let ExprKind::Element(_, fields) = &item.kind
                                && let Some((
                                    _,
                                    Expr {
                                        kind: ExprKind::Int(n),
                                        ..
                                    },
                                )) = fields.iter().find(|(key, _)| key == "number")
                            {
                                number = *n;
                            }
                            let prefix = if flag("ordered") {
                                format!("{number}. ")
                            } else {
                                "- ".into()
                            };
                            let body = node(item, markdown);
                            let mut lines = body.trim_end().lines();
                            out.push_str(&prefix);
                            out.push_str(lines.next().unwrap_or(""));
                            out.push('\n');
                            for line in lines {
                                out.push_str(&" ".repeat(prefix.len()));
                                out.push_str(line);
                                out.push('\n');
                            }
                            number = number.saturating_add(1);
                        }
                        format!("{out}\n")
                    }
                    "term-item" => format!("{}: {}", content("term"), content("body")),
                    "list-item" => content("body"),
                    "link" => {
                        let dest = field("dest")
                            .and_then(|e| {
                                if let ExprKind::String(s) = &e.kind {
                                    Some(s.as_str())
                                } else {
                                    None
                                }
                            })
                            .unwrap_or("");
                        if markdown
                            && let Ok(url) = url::Url::parse(dest)
                            && matches!(url.scheme(), "http" | "https")
                        {
                            return format!(
                                "[{}](<{}>)",
                                content("body"),
                                url.as_str().replace('<', "%3C").replace('>', "%3E")
                            );
                        }
                        content("body")
                    }
                    "smartquote" => {
                        if flag("double") {
                            "\"".into()
                        } else {
                            "'".into()
                        }
                    }
                    _ => content("body"),
                }
            }
            _ => String::new(),
        }
    }
    match syntax::parse_documentation(source) {
        Ok(values) => all(&values, markdown).trim().into(),
        Err(_) => {
            if markdown {
                escape(source)
            } else {
                source.into()
            }
        }
    }
}
