//! Serde wire schema carried inside the bytes-in/bytes-out component ABI.
//!
//! The recursive `Stream<Call | Leaf>` shape cannot be expressed directly in
//! WIT, so the component boundary carries JSON-encoded [`WireNode`] values.
//! Conversion helpers mirror the host-side `notist-eval` call and leaf types.

use notist_eval::{
    Argument, Call, CallContent, CallNode, Value, instances_to_legacy_content,
    legacy_content_to_nodes,
};
use notist_model::{ElementInstance, ElementName, FieldValue, InstanceNode, TextRange};
use serde::{Deserialize, Serialize};

/// One value on the plugin ABI. Functions never cross the boundary.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum WireValue {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Stream(Vec<WireNode>),
    Array(Vec<WireValue>),
}

/// One named argument on a wire call.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WireArgument {
    pub name: String,
    pub value: WireValue,
}

/// One call in the wire stream.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WireCall {
    pub name: String,
    #[serde(default)]
    pub arguments: Vec<WireArgument>,
    #[serde(default)]
    pub body: Option<Vec<WireNode>>,
}

/// One named field on a wire leaf.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WireField {
    pub name: String,
    pub value: WireValue,
}

/// One terminal leaf in the wire stream.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WireLeaf {
    pub name: String,
    #[serde(default)]
    pub fields: Vec<WireField>,
    #[serde(default)]
    pub body: Vec<WireNode>,
    pub block: bool,
}

/// A plugin-visible stream node: `Call | Leaf`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireNode {
    Call(WireCall),
    Leaf(WireLeaf),
}

/// Serializes a call request for `evaluate` / `host.call`.
pub fn encode_call(call: &WireCall) -> Result<Vec<u8>, String> {
    serde_json::to_vec(call).map_err(|error| format!("cannot encode wire call: {error}"))
}

/// Decodes a call request from `evaluate` / `host.call`.
pub fn decode_call(bytes: &[u8]) -> Result<WireCall, String> {
    serde_json::from_slice(bytes).map_err(|error| format!("invalid wire call: {error}"))
}

/// Serializes a reduced Leaf stream response.
pub fn encode_nodes(nodes: &[WireNode]) -> Result<Vec<u8>, String> {
    serde_json::to_vec(nodes).map_err(|error| format!("cannot encode wire stream: {error}"))
}

/// Decodes a reduced Leaf stream response.
pub fn decode_nodes(bytes: &[u8]) -> Result<Vec<WireNode>, String> {
    serde_json::from_slice(bytes).map_err(|error| format!("invalid wire stream: {error}"))
}

/// Validates decoded response size and node count before host reduction.
pub fn validate_nodes(nodes: &[WireNode], max_nodes: usize) -> Result<(), String> {
    fn count(nodes: &[WireNode]) -> usize {
        nodes
            .iter()
            .map(|node| match node {
                WireNode::Leaf(leaf) => 1 + count(&leaf.body),
                WireNode::Call(call) => 1 + call.body.as_deref().map(count).unwrap_or(0),
            })
            .sum()
    }
    let count = count(nodes);
    if count > max_nodes {
        return Err(format!(
            "plugin returned {count} nodes, exceeding the limit of {max_nodes}"
        ));
    }
    Ok(())
}

