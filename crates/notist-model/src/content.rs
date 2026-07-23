use crate::{TextRange, WikiReference};

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
    /// Inline content explicitly grouped as a paragraph.
    Paragraph(Content),
    /// Inline content with strong emphasis.
    Strong(Content),
    /// Inline content with ordinary emphasis.
    Emph(Content),
    /// Inline content rendered as no longer applicable.
    Strike(Content),
    /// Inline content marked as newly inserted.
    Insert(Content),
    /// Inline content visually concealed until the reader reveals it.
    Spoiler(Content),
    /// Inline content marked for visual or semantic highlighting.
    Highlight(Content),
    /// Inline content rendered with an underline.
    Underline(Content),
    /// Inline keyboard input or shortcut notation.
    Keyboard(Content),
    /// Inline sample output from a program or system.
    Sample(Content),
    /// Inline content rendered as superscript.
    Super(Content),
    /// Inline content rendered as subscript.
    Sub(Content),
    /// Inline reference to content collected in the document footnote section.
    Footnote(Content),
    /// Author-only content omitted from published rendering.
    Comment(Content),
    /// Mathematical source text, optionally rendered as a block.
    Math { text: String, block: bool },
    /// An abbreviation paired with its expanded form.
    Abbr {
        /// Visible abbreviated term.
        term: String,
        /// Full expansion exposed to readers and assistive technology.
        expansion: String,
    },
    /// Human-readable date/time content with a machine-readable value.
    Time { datetime: String, body: Content },
    /// A bibliography citation retained for later reference resolution.
    Citation {
        /// Stable bibliography key.
        key: String,
        /// Optional page, section, chapter, or other locator.
        locator: Option<String>,
    },
    /// A hyperlink with an explicit destination and visible body.
    Link {
        /// The link destination exactly as provided by the author.
        destination: String,
        /// Optional advisory title exposed by the renderer.
        title: Option<String>,
        /// Visible inline content.
        body: Content,
    },
    /// An image with a source, alternative text, and optional presentation metadata.
    Image {
        /// The image source URL or path.
        source: String,
        /// Alternative text used when the image is unavailable.
        alt: String,
        /// Optional advisory title exposed by the renderer.
        title: Option<String>,
        /// Optional rendered width in CSS pixels.
        width: Option<u32>,
        /// Optional rendered height in CSS pixels.
        height: Option<u32>,
    },
    /// A block image with a semantic caption.
    Figure {
        /// The image source URL or path.
        source: String,
        /// Alternative text used when the image is unavailable.
        alt: String,
        /// Optional advisory image title.
        title: Option<String>,
        /// Evaluated caption content.
        caption: Content,
    },
    /// A block video with optional poster and playback controls.
    Video {
        source: String,
        poster: Option<String>,
        controls: bool,
    },
    /// A block audio player.
    Audio {
        source: String,
        controls: bool,
        looping: bool,
    },
    /// A forced line break inside paragraph flow.
    Linebreak,
    /// A thematic break between document sections.
    Rule,
    /// A forced page boundary for paged or printed output.
    Pagebreak,
    /// A section heading.
    Heading {
        /// The one-based heading level.
        level: u8,
        /// The heading body.
        body: Content,
    },
    /// A document outline generated from headings in the rendered document.
    Outline {
        /// Maximum heading level included in the outline.
        depth: u8,
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
    /// A definition-list item containing a term and its description.
    TermItem {
        /// The term being defined.
        term: Content,
        /// The definition or description.
        description: Content,
    },
    /// A realized definition-list container.
    Terms {
        /// Definition items in source order.
        items: Vec<ElementNode>,
    },
    /// A task-list item with a completion state.
    TaskItem {
        /// Whether the task is completed.
        checked: bool,
        /// The task description.
        body: Content,
    },
    /// A realized task-list container.
    Tasks {
        /// Task items in source order.
        items: Vec<ElementNode>,
    },
    /// A table cell consumed by a surrounding `table` element.
    TableCell {
        /// Cell contents.
        body: Content,
        /// Number of columns occupied by this cell.
        colspan: u16,
        /// Number of rows occupied by this cell.
        rowspan: u16,
    },
    /// A table with a fixed number of columns and evaluated cells.
    Table {
        /// Number of cells per row.
        columns: u16,
        /// Whether the first row contains column headings.
        header: bool,
        /// Horizontal alignment for each column.
        alignments: Vec<TableAlignment>,
        /// Optional semantic caption displayed with the table.
        caption: Option<Content>,
        /// Cells in row-major order.
        cells: Vec<ElementNode>,
    },
    /// A block quotation containing evaluated Notist content.
    Quote {
        /// Quoted body content.
        body: Content,
        /// Optional source attribution.
        attribution: Option<Content>,
    },
    /// An emphasized block of advisory content with an author-defined kind.
    Callout {
        /// A short category such as `note`, `tip`, or `warning`.
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
            | Self::Insert(_)
            | Self::Spoiler(_)
            | Self::Highlight(_)
            | Self::Underline(_)
            | Self::Keyboard(_)
            | Self::Sample(_)
            | Self::Super(_)
            | Self::Sub(_)
            | Self::Footnote(_)
            | Self::Comment(_)
            | Self::Math { block: false, .. }
            | Self::Abbr { .. }
            | Self::Time { .. }
            | Self::Citation { .. }
            | Self::Link { .. }
            | Self::Image { .. }
            | Self::Linebreak => true,
            Self::Raw { block, .. }
            | Self::Custom { block, .. }
            | Self::UnresolvedCall { block, .. } => !block,
            Self::Parbreak
            | Self::Paragraph(_)
            | Self::Heading { .. }
            | Self::Outline { .. }
            | Self::List { .. }
            | Self::ListItem(_)
            | Self::EnumItem { .. }
            | Self::Terms { .. }
            | Self::TermItem { .. }
            | Self::Tasks { .. }
            | Self::TaskItem { .. }
            | Self::TableCell { .. }
            | Self::Table { .. }
            | Self::Figure { .. }
            | Self::Video { .. }
            | Self::Audio { .. }
            | Self::Rule
            | Self::Pagebreak
            | Self::Quote { .. }
            | Self::Callout { .. }
            | Self::Details { .. }
            | Self::Math { block: true, .. } => false,
        }
    }
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

