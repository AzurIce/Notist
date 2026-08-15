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
    /// A paragraph produced by shaping inline runs.
    Paragraph(Content),
    /// Inline content with strong emphasis.
    Strong(Content),
    /// Inline content with ordinary emphasis.
    Emph(Content),
    /// Inline content rendered as no longer applicable.
    Strike(Content),
    /// Inline content rendered with an underline.
    Underline(Content),
    /// A thematic break between document sections.
    Rule,
    /// A section heading.
    Heading {
        /// The one-based heading level.
        level: u8,
        /// The heading body.
        body: Content,
    },
    /// An item that will be grouped with adjacent items during structuring.
    ListItem(Content),
    /// A realized ordered or unordered list container.
    List {
        /// Whether the list uses ordered numbering.
        ordered: bool,
        /// Evaluated item elements in source order.
        items: Vec<ElementNode>,
    },
    /// An ordered-list item that will be grouped with adjacent items during structuring.
    EnumItem {
        /// Optional explicit ordinal value.
        value: Option<u32>,
        /// Item body.
        body: Content,
    },
    /// An emphasized block of advisory content with an author-defined kind.
    Callout {
        /// A short category such as note, tip, or warning.
        kind: String,
        /// Optional visible heading for the callout.
        title: Option<Content>,
        /// The callout body.
        body: Content,
    },
    /// A collapsible block with a summary and body.
    Details {
        /// Summary displayed by the disclosure control.
        summary: Option<Content>,
        /// Whether the block is initially expanded.
        open: bool,
        /// Collapsible body content.
        body: Content,
    },
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
            Self::Text(_)
            | Self::Reference(_)
            | Self::Strong(_)
            | Self::Emph(_)
            | Self::Strike(_)
            | Self::Underline(_) => true,
            Self::Raw { block, .. }
            | Self::Custom { block, .. }
            | Self::UnresolvedCall { block, .. } => !block,
            Self::Parbreak
            | Self::Paragraph(_)
            | Self::Heading { .. }
            | Self::List { .. }
            | Self::ListItem(_)
            | Self::EnumItem { .. }
            | Self::Rule
            | Self::Callout { .. }
            | Self::Details { .. } => false,
        }
    }
}
/// A structured view produced from an evaluated content sequence.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StructuredDocument {
    /// Top-level document blocks.
    pub blocks: Vec<Block>,
}

/// A top-level structure created by grouping evaluated elements.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Block {
    /// A block-level element that stands on its own.
    Element(ElementNode),
    /// A section (D0002 shaping): a heading plus its content up to the next
    /// same-or-higher-level heading, grouped recursively.
    Section {
        level: u8,
        heading: ElementNode,
        body: Vec<Block>,
    },
}

impl Block {
    /// The complete source range covered by this block: the element range, or
    /// for a section the heading start through its last child's end.
    pub fn range(&self) -> TextRange {
        match self {
            Self::Element(node) => node.range,
            Self::Section { heading, body, .. } => {
                let end = body.last().map_or(heading.range.end, |child| child.range().end);
                TextRange::new(heading.range.start, end)
            }
        }
    }
}
