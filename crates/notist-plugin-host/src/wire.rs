//! Plugin ABI payload helpers over the unified node.
//!
//! The wire schema IS [`notist_model::Node`]: the SDK version doubles as the
//! payload ABI version because host and plugins compiled against the same
//! `notist_model` produce and accept the same bytes. This module only adds
//! host-side conversion and budget validation.

use notist_eval::Value;
use notist_model::{Node, NodeValue};

pub(crate) use notist_model::wire as codec;

/// Maximum encoded bytes accepted from one component response.
pub(crate) const MAX_COMPONENT_RESPONSE_BYTES: usize = 1024 * 1024;

/// Maximum nodes accepted in one component response stream.
pub(crate) const MAX_COMPONENT_RESPONSE_NODES: usize = 10_000;

/// Counts every node in a forest, descending through children and args.
fn count_nodes(nodes: &[Node]) -> usize {
    fn count(value: &NodeValue) -> usize {
        match value {
            NodeValue::Stream(stream) => count_nodes(stream),
            NodeValue::Array(values) => values.iter().map(count).sum(),
            _ => 0,
        }
    }
    nodes
        .iter()
        .map(|node| {
            1 + count_nodes(&node.children) + node.args.iter().map(|(_, v)| count(v)).sum::<usize>()
        })
        .sum()
}

/// Validates decoded response size and node count before host reduction.
pub(crate) fn validate_forest(nodes: &[Node]) -> Result<(), String> {
    let bytes_ignored = 0usize;
    let _ = bytes_ignored;
    let count = count_nodes(nodes);
    if count > MAX_COMPONENT_RESPONSE_NODES {
        return Err(format!(
            "plugin returned {count} nodes, exceeding the limit of {MAX_COMPONENT_RESPONSE_NODES}"
        ));
    }
    Ok(())
}

/// Converts one evaluator value into its node representation.
///
/// Function values cannot live on content nodes.
pub(crate) fn value_to_node_value(value: &Value) -> Result<NodeValue, String> {
    Ok(match value {
        Value::None => NodeValue::None,
        Value::Bool(value) => NodeValue::Bool(*value),
        Value::Int(value) => NodeValue::Int(*value),
        Value::Float(value) => NodeValue::Float(*value),
        Value::String(value) => NodeValue::String(value.clone()),
        Value::Content(forest) => NodeValue::Stream(forest.clone()),
        Value::Function(_) => {
            return Err("function values cannot cross the plugin boundary".into());
        }
    })
}

/// Builds the request node for one dispatch from bound arguments.
pub(crate) fn build_request_node(
    name: &str,
    signature: &notist_eval::FunctionSignature,
    input: &mut notist_eval::FunctionInput<'_>,
) -> Result<Node, String> {
    let trailing = signature.trailing_content.clone();
    let mut node = Node::call(name, input.range);
    for (arg_name, value) in input.arguments.iter() {
        if Some(arg_name) == trailing.as_deref() {
            continue;
        }
        node.args
            .push((arg_name.to_owned(), value_to_node_value(value)?));
    }
    if let Some(trailing_name) = trailing.as_deref() {
        node.children = input.arguments.take_content(trailing_name);
    }
    Ok(node)
}

/// Decodes and validates one component response forest.
pub(crate) fn decode_response(
    response: &[u8],
    range: notist_model::TextRange,
) -> Result<Vec<Node>, Vec<notist_eval::EvalDiagnostic>> {
    use notist_eval::EvalDiagnostic;
    if response.len() > MAX_COMPONENT_RESPONSE_BYTES {
        return Err(vec![EvalDiagnostic {
            message: format!(
                "wasm component response exceeded the limit of {MAX_COMPONENT_RESPONSE_BYTES} bytes"
            ),
            range,
        }]);
    }
    let nodes = codec::decode_forest(response)
        .map_err(|message| vec![EvalDiagnostic { message, range }])?;
    validate_forest(&nodes).map_err(|message| vec![EvalDiagnostic { message, range }])?;
    Ok(nodes)
}
