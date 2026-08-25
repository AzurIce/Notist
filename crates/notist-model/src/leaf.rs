//! Element names and shaping schemas.
//!
//! A reduced node is identified by its qualified [`ElementName`]; the shaping
//! stage classifies nodes through declarative [`ElementSchema`] entries. There
//! is no separate terminal-node type: the unified [`crate::Node`] serves both
//! the call and the terminal phase.

use std::fmt;

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

/// How a node participates in paragraph/section folding.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ShapingKind {
    /// Joins the current paragraph buffer.
    Inline,
    /// Flushes surrounding inline runs and stands alone.
    Block,
    /// Flushes surrounding inline runs and is consumed without output.
    Separator,
    /// No shaping declaration; fall back to `Node.block`.
    #[default]
    Unspecified,
}

/// How the children of a node are shaped.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum BodyMode {
    /// Children are already shaped by a previous pass.
    Shaped,
    /// Inline-only children.
    Inline,
    /// Full block flow: paragraph / list / section folding.
    Flow,
    /// Table cells; no paragraph grouping.
    Cells,
    /// No child shaping.
    #[default]
    None,
}

/// A shaping-time grouping role.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ShapingRole {
    /// An ordinary node.
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
    /// How the children participate in recursive shaping.
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