/// Places table cells into logical rows while respecting both column and row spans.
pub fn table_layout(
    columns: u16,
    cells: &[ElementNode],
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
            let Element::TableCell {
                colspan, rowspan, ..
            } = &cell.element
            else {
                return Err(TableLayoutError::NonCell {
                    cell: cell_index + 1,
                });
            };
            let column = occupied.iter().position(|occupied| !occupied).unwrap();
            let end = column + *colspan as usize;
            if end > columns || occupied[column..end].iter().any(|occupied| *occupied) {
                return Err(TableLayoutError::CellDoesNotFit {
                    row: row_number,
                    cell: cell_index + 1,
                    column: column as u16,
                    colspan: *colspan,
                });
            }
            occupied[column..end].fill(true);
            if *rowspan > 1 {
                for remaining in &mut next_active[column..end] {
                    *remaining = (*remaining).max(*rowspan - 1);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(colspan: u16, rowspan: u16) -> ElementNode {
        ElementNode {
            element: Element::TableCell {
                body: Content::new(),
                colspan,
                rowspan,
            },
            range: TextRange::new(0, 0),
        }
    }

    #[test]
    fn table_layout_places_cells_around_rowspans() {
        let cells = [cell(2, 1), cell(1, 2), cell(1, 1), cell(1, 1)];

        assert_eq!(
            table_layout(3, &cells),
            Ok(vec![
                vec![
                    TableCellPlacement {
                        cell_index: 0,
                        column: 0,
                    },
                    TableCellPlacement {
                        cell_index: 1,
                        column: 2,
                    },
                ],
                vec![
                    TableCellPlacement {
                        cell_index: 2,
                        column: 0,
                    },
                    TableCellPlacement {
                        cell_index: 3,
                        column: 1,
                    },
                ],
            ])
        );
    }

    #[test]
    fn table_layout_rejects_overlap_with_active_rowspan() {
        let cells = [cell(2, 1), cell(1, 2), cell(1, 1), cell(2, 1)];

        assert_eq!(
            table_layout(3, &cells),
            Err(TableLayoutError::CellDoesNotFit {
                row: 2,
                cell: 4,
                column: 1,
                colspan: 2,
            })
        );
    }

    #[test]
    fn table_layout_rejects_incomplete_and_fully_covered_rows() {
        assert_eq!(
            table_layout(2, &[cell(1, 1)]),
            Err(TableLayoutError::IncompleteRow { row: 1 })
        );
        assert_eq!(
            table_layout(1, &[cell(1, 2), cell(1, 1)]),
            Err(TableLayoutError::FullyCoveredRow { row: 2 })
        );
    }

    #[test]
    fn table_layout_rejects_rowspan_beyond_final_row() {
        assert_eq!(
            table_layout(2, &[cell(1, 2), cell(1, 1)]),
            Err(TableLayoutError::RowspanBeyondTable)
        );
    }

    #[test]
    fn table_layout_rejects_non_cell_elements() {
        let nodes = [ElementNode {
            element: Element::Text("not a cell".into()),
            range: TextRange::new(0, 0),
        }];

        assert_eq!(
            table_layout(1, &nodes),
            Err(TableLayoutError::NonCell { cell: 1 })
        );
    }
}
