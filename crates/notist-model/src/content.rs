use crate::{TextRange, WikiReference};

/// A sequence of evaluated document elements.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Content {
    /// Elements in their evaluation order.
    pub elements: Vec<ElementNode>,
}

impl Content {
    /// Creates an empty content sequence.
    pub const fn new() -> Self {
        Self {
            elements: Vec::new(),
        }
    }

    /// Creates a content sequence containing one element.
    pub fn single(element: Element, range: TextRange) -> Self {
        Self {
            elements: vec![ElementNode { element, range }],
        }
    }

    /// Appends another content sequence.
    pub fn extend(&mut self, other: Content) {
        self.elements.extend(other.elements);
    }

    /// Returns whether this sequence contains no elements.
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }
}

/// An evaluated element associated with its original source range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ElementNode {
    /// The semantic document element.
    pub element: Element,
    /// The source range responsible for the element.
    pub range: TextRange,
}

/// A semantic element produced by lowering or function evaluation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Element {
    /// Plain text.
    Text(String),
    /// A wiki-style module or label reference.
    Reference(WikiReference),
    /// A paragraph boundary created by one or more blank lines.
    Parbreak,
    /// Inline content with strong emphasis.
    Strong(Content),
    /// A section heading.
    Heading {
        /// The one-based heading level.
        level: u8,
        /// The heading body.
        body: Content,
    },
    /// An item that will be grouped with adjacent items during structuring.
    ListItem(Content),
    /// A block quotation containing evaluated Notist content.
    Quote(Content),
    /// Raw text that may be inline or block-level.
    Raw {
        /// The original raw text.
        text: String,
        /// Whether the raw element interrupts paragraph flow.
        block: bool,
        /// An optional language identifier.
        language: Option<String>,
    },
    /// A plugin-defined element with a stable name and evaluated body.
    Custom {
        /// The globally meaningful element name.
        name: String,
        /// Nested content owned by the custom element.
        body: Content,
        /// Whether the custom element interrupts paragraph flow.
        block: bool,
    },
    /// A call for which no function was registered.
    UnresolvedCall {
        /// The unresolved function name.
        name: String,
        /// The raw argument text, if present.
        arguments: Option<String>,
        /// The optional trailing Content preserved for display and tooling.
        trailing: Option<Content>,
        /// Whether the trailing Content uses block source form.
        block: bool,
    },
}

impl Element {
    /// Returns whether this element participates in paragraph flow.
    pub fn is_inline(&self) -> bool {
        match self {
            Self::Text(_) | Self::Reference(_) | Self::Strong(_) => true,
            Self::Raw { block, .. }
            | Self::Custom { block, .. }
            | Self::UnresolvedCall { block, .. } => !block,
            Self::Parbreak | Self::Heading { .. } | Self::ListItem(_) | Self::Quote(_) => false,
        }
    }
}

/// Normalized metadata attached to a source range or function result.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Metadata {
    /// The optional stable identifier.
    pub id: Option<String>,
    /// Semantic tags in source order.
    pub tags: Vec<String>,
    /// Presentation classes in source order.
    pub classes: Vec<String>,
    /// Structured properties in source order.
    pub properties: Vec<Property>,
}

impl Metadata {
    /// Returns whether no metadata was specified.
    pub fn is_empty(&self) -> bool {
        self.id.is_none()
            && self.tags.is_empty()
            && self.classes.is_empty()
            && self.properties.is_empty()
    }
}

/// A normalized key-value metadata property.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Property {
    /// The property key.
    pub key: String,
    /// The raw property value.
    pub value: String,
}

/// Metadata projected over a source range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Annotation {
    /// The annotated source range.
    pub range: TextRange,
    /// Metadata associated with the range.
    pub metadata: Metadata,
}

/// A structured view produced from an evaluated content sequence.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StructuredDocument {
    /// Top-level document blocks.
    pub blocks: Vec<Block>,
    /// Source-range annotations preserved from scopes and calls.
    pub annotations: Vec<Annotation>,
}

/// A top-level structure created by grouping evaluated elements.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Block {
    /// A run of inline elements grouped into a paragraph.
    Paragraph(Content),
    /// Adjacent list item elements grouped into one list.
    List(Vec<ElementNode>),
    /// A block-level element that stands on its own.
    Element(ElementNode),
}
