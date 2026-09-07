//! Pipeline inspection for a single `.not` document: prints the lowered call
//! forest, the reduced forest, the shaped tree, the side annotation table and
//! the evaluation diagnostics. Development-time exploration tool; not part of
//! the CLI command surface.
//!
//! Usage: cargo run -p notist-html --example pipeline_dump -- <file.not>

use notist_eval::Evaluator;
use notist_model::{Node, NodeValue};
use notist_plugin_core::registry;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: pipeline_dump <file.not>")?;
    let source = std::fs::read_to_string(&path)?;

    let (registry, shaping) = registry();
    let evaluation = Evaluator::new(registry).evaluate_with_shaping(&source, &shaping);

    println!("== lowered ({} roots)", evaluation.lowered.len());
    print_forest(&evaluation.lowered, 0);
    println!("\n== forest ({} roots)", evaluation.forest.len());
    print_forest(&evaluation.forest, 0);
    println!("\n== tree ({} roots)", evaluation.tree.roots.len());
    print_forest(&evaluation.tree.roots, 0);

    println!("\n== annotations ({})", evaluation.annotations.len());
    for entry in &evaluation.annotations {
        println!(
            "  {}..{} {:?}",
            entry.range.start, entry.range.end, entry.attributes
        );
    }

    println!("\n== diagnostics ({})", evaluation.diagnostics.len());
    for diagnostic in &evaluation.diagnostics {
        println!(
            "  {}..{} {}",
            diagnostic.range.start, diagnostic.range.end, diagnostic.message
        );
    }
    Ok(())
}

fn print_forest(nodes: &[Node], depth: usize) {
    for node in nodes {
        let mut line = String::new();
        line.push_str(&"  ".repeat(depth));
        line.push_str(if node.block { "◼ " } else { "· " });
        line.push_str(&node.name);
        for (key, value) in &node.args {
            line.push_str(&format!(" {key}={}", format_value(value)));
        }
        println!("{line}");
        print_forest(&node.children, depth + 1);
    }
}

fn format_value(value: &NodeValue) -> String {
    match value {
        NodeValue::None => "()".to_owned(),
        NodeValue::Bool(value) => value.to_string(),
        NodeValue::Int(value) => value.to_string(),
        NodeValue::Float(value) => value.to_string(),
        NodeValue::String(value) => format!("{value:?}"),
        NodeValue::Stream(nodes) => format!("stream({})", nodes.len()),
        NodeValue::Array(values) => format!("array({})", values.len()),
        NodeValue::Target(target) => format!("{target:?}"),
    }
}
