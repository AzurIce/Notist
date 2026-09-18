//! 产物 dump：golden 对拍用的文本形态。
//!
//! 格式稳定、可 diff、人眼可读；体渲染为子行，其他实参渲染为内联值。

use crate::ir::{CoreElement, Diagnostic, ElementName, Item, ItemState, ModuleResult, Value};
use crate::package::{PackageGraph, PackageReport};

/// 渲染一个模块的产物。
pub fn dump(result: &ModuleResult) -> String {
    let mut out = String::from("module\n");
    for item in &result.content.items {
        write_item(item, 2, &mut out);
    }
    out.push_str("bindings\n");
    for (name, value) in &result.bindings {
        out.push_str(&format!("  {name} = {}\n", compact_value(value)));
    }
    out.push_str("diagnostics\n");
    write_diagnostics(&result.diagnostics, &mut out);
    out
}

/// 渲染一个包的接口与实现解析结果。
pub fn dump_package(report: &PackageReport) -> String {
    let mut out = format!(
        "package {} {}\nentry {}\n",
        report.name,
        report.version,
        report.entry.as_deref().unwrap_or("<none>")
    );
    out.push_str("exports\n");
    for export in &report.exports {
        out.push_str("  ");
        out.push_str(&export.canonical());
        out.push('\n');
    }
    match &report.implementation {
        None => out.push_str("implementation none\n"),
        Some(implementation) => {
            out.push_str(&format!(
                "implementation {} {}\n",
                implementation.kind, implementation.artifact
            ));
            out.push_str(&format!(
                "  interface {}\n",
                if implementation.interface_match {
                    "match"
                } else {
                    "mismatch"
                }
            ));
            out.push_str("  symbols\n");
            for (declared, symbol) in &implementation.symbols {
                match symbol {
                    Some(symbol) => out.push_str(&format!("    {declared} -> {symbol}\n")),
                    None => out.push_str(&format!("    {declared} -> missing\n")),
                }
            }
        }
    }
    out.push_str("diagnostics\n");
    write_diagnostics(&report.diagnostics, &mut out);
    out
}

/// 渲染一棵依赖树：包清单、导出数、实现状态与依赖边。
pub fn dump_graph(graph: &PackageGraph) -> String {
    let mut out = format!("workspace\nroot {}\n", graph.root);
    out.push_str("packages\n");
    for package in &graph.packages {
        out.push_str(&format!(
            "  {} {} exports={} implementation={}\n",
            package.name,
            package.version,
            package.exports,
            package.implementation.as_str()
        ));
    }
    out.push_str("edges\n");
    for (from, to) in &graph.edges {
        out.push_str(&format!("  {from} -> {to}\n"));
    }
    out.push_str("diagnostics\n");
    write_diagnostics(&graph.diagnostics, &mut out);
    out
}

fn write_diagnostics(diagnostics: &[Diagnostic], out: &mut String) {
    let mut diagnostics: Vec<_> = diagnostics.iter().collect();
    diagnostics.sort_by_key(|diagnostic| {
        (diagnostic.range.start, diagnostic.range.end, diagnostic.code)
    });
    for diagnostic in diagnostics {
        out.push_str(&format!(
            "  {} {} [{}..{}] {}\n",
            diagnostic.severity.as_str(),
            diagnostic.code,
            diagnostic.range.start,
            diagnostic.range.end,
            diagnostic.message
        ));
    }
}

fn write_item(item: &Item, indent: usize, out: &mut String) {
    if let Some(text) = text_literal(item) {
        out.push_str(&format!("{:indent$}text({text:?})\n", ""));
        return;
    }
    out.push_str(&" ".repeat(indent));
    out.push_str(&head(item, true));
    out.push('\n');
    if let Some(Value::Content(content)) = item.arg("body") {
        for child in &content.items {
            write_item(child, indent + 2, out);
        }
    }
}

/// 单行形态，用于内联渲染。
fn compact(item: &Item) -> String {
    match text_literal(item) {
        Some(text) => format!("text({text:?})"),
        None => head(item, false),
    }
}

fn head(item: &Item, skip_body: bool) -> String {
    let mut out = item.name.to_string();
    if item.state == ItemState::Degraded {
        out.push_str("[degraded]");
    }
    let mut parts = Vec::new();
    for (name, value) in &item.args {
        if skip_body && name == "body" {
            continue;
        }
        parts.push(format!("{name}={}", compact_value(value)));
    }
    if !parts.is_empty() {
        out.push_str(&format!("({})", parts.join(", ")));
    }
    out
}

fn text_literal(item: &Item) -> Option<&str> {
    match (&item.name, item.arg("text")) {
        (ElementName::Core(CoreElement::Text), Some(Value::String(text)))
            if item.state == ItemState::Resolved =>
        {
            Some(text)
        }
        _ => None,
    }
}

/// 值的单行形态，用于 dump 与表达式渲染。
pub fn compact_value(value: &Value) -> String {
    match value {
        Value::Unit => "()".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Int(value) => value.to_string(),
        Value::Float(value) => value.to_string(),
        Value::String(value) => format!("{value:?}"),
        Value::Content(content) => {
            let items: Vec<String> = content.items.iter().map(compact).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Function(function) => format!("<fn{}>", function.signature_text()),
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(compact_value).collect();
            format!("array({})", items.join(", "))
        }
        Value::Dict(pairs) => {
            let pairs: Vec<String> = pairs
                .iter()
                .map(|(key, value)| format!("{key}: {}", compact_value(value)))
                .collect();
            format!("dict({})", pairs.join(", "))
        }
    }
}
