//! The unified content node.
//!
//! `Node` is the single representation of evaluated content: a call awaiting
//! reduction and a terminal leaf are the same shape, differing only in phase.
//! Reduction is a fixpoint iteration — a handler registered for `name`
//! replaces the node with its output, and a node nobody handles *is* a leaf.

use crate::TextRange;

/// Horizontal alignment applied to one table column.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TableAlignment {
    /// Use the renderer's default table alignment.
    #[default]
    Default,
    /// Align cell content to the left.
    Left,
    /// Center cell content.
    Center,
    /// Align cell content to the right.
    Right,
}

/// A table cell placed at its logical starting column.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TableCellPlacement {
    /// Index into the table's row-major cell vector.
    pub cell_index: usize,
    /// Zero-based logical starting column after accounting for active row spans.
    pub column: u16,
}

/// A structural table-layout error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableLayoutError {
    /// A non-cell element appeared in the table cell vector.
    NonCell { cell: usize },
    /// A cell cannot occupy the first available column range.
    CellDoesNotFit {
        row: usize,
        cell: usize,
        column: u16,
        colspan: u16,
    },
    /// The final explicit cell does not complete its logical row.
    IncompleteRow { row: usize },
    /// Existing row spans cover a whole row, leaving no position for the next explicit cell.
    FullyCoveredRow { row: usize },
    /// One or more row spans continue beyond the final explicit row.
    RowspanBeyondTable,
}

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
    /// Source range responsible for this node. It is transported so returned
    /// subtrees retain diagnostics, but never participates in call semantics.
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

    /// Returns the local name for `core::*` nodes, or `None`.
    pub fn core_local(&self) -> Option<&str> {
        self.name.strip_prefix("core::")
    }

    /// Returns whether this node has the given core local name.
    pub fn is_core(&self, local: &str) -> bool {
        self.core_local() == Some(local)
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

/// Validates the row/column layout of `core::table-cell` nodes.
///
/// Cell spans are read directly from the node arguments.
pub fn table_layout_nodes(
    columns: u16,
    cells: &[Node],
) -> Result<Vec<Vec<TableCellPlacement>>, TableLayoutError> {
    let columns = columns as usize;
    let mut active = vec![0u16; columns];
    let mut rows = Vec::new();
    let mut cell_index = 0usize;

    while cell_index < cells.len() {
        let row_number = rows.len() + 1;
        let mut occupied: Vec<_> = active.iter().map(|remaining| *remaining > 0).collect();
        if occupied.iter().all(|occupied| *occupied) {
            return Err(TableLayoutError::FullyCoveredRow { row: row_number });
        }
        let mut next_active: Vec<_> = active
            .iter()
            .map(|remaining| remaining.saturating_sub(1))
            .collect();
        let mut row = Vec::new();

        while occupied.iter().any(|occupied| !occupied) {
            let Some(cell) = cells.get(cell_index) else {
                return Err(TableLayoutError::IncompleteRow { row: row_number });
            };
            if !cell.is_core("table-cell") {
                return Err(TableLayoutError::NonCell {
                    cell: cell_index + 1,
                });
            }
            let span = |name: &str| match cell.get(name) {
                Some(NodeValue::Int(value)) => u16::try_from(*value).unwrap_or(u16::MAX),
                _ => 1,
            };
            let colspan = span("colspan");
            let rowspan = span("rowspan");
            let column = occupied.iter().position(|occupied| !occupied).unwrap();
            let end = column + colspan as usize;
            if end > columns || occupied[column..end].iter().any(|occupied| *occupied) {
                return Err(TableLayoutError::CellDoesNotFit {
                    row: row_number,
                    cell: cell_index + 1,
                    column: column as u16,
                    colspan,
                });
            }
            occupied[column..end].fill(true);
            if rowspan > 1 {
                for remaining in &mut next_active[column..end] {
                    *remaining = (*remaining).max(rowspan - 1);
                }
            }
            row.push(TableCellPlacement {
                cell_index,
                column: column as u16,
            });
            cell_index += 1;
        }

        rows.push(row);
        active = next_active;
    }

    if active.iter().any(|remaining| *remaining > 0) {
        return Err(TableLayoutError::RowspanBeyondTable);
    }
    Ok(rows)
}

/// Wire helpers over the shared plugin protocol types.
///
/// WIT is the outer component ABI; these helpers define the payloads carried
/// by its opaque byte arguments. Every frame starts with the same wire version
/// byte, followed by the postcard payload.
pub mod wire {
    use super::Node;
    use crate::PluginElementDecl;

    /// Version of the opaque plugin payload format.
    pub const WIRE_VERSION: u8 = 1;

    fn encode_payload<T: serde::Serialize + ?Sized>(
        value: &T,
        kind: &str,
    ) -> Result<Vec<u8>, String> {
        let payload = postcard::to_allocvec(value)
            .map_err(|error| format!("encode {kind} payload: {error}"))?;
        let mut bytes = Vec::with_capacity(1 + payload.len());
        bytes.push(WIRE_VERSION);
        bytes.extend(payload);
        Ok(bytes)
    }

    fn decode_payload<T: serde::de::DeserializeOwned>(
        bytes: &[u8],
        kind: &str,
    ) -> Result<T, String> {
        let Some((&version, payload)) = bytes.split_first() else {
            return Err(format!("decode {kind} payload: missing wire version byte"));
        };
        if version != WIRE_VERSION {
            return Err(format!(
                "decode {kind} payload: unsupported wire version {version}, expected {WIRE_VERSION}"
            ));
        }
        postcard::from_bytes(payload).map_err(|error| format!("decode {kind} payload: {error}"))
    }

    /// Serializes one node tree onto the plugin ABI.
    pub fn encode(value: &Node) -> Result<Vec<u8>, String> {
        encode_payload(value, "node")
    }

    /// Deserializes one node tree from the plugin ABI.
    pub fn decode(bytes: &[u8]) -> Result<Node, String> {
        decode_payload(bytes, "node")
    }

    /// Serializes a forest (reduction responses carry many roots).
    pub fn encode_forest(nodes: &[Node]) -> Result<Vec<u8>, String> {
        encode_payload(nodes, "node forest")
    }

    /// Deserializes a forest from the plugin ABI.
    pub fn decode_forest(bytes: &[u8]) -> Result<Vec<Node>, String> {
        decode_payload(bytes, "node forest")
    }

    /// Serializes the declarations returned by component initialization.
    pub fn encode_declarations(declarations: &[PluginElementDecl]) -> Result<Vec<u8>, String> {
        encode_payload(declarations, "plugin declarations")
    }

    /// Deserializes the declarations returned by component initialization.
    pub fn decode_declarations(bytes: &[u8]) -> Result<Vec<PluginElementDecl>, String> {
        decode_payload(bytes, "plugin declarations")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_shapes_a_call_node() {
        let node = Node::block_call("core::details", TextRange::new(0, 5))
            .arg("open", true)
            .arg("summary", "Shader")
            .child(Node::call("core::text", TextRange::new(6, 9)).arg("text", "hi"));
        assert_eq!(node.name, "core::details");
        assert!(node.block);
        assert_eq!(node.core_local(), Some("details"));
        assert!(node.is_core("details"));
        assert_eq!(node.get("summary"), Some(&NodeValue::from("Shader")));
        assert_eq!(node.children.len(), 1);
        assert!(!node.has_pending_args());
    }

    #[test]
    fn table_layout_nodes_validates_spans() {
        let cell = |colspan: i64, rowspan: i64| {
            Node::block_call("core::table-cell", TextRange::new(0, 0))
                .arg("colspan", colspan)
                .arg("rowspan", rowspan)
        };
        let rows = table_layout_nodes(2, &[cell(1, 1), cell(1, 2), cell(1, 1)]).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].len(), 2);
        assert_eq!(rows[1].len(), 1);

        let overflow = table_layout_nodes(1, &[cell(2, 1)]);
        assert!(matches!(
            overflow,
            Err(TableLayoutError::CellDoesNotFit { .. })
        ));
    }

    #[test]
    fn wire_roundtrips_nodes_and_forests_with_version_byte() {
        let node = Node::block_call("demo::box", TextRange::new(3, 8))
            .arg("label", "hello")
            .child(Node::call("core::text", TextRange::new(9, 14)).arg("text", "body"));
        let encoded = wire::encode(&node).unwrap();
        assert_eq!(encoded[0], wire::WIRE_VERSION);
        assert_eq!(wire::decode(&encoded).unwrap(), node);

        let forest = vec![node, Node::call("core::parbreak", TextRange::new(15, 15))];
        let encoded = wire::encode_forest(&forest).unwrap();
        assert_eq!(encoded[0], wire::WIRE_VERSION);
        assert_eq!(wire::decode_forest(&encoded).unwrap(), forest);
    }

    #[test]
    fn wire_roundtrips_plugin_declarations_with_defaults() {
        let declarations = vec![
            crate::PluginElementDecl::new("echo")
                .block(true)
                .param_default("message", "String", "hello")
                .trailing_content("body")
                .body_mode("flow"),
        ];
        let encoded = wire::encode_declarations(&declarations).unwrap();
        assert_eq!(encoded[0], wire::WIRE_VERSION);
        assert_eq!(wire::decode_declarations(&encoded).unwrap(), declarations);
    }

    #[test]
    fn wire_rejects_unknown_version() {
        let error = wire::decode_forest(&[wire::WIRE_VERSION + 1, 0]).unwrap_err();
        assert!(error.contains("unsupported wire version"), "{error}");
    }

    #[test]
    fn wire_rejects_missing_and_malformed_payloads() {
        let missing = wire::decode_forest(&[]).unwrap_err();
        assert!(missing.contains("missing wire version byte"), "{missing}");

        let malformed = wire::decode_forest(&[wire::WIRE_VERSION, 0xff]).unwrap_err();
        assert!(
            malformed.contains("decode node forest payload"),
            "{malformed}"
        );
    }
}
