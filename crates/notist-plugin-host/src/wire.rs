//! Plugin ABI payload helpers over the unified node.
//!
//! The wire schema IS [`notist_model::Node`]: the SDK version doubles as the
//! payload ABI version because host and plugins compiled against the same
//! `notist_model` produce and accept the same bytes. This module only adds
//! host-side conversion and budget validation.

use notist_eval::{
    Argument, Call, CallContent, CallNode, Value, instances_to_legacy_content,
    legacy_content_to_nodes,
};
use notist_model::{Node, NodeValue, node_from_instance, node_to_instance};

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
        Value::Content(content) => NodeValue::Stream(
            legacy_content_to_nodes(content)
                .iter()
                .map(node_from_instance)
                .collect(),
        ),
        Value::Function(_) => {
            return Err("function values cannot cross the plugin boundary".into());
        }
    })
}

/// Converts a reduced node back into an evaluator value.
#[allow(dead_code)]
pub(crate) fn node_value_to_value(value: &NodeValue) -> Result<Value, String> {
    Ok(match value {
        NodeValue::None => Value::None,
        NodeValue::Bool(value) => Value::Bool(*value),
        NodeValue::Int(value) => Value::Int(*value),
        NodeValue::Float(value) => Value::Float(*value),
        NodeValue::String(value) => Value::String(value.clone()),
        NodeValue::Stream(stream) => {
            let instances = stream
                .iter()
                .map(node_to_instance)
                .collect::<Result<Vec<_>, String>>()?;
            Value::Content(
                instances_to_legacy_content(&instances)
                    .ok_or_else(|| "node stream cannot project to legacy content".to_owned())?,
            )
        }
        NodeValue::Array(values) => {
            let converted = values
                .iter()
                .map(node_value_to_value)
                .collect::<Result<Vec<_>, String>>()?;
            // Arrays ride as content streams of nothing today; represent them
            // lossily as none until Array enters the eval value domain.
            let _ = converted;
            Value::None
        }
    })
}

/// Converts one unified node into the legacy call form for reduction frames
/// that still speak `CallContent`.
pub(crate) fn node_to_legacy_call(
    node: &Node,
    registry: &notist_eval::FunctionRegistry,
) -> Result<Call, String> {
    let arguments = node
        .args
        .iter()
        .map(|(name, value)| {
            Ok(Argument {
                name: name.clone(),
                value: node_value_to_value(value)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    // Legacy `Call.body` carries already-reduced content; unreduced child
    // calls cannot be represented here, so they are rejected at this edge
    // (the node engine handles them natively).
    let body = (!node.children.is_empty())
        .then(|| -> Result<CallContent, String> {
            let mut content = CallContent::new();
            for child in &node.children {
                if registry.get(&child.name).is_some() {
                    return Err("unreduced call inside legacy call body".into());
                }
                let instance = node_to_instance(child)?;
                content.nodes.extend(
                    instances_to_legacy_content(&[instance])
                        .ok_or_else(|| "child cannot project to legacy content".to_owned())?
                        .elements
                        .into_iter()
                        .map(CallNode::Element),
                );
            }
            Ok(content)
        })
        .transpose()?;
    Ok(Call {
        name: node.name.clone(),
        arguments,
        body,
        range: node.range,
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
        let body = input.arguments.take_content(trailing_name);
        node.children = legacy_content_to_nodes(&body)
            .iter()
            .map(node_from_instance)
            .collect();
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

/// Wraps one legacy call into the `CallContent` consumed by reduction frames.
pub(crate) fn call_content_of(call: Call) -> CallContent {
    CallContent {
        nodes: vec![CallNode::Call(call)],
    }
}
