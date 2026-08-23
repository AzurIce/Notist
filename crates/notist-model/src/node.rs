//! The unified content node.
//!
//! `Node` is the single representation of evaluated content: a call awaiting
//! reduction and a terminal leaf are the same shape, differing only in phase.
//! Reduction is a fixpoint iteration — a handler registered for `name`
//! replaces the node with its output, and a node nobody handles *is* a leaf.

use crate::{ElementName, FieldValue, TextRange};

/// The unified content representation.
///
/// One type serves every pipeline phase:
///
/// - **call phase**: `name` addresses a handler; `args` may carry pending
///   values ([`NodeValue::Stream`]); `children` is the trailing body stream;
/// - **leaf phase**: no handler answers for `name`; `args` are concrete data,
///   `children` are already-reduced nodes, and the projection layer renders
///   the node from its name and args alone.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Node {
    /// Qualified constructor identity (`core::text`, `demo::box`).
    pub name: String,
    /// Named arguments. Pending values must be reduced before a handler for
    /// this node is invoked; leaves only ever carry concrete values.
    pub args: Vec<(String, NodeValue)>,
    /// Trailing body / nested children in source order.
    pub children: Vec<Node>,
    /// Whether this node interrupts paragraph flow.
    pub block: bool,
    /// Source range responsible for this node. Host-side metadata only;
    /// never meaningful across the plugin boundary.
    #[serde(default)]
    pub range: TextRange,
}

impl Node {
    /// Creates an empty inline node addressed to `name`.
    pub fn call(name: impl Into<String>, range: TextRange) -> Self {
        Self {
            name: name.into(),
            args: Vec::new(),
            children: Vec::new(),
            block: false,
            range,
        }
    }

    /// Creates an empty block node addressed to `name`.
    pub fn block_call(name: impl Into<String>, range: TextRange) -> Self {
        let mut node = Self::call(name, range);
        node.block = true;
        node
    }

    /// Appends one argument.
    pub fn arg(mut self, name: impl Into<String>, value: impl Into<NodeValue>) -> Self {
        self.args.push((name.into(), value.into()));
        self
    }

    /// Appends one child node.
    pub fn child(mut self, child: Node) -> Self {
        self.children.push(child);
        self
    }

    /// Returns the last value bound to `name`, if any.
    pub fn get(&self, name: &str) -> Option<&NodeValue> {
        self.args
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }

    /// Whether any argument still carries a pending stream.
    pub fn has_pending_args(&self) -> bool {
        self.args.iter().any(|(_, value)| value.is_pending())
    }
}

/// One argument value on the unified node.
///
/// [`NodeValue::Stream`] is the pending state: a not-yet-reduced child
/// stream. Once reduction reaches fixpoint every value is concrete, so
/// downstream consumers (shaping, projection) never observe it.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum NodeValue {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Stream(Vec<Node>),
    Array(Vec<NodeValue>),
}

impl NodeValue {
    /// Whether this value still hides an unreduced stream.
    pub fn is_pending(&self) -> bool {
        match self {
            Self::Stream(nodes) => !nodes.is_empty(),
            _ => false,
        }
    }
}

