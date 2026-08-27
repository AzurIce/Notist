//! Captures real plugin dispatches as fixture bytes for the JS conformance
//! test.
//!
//! Builds the same request forests the eval host would encode
//! (`wire::build_request_node` → `encode_forest`), runs them through the
//! SDK's real guest dispatch, and writes per-plugin fixture dirs:
//! `<out>/<plugin>/{request,response,declarations}.bin`.

use notist_model::{Node, NodeValue, TextRange};
use notist_plugin_sdk::{wire, GuestState};

pub fn capture(
    out_dir: &str,
    package: &str,
    state: GuestState,
    request_node: Node,
) -> Result<(), String> {
    let dir = format!("{out_dir}/{package}");
    std::fs::create_dir_all(&dir).map_err(|error| format!("mkdir {dir}: {error}"))?;

    let request = wire::encode_forest(std::slice::from_ref(&request_node))?;
    let response = notist_plugin_sdk::evaluate_dispatch(&state, package, request.clone())?;
    let declarations = wire::encode_declarations(&state.declarations)?;

    for (name, bytes) in [
        ("request.bin", &request),
        ("response.bin", &response),
        ("declarations.bin", &declarations),
    ] {
        std::fs::write(format!("{dir}/{name}"), bytes)
            .map_err(|error| format!("write {dir}/{name}: {error}"))?;
    }
    println!(
        "{package}: request {} B, response {} B, declarations {} B -> {dir}",
        request.len(),
        response.len(),
        declarations.len()
    );
    Ok(())
}

pub fn shader_request() -> Node {
    // Mirrors `#shader[source="void main() {}"]{ ... }`:
    // block call, string arg, two defaulted int args, one body child.
    let mut body_child = Node::call("core::text", TextRange::new(30, 42));
    body_child
        .args
        .push(("value".to_owned(), NodeValue::from("precision mediump float;")));

    let mut request = Node::block_call("shader::shader", TextRange::new(10, 60));
    request
        .args
        .push(("source".to_owned(), NodeValue::from("void main() {}")));
    request.args.push(("width".to_owned(), NodeValue::Int(1024)));
    request.args.push(("height".to_owned(), NodeValue::Int(768)));
    request.children.push(body_child);
    request
}

pub fn mermaid_request() -> Result<Node, String> {
    // Mirrors `#mermaid[source="graph TD\n  A --> B"]{ ... }`.
    let mut body_child = Node::call("core::text", TextRange::new(40, 52));
    body_child
        .args
        .push(("value".to_owned(), NodeValue::from("flow downstream")));

    let mut request = Node::block_call("mermaid::mermaid", TextRange::new(12, 70));
    request.args.push((
        "source".to_owned(),
        NodeValue::from("graph TD\n  A --> B"),
    ));
    request.args.push(("theme".to_owned(), NodeValue::from("default")));
    request.children.push(body_child);
    Ok(request)
}
