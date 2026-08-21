//! The unified Leaf content model.
//!
//! Every evaluated content node is an [`ElementInstance`]: a constructor name,
//! typed fields, recursive body, and block/inline classification.

use std::fmt;

use crate::TextRange;

/// The namespace of an [`ElementName`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ElementNamespace {
    /// The language-owned `core` namespace.
    Core,
    /// A plugin-owned namespace.
    Plugin(String),
}

impl fmt::Display for ElementNamespace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core => formatter.write_str("core"),
            Self::Plugin(package) => formatter.write_str(package),
        }
    }
}

/// A qualified constructor identity such as `core::details` or `shader::canvas`.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ElementName {
    /// The owning namespace.
    pub namespace: ElementNamespace,
    /// The constructor name inside the namespace.
    pub local: String,
}

impl ElementName {
    /// Creates a core constructor name.
    pub fn core(local: impl Into<String>) -> Self {
        Self {
            namespace: ElementNamespace::Core,
            local: local.into(),
        }
    }

    /// Creates a plugin constructor name.
    pub fn plugin(package: impl Into<String>, local: impl Into<String>) -> Self {
        Self {
            namespace: ElementNamespace::Plugin(package.into()),
            local: local.into(),
        }
    }

    /// Parses `namespace::local`. Unknown namespaces are treated as plugins.
    pub fn parse(qualified: &str) -> Self {
        match qualified.split_once("::") {
            Some(("core", local)) => Self::core(local),
            Some((namespace, local)) => Self::plugin(namespace, local),
            None => Self::plugin(qualified, qualified),
        }
    }

    /// Returns the short name for `core::*` constructors, or `None`.
    pub fn core_local(&self) -> Option<&str> {
        match &self.namespace {
            ElementNamespace::Core => Some(&self.local),
            ElementNamespace::Plugin(_) => None,
        }
    }
}

impl fmt::Display for ElementName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.namespace {
            // Legacy bare plugin names round-trip as `name` rather than
            // `name::name`.
            ElementNamespace::Plugin(package) if package == &self.local => {
                formatter.write_str(&self.local)
            }
            _ => write!(formatter, "{}::{}", self.namespace, self.local),
        }
    }
}

/// A typed value stored on an [`ElementInstance`] field.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    /// The absence of a value.
    None,
    /// A boolean value.
    Bool(bool),
    /// A signed integer value.
    Int(i64),
    /// A floating-point value.
    Float(f64),
    /// A UTF-8 string value.
    String(String),
    /// Nested Leaf content.
    Content(Vec<InstanceNode>),
    /// A list of field values.
    Array(Vec<FieldValue>),
}

/// One named, typed constructor argument stored on an [`ElementInstance`].
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// The argument name.
    pub name: String,
    /// The typed argument value.
    pub value: FieldValue,
}

impl Field {
    /// Creates a field.
    pub fn new(name: impl Into<String>, value: FieldValue) -> Self {
        Self {
            name: name.into(),
            value,
        }
    }
}

/// The unified terminal content node.
///
/// `ElementInstance` is the only leaf type in the evaluated content model.
/// Built-in constructors use `core::*` names; plugins use their own namespace.
#[derive(Clone, Debug, PartialEq)]
pub struct ElementInstance {
    /// The constructor identity.
    pub name: ElementName,
    /// Typed constructor arguments.
    pub fields: Vec<Field>,
    /// Recursive Leaf content.
    pub body: Vec<InstanceNode>,
    /// Whether this instance interrupts paragraph flow.
    pub block: bool,
}

impl ElementInstance {
    /// Creates an empty instance.
    pub fn new(name: ElementName, block: bool) -> Self {
        Self {
            name,
            fields: Vec::new(),
            body: Vec::new(),
            block,
        }
    }

    /// Creates a core instance with one string field.
    pub fn text(value: impl Into<String>) -> Self {
        Self::new(ElementName::core("text"), false)
            .with_field("text", FieldValue::String(value.into()))
    }

    /// Creates the transient `core::parbreak` stream leaf.
    pub fn parbreak() -> Self {
        Self::new(ElementName::core("parbreak"), false)
    }

    /// Appends a typed field.
    pub fn with_field(mut self, name: impl Into<String>, value: FieldValue) -> Self {
        self.fields.push(Field::new(name, value));
        self
    }

    /// Appends a child node.
    pub fn with_child(mut self, child: InstanceNode) -> Self {
        self.body.push(child);
        self
    }

    /// Returns the named field.
    pub fn field(&self, name: &str) -> Option<&FieldValue> {
        self.fields.iter().find_map(|field| {
            if field.name == name {
                Some(&field.value)
            } else {
                None
            }
        })
    }

    /// Returns whether this instance has the given core local name.
    pub fn is_core(&self, local: &str) -> bool {
        self.name.core_local() == Some(local)
    }

    /// Returns the `block` classification of a core constructor by name.
    pub fn core_block(local: &str) -> Option<bool> {
        let block = matches!(
            local,
            "heading"
                | "rule"
                | "item"
                | "table-cell"
                | "table"
                | "figure"
                | "callout"
                | "details"
                | "raw" // refined by the `block` field at construction time
                | "paragraph"
                | "list"
                | "section"
        );
        Some(block)
    }
}

/// How a Leaf participates in paragraph/section folding.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ShapingKind {
    /// Joins the current paragraph buffer.
    Inline,
    /// Flushes surrounding inline runs and stands alone.
    Block,
    /// Flushes surrounding inline runs and is consumed without output.
    Separator,
    /// No shaping declaration; fall back to `ElementInstance.block`.
    #[default]
    Unspecified,
}

/// How the body of a Leaf is shaped.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum BodyMode {
    /// Body is already shaped by a previous pass.
    Shaped,
    /// Inline-only body.
    Inline,
    /// Full block flow: paragraph / list / section folding.
    Flow,
    /// Table cells; no paragraph grouping.
    Cells,
    /// No body shaping.
    #[default]
    None,
}

/// A shaping-time grouping role.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ShapingRole {
    /// An ordinary Leaf.
    #[default]
    None,
    /// Starts a `core::section` grouping.
    Heading,
    /// Adjacent items merge into `core::list`.
    Item,
}

/// Declarative shaping metadata for one Element name.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ElementSchema {
    /// The qualified name this schema describes.
    pub name: ElementName,
    /// Paragraph-flow classification.
    pub kind: ShapingKind,
    /// How the body participates in recursive shaping.
    pub body_mode: BodyMode,
    /// Grouping role.
    pub role: ShapingRole,
}

impl ElementSchema {
    /// Creates a schema entry.
    pub fn new(
        name: ElementName,
        kind: ShapingKind,
        body_mode: BodyMode,
        role: ShapingRole,
    ) -> Self {
        Self {
            name,
            kind,
            body_mode,
            role,
        }
    }
}

/// An [`ElementInstance`] paired with its source range.
#[derive(Clone, Debug, PartialEq)]
pub struct InstanceNode {
    /// The content node.
    pub instance: ElementInstance,
    /// The source range responsible for the node.
    pub range: TextRange,
}

impl InstanceNode {
    /// Creates a node with a zero-width synthetic range.
    pub fn synthetic(instance: ElementInstance) -> Self {
        Self {
            instance,
            range: TextRange::new(0, 0),
        }
    }

    /// Creates a node at the given range.
    pub fn ranged(instance: ElementInstance, range: TextRange) -> Self {
        Self { instance, range }
    }
}