impl From<bool> for NodeValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for NodeValue {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<f64> for NodeValue {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<&str> for NodeValue {
    fn from(value: &str) -> Self {
        Self::String(value.into())
    }
}

impl From<String> for NodeValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<Vec<Node>> for NodeValue {
    fn from(nodes: Vec<Node>) -> Self {
        Self::Stream(nodes)
    }
}

/// Converts one terminal instance into the unified representation.
pub fn node_from_instance(instance: &crate::InstanceNode) -> Node {
    Node {
        name: instance.instance.name.to_string(),
        args: instance
            .instance
            .fields
            .iter()
            .map(|field| (field.name.clone(), NodeValue::from(&field.value)))
            .collect(),
        children: instance
            .instance
            .body
            .iter()
            .map(node_from_instance)
            .collect(),
        block: instance.instance.block,
        range: instance.range,
    }
}

/// Converts one unified node back into the terminal instance form.
///
/// Pending streams inside arguments are rejected: they must be reduced before
/// a node can become an instance.
pub fn node_to_instance(node: &Node) -> Result<crate::InstanceNode, String> {
    let mut instance = crate::ElementInstance::new(ElementName::parse(&node.name), node.block);
    for (name, value) in &node.args {
        instance.fields.push(crate::Field::new(
            name.clone(),
            field_value_from_node_value(value)?,
        ));
    }
    for child in &node.children {
        instance.body.push(node_to_instance(child)?);
    }
    Ok(crate::InstanceNode::ranged(instance, node.range))
}

fn field_value_from_node_value(value: &NodeValue) -> Result<FieldValue, String> {
    Ok(match value {
        NodeValue::None => FieldValue::None,
        NodeValue::Bool(value) => FieldValue::Bool(*value),
        NodeValue::Int(value) => FieldValue::Int(*value),
        NodeValue::Float(value) => FieldValue::Float(*value),
        NodeValue::String(value) => FieldValue::String(value.clone()),
        NodeValue::Stream(nodes) => FieldValue::Content(
            nodes
                .iter()
                .map(node_to_instance)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        NodeValue::Array(values) => FieldValue::Array(
            values
                .iter()
                .map(field_value_from_node_value)
                .collect::<Result<Vec<_>, _>>()?,
        ),
    })
}

impl From<&FieldValue> for NodeValue {
    fn from(value: &FieldValue) -> Self {
        match value {
            FieldValue::None => Self::None,
            FieldValue::Bool(value) => Self::Bool(*value),
            FieldValue::Int(value) => Self::Int(*value),
            FieldValue::Float(value) => Self::Float(*value),
            FieldValue::String(value) => Self::String(value.clone()),
            FieldValue::Content(nodes) => {
                Self::Stream(nodes.iter().map(node_from_instance).collect())
            }
            FieldValue::Array(values) => Self::Array(values.iter().map(Self::from).collect()),
        }
    }
}

/// Wire helpers over the unified node.
///
/// The SDK crate version doubles as the payload ABI version: host and
/// plugins compiled against the same `notist_model` produce and accept the
/// same bytes. No separate protocol version exists.
pub mod wire {
    use super::Node;

    /// Serializes one node tree onto the plugin ABI.
    pub fn encode(value: &Node) -> Result<Vec<u8>, String> {
        serde_json::to_vec(value).map_err(|error| format!("encode node payload: {error}"))
    }

    /// Deserializes one node tree from the plugin ABI.
    pub fn decode(bytes: &[u8]) -> Result<Node, String> {
        serde_json::from_slice(bytes).map_err(|error| format!("decode node payload: {error}"))
    }

    /// Serializes a forest (reduction responses carry many roots).
    pub fn encode_forest(nodes: &[Node]) -> Result<Vec<u8>, String> {
        serde_json::to_vec(nodes).map_err(|error| format!("encode node forest: {error}"))
    }

    /// Deserializes a forest from the plugin ABI.
    pub fn decode_forest(bytes: &[u8]) -> Result<Vec<Node>, String> {
        serde_json::from_slice(bytes).map_err(|error| format!("decode node forest: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ElementInstance, InstanceNode};

    #[test]
    fn builder_shapes_a_call_node() {
        let node = Node::block_call("core::details", TextRange::new(0, 5))
            .arg("open", true)
            .arg("summary", "Shader")
            .child(Node::call("core::text", TextRange::new(6, 9)).arg("text", "hi"));
        assert_eq!(node.name, "core::details");
        assert!(node.block);
        assert_eq!(node.get("summary"), Some(&NodeValue::from("Shader")));
        assert_eq!(node.children.len(), 1);
        assert!(!node.has_pending_args());
    }

    #[test]
    fn instance_round_trips_through_node() {
        let mut instance = ElementInstance::new(ElementName::parse("core::heading"), true);
        instance
            .fields
            .push(crate::Field::new("level", FieldValue::Int(2)));
        instance.fields.push(crate::Field::new(
            "meta",
            FieldValue::Array(vec![FieldValue::String("a".into()), FieldValue::None]),
        ));
        let mut child = ElementInstance::new(ElementName::parse("core::text"), false);
        child
            .fields
            .push(crate::Field::new("text", FieldValue::String("hi".into())));
        instance.body.push(InstanceNode::synthetic(child));
        let original = InstanceNode::synthetic(instance);

        let node = node_from_instance(&original);
        assert_eq!(node.name, "core::heading");
        assert!(node.block);

        let back = node_to_instance(&node).unwrap();
        assert_eq!(back.instance, original.instance);
    }

    #[test]
    fn stream_args_convert_to_nested_content() {
        let node = Node::call("demo::box", TextRange::new(0, 1)).arg(
            "body",
            vec![Node::call("core::text", TextRange::new(0, 1)).arg("text", "hi")],
        );
        assert!(node.has_pending_args());

        let instance = node_to_instance(&node).unwrap();
        assert!(matches!(
            instance.instance.fields[0].value,
            FieldValue::Content(_)
        ));
        // And back again: the round trip preserves the nested shape.
        let returned = node_from_instance(&instance);
        assert!(returned.args[0].1.is_pending());
        assert_eq!(
            returned.args[0].1,
            NodeValue::Stream(vec![
                Node::call("core::text", TextRange::new(0, 1)).arg("text", "hi")
            ])
        );
    }
}