/// Converts a wire call into the legacy call representation consumed by the
/// shared reduction engine.
pub fn wire_call_to_legacy(call: &WireCall) -> Result<Call, String> {
    Ok(Call {
        name: call.name.clone(),
        arguments: call
            .arguments
            .iter()
            .map(|argument| {
                Ok(Argument {
                    name: argument.name.clone(),
                    value: wire_value_to_eval(&argument.value)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?,
        body: call
            .body
            .as_deref()
            .map(wire_nodes_to_call_content)
            .transpose()?,
        range: TextRange::new(0, 0),
    })
}

/// Converts a wire stream into legacy call content for reduction.
pub fn wire_nodes_to_call_content(nodes: &[WireNode]) -> Result<CallContent, String> {
    let mut output = CallContent::new();
    for node in nodes {
        match node {
            WireNode::Call(call) => output
                .nodes
                .push(CallNode::Call(wire_call_to_legacy(call)?)),
            WireNode::Leaf(leaf) => {
                let instance = wire_leaf_to_instance(leaf)?;
                output.nodes.extend(
                    instances_to_legacy_content(&[instance])
                        .ok_or_else(|| "wire leaf cannot project to legacy content".to_owned())?
                        .elements
                        .into_iter()
                        .map(CallNode::Element),
                );
            }
        }
    }
    Ok(output)
}

/// Converts reduced host content back into the wire stream representation.
pub fn content_to_wire_nodes(content: &notist_model::Content) -> Result<Vec<WireNode>, String> {
    Ok(legacy_content_to_nodes(content)
        .iter()
        .map(instance_to_wire)
        .collect())
}

/// Converts an evaluated value into its wire representation.
pub fn eval_value_to_wire(value: &Value) -> Result<WireValue, String> {
    match value {
        Value::None => Ok(WireValue::None),
        Value::Bool(value) => Ok(WireValue::Bool(*value)),
        Value::Int(value) => Ok(WireValue::Int(*value)),
        Value::Float(value) => Ok(WireValue::Float(*value)),
        Value::String(value) => Ok(WireValue::String(value.clone())),
        Value::Content(content) => Ok(WireValue::Stream(content_to_wire_nodes(content)?)),
        Value::Function(_) => Err("Function values cannot cross the plugin boundary".into()),
    }
}

fn wire_value_to_eval(value: &WireValue) -> Result<Value, String> {
    match value {
        WireValue::None => Ok(Value::None),
        WireValue::Bool(value) => Ok(Value::Bool(*value)),
        WireValue::Int(value) => Ok(Value::Int(*value)),
        WireValue::Float(value) => Ok(Value::Float(*value)),
        WireValue::String(value) => Ok(Value::String(value.clone())),
        WireValue::Stream(nodes) => {
            let mut instances = Vec::new();
            for node in nodes {
                match node {
                    WireNode::Leaf(leaf) => instances.push(wire_leaf_to_instance(leaf)?),
                    WireNode::Call(_) => {
                        return Err("unreduced Call inside a value stream".into());
                    }
                }
            }
            instances_to_legacy_content(&instances)
                .map(Value::Content)
                .ok_or_else(|| "wire stream cannot project to legacy content".into())
        }
        WireValue::Array(_) => Err("Array values are not yet representable in eval Value".into()),
    }
}

/// Converts one Leaf instance to its wire representation.
pub fn instance_to_wire(instance: &InstanceNode) -> WireNode {
    WireNode::Leaf(WireLeaf {
        name: instance.instance.name.to_string(),
        fields: instance
            .instance
            .fields
            .iter()
            .map(|field| WireField {
                name: field.name.clone(),
                value: field_value_to_wire(&field.value),
            })
            .collect(),
        body: instance
            .instance
            .body
            .iter()
            .map(instance_to_wire)
            .collect(),
        block: instance.instance.block,
    })
}

fn wire_leaf_to_instance(leaf: &WireLeaf) -> Result<InstanceNode, String> {
    let mut instance = ElementInstance::new(ElementName::parse(&leaf.name), leaf.block);
    for field in &leaf.fields {
        instance.fields.push(notist_model::Field {
            name: field.name.clone(),
            value: wire_value_to_field(&field.value)?,
        });
    }
    instance.body = leaf
        .body
        .iter()
        .map(|node| match node {
            WireNode::Leaf(leaf) => wire_leaf_to_instance(leaf),
            WireNode::Call(_) => Err("unreduced Call inside a Leaf body".to_owned()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(InstanceNode::synthetic(instance))
}

fn field_value_to_wire(value: &FieldValue) -> WireValue {
    match value {
        FieldValue::None => WireValue::None,
        FieldValue::Bool(value) => WireValue::Bool(*value),
        FieldValue::Int(value) => WireValue::Int(*value),
        FieldValue::Float(value) => WireValue::Float(*value),
        FieldValue::String(value) => WireValue::String(value.clone()),
        FieldValue::Content(nodes) => {
            WireValue::Stream(nodes.iter().map(instance_to_wire).collect())
        }
        FieldValue::Array(values) => {
            WireValue::Array(values.iter().map(field_value_to_wire).collect())
        }
    }
}

fn wire_value_to_field(value: &WireValue) -> Result<FieldValue, String> {
    match value {
        WireValue::None => Ok(FieldValue::None),
        WireValue::Bool(value) => Ok(FieldValue::Bool(*value)),
        WireValue::Int(value) => Ok(FieldValue::Int(*value)),
        WireValue::Float(value) => Ok(FieldValue::Float(*value)),
        WireValue::String(value) => Ok(FieldValue::String(value.clone())),
        WireValue::Stream(nodes) => {
            let instances = nodes
                .iter()
                .map(|node| match node {
                    WireNode::Leaf(leaf) => wire_leaf_to_instance(leaf),
                    WireNode::Call(_) => Err("unreduced Call inside a field value".to_owned()),
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(FieldValue::Content(instances))
        }
        WireValue::Array(values) => values
            .iter()
            .map(wire_value_to_field)
            .collect::<Result<Vec<_>, _>>()
            .map(FieldValue::Array),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_call_and_leaf_streams() {
        let call = WireCall {
            name: "core::text".into(),
            arguments: vec![WireArgument {
                name: "text".into(),
                value: WireValue::String("hello".into()),
            }],
            body: None,
        };
        let bytes = encode_call(&call).unwrap();
        let decoded = decode_call(&bytes).unwrap();
        assert_eq!(decoded.name, "core::text");

        let leaf = instance_to_wire(&InstanceNode::synthetic(ElementInstance::text("leaf")));
        let bytes = encode_nodes(&[leaf]).unwrap();
        let decoded = decode_nodes(&bytes).unwrap();
        assert!(matches!(decoded[0], WireNode::Leaf(_)));
    }
}
