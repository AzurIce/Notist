//! Semantic HTML rendering for structured Notist documents.

use std::collections::HashSet;
use std::fmt::Write;

use notist_model::{
    Block, Content, Element, ElementNode, ModulePath, ModuleReference, StructuredDocument,
    TableAlignment, TableCellPlacement, TextRange, WikiReference, table_layout,
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

/// Resolves an absolute module target and optional label to an HTML URL.
pub type ReferenceResolver<'a> = dyn Fn(&ModulePath, Option<&str>) -> Option<String> + 'a;

/// Supplies source-annotation IDs for semantic elements that fall within an annotated scope.
pub type SourceIdResolver<'a> = dyn Fn(TextRange) -> Option<String> + 'a;

/// Options that control links produced by the HTML renderer.
#[derive(Clone, Copy, Debug)]
pub struct RenderOptions<'a> {
    /// The module containing the rendered document.
    ///
    /// Relative references are only made clickable when this is available.
    pub current_module: Option<&'a ModulePath>,
    /// The URL prefix placed before the percent-encoded target module path.
    pub module_url_prefix: &'a str,
}

impl Default for RenderOptions<'_> {
    fn default() -> Self {
        Self {
            current_module: None,
            module_url_prefix: "?module=",
        }
    }
}

/// Renders a structured document using the default options.
pub fn render(document: &StructuredDocument) -> String {
    render_with_options(document, &RenderOptions::default())
}

/// Renders a structured document as an HTML fragment.
pub fn render_with_options(document: &StructuredDocument, options: &RenderOptions<'_>) -> String {
    render_internal(document, options, None)
}

/// Renders a document using a caller-provided module reference URL resolver.
///
/// Returning `None` leaves the reference visible but unclickable.
pub fn render_with_reference_resolver(
    document: &StructuredDocument,
    options: &RenderOptions<'_>,
    resolver: &ReferenceResolver<'_>,
) -> String {
    render_internal(document, options, Some(resolver))
}

/// Renders a document with both module-reference URLs and source-annotation IDs resolved by the
/// caller.
pub fn render_with_resolvers(
    document: &StructuredDocument,
    options: &RenderOptions<'_>,
    reference_resolver: &ReferenceResolver<'_>,
    source_id_resolver: &SourceIdResolver<'_>,
) -> String {
    render_internal_with_source_ids(
        document,
        options,
        Some(reference_resolver),
        Some(source_id_resolver),
    )
}

fn render_internal<'a>(
    document: &StructuredDocument,
    options: &'a RenderOptions<'a>,
    resolver: Option<&'a ReferenceResolver<'a>>,
) -> String {
    render_internal_with_source_ids(document, options, resolver, None)
}

fn render_internal_with_source_ids<'a>(
    document: &StructuredDocument,
    options: &'a RenderOptions<'a>,
    resolver: Option<&'a ReferenceResolver<'a>>,
    source_id_resolver: Option<&'a SourceIdResolver<'a>>,
) -> String {
    let outline_entries = collect_outline_entries(document, source_id_resolver);
    let mut renderer = Renderer {
        output: String::new(),
        options,
        reference_resolver: resolver,
        source_id_resolver,
        emitted_source_ids: HashSet::new(),
        outline_entries,
        footnotes: Vec::new(),
    };
    renderer.document(document);
    renderer.output
}

struct Renderer<'a, 'options> {
    output: String,
    options: &'options RenderOptions<'a>,
    reference_resolver: Option<&'options ReferenceResolver<'options>>,
    source_id_resolver: Option<&'options SourceIdResolver<'options>>,
    emitted_source_ids: HashSet<String>,
    outline_entries: Vec<OutlineEntry>,
    footnotes: Vec<Content>,
}

impl Renderer<'_, '_> {
    fn document(&mut self, document: &StructuredDocument) {
        for block in &document.blocks {
            self.block(block);
        }
        if !self.footnotes.is_empty() {
            self.output
                .push_str("<section class=\"notist-footnotes\" aria-label=\"Footnotes\"><ol>");
            let mut index = 0;
            while index < self.footnotes.len() {
                let number = index + 1;
                let body = self.footnotes[index].clone();
                write!(self.output, "<li id=\"notist-footnote-{number}\">").unwrap();
                self.flow_content(&body);
                write!(
                    self.output,
                    "<a class=\"notist-footnote-backref\" href=\"#notist-footnote-ref-{number}\" aria-label=\"Back to reference {number}\">&#8617;</a></li>"
                )
                .unwrap();
                index += 1;
            }
            self.output.push_str("</ol></section>");
        }
    }

    fn block(&mut self, block: &Block) {
        match block {
            Block::Element(node) => self.element(&node.element, node, RenderPosition::Block),
        }
    }

    fn inline_content(&mut self, content: &Content) {
        for node in &content.elements {
            self.element(&node.element, node, RenderPosition::Inline);
        }
    }

    fn flow_content(&mut self, content: &Content) {
        let mut paragraph_open = false;
        let mut index = 0;

        while index < content.elements.len() {
            let node = &content.elements[index];
            if node.element.is_inline() {
                if !paragraph_open {
                    self.output.push_str("<p>");
                    paragraph_open = true;
                }
                self.element(&node.element, node, RenderPosition::Inline);
                index += 1;
                continue;
            }

            if paragraph_open {
                self.output.push_str("</p>");
                paragraph_open = false;
            }

            match &node.element {
                Element::Parbreak => index += 1,
                Element::ListItem(_) => {
                    self.output.push_str("<ul>");
                    while index < content.elements.len()
                        && matches!(content.elements[index].element, Element::ListItem(_))
                    {
                        self.list_item(&content.elements[index]);
                        index += 1;
                    }
                    self.output.push_str("</ul>");
                }
                Element::EnumItem { .. } => {
                    self.ordered_list_open(Some(node));
                    while index < content.elements.len()
                        && matches!(content.elements[index].element, Element::EnumItem { .. })
                    {
                        self.list_item(&content.elements[index]);
                        index += 1;
                    }
                    self.output.push_str("</ol>");
                }
                Element::TermItem { .. } => {
                    self.output.push_str("<dl>");
                    while index < content.elements.len()
                        && matches!(content.elements[index].element, Element::TermItem { .. })
                    {
                        self.term_item(&content.elements[index]);
                        index += 1;
                    }
                    self.output.push_str("</dl>");
                }
                Element::TaskItem { .. } => {
                    self.output.push_str("<ul class=\"notist-task-list\">");
                    while index < content.elements.len()
                        && matches!(content.elements[index].element, Element::TaskItem { .. })
                    {
                        self.task_item(&content.elements[index]);
                        index += 1;
                    }
                    self.output.push_str("</ul>");
                }
                element => {
                    self.element(element, node, RenderPosition::Block);
                    index += 1;
                }
            }
        }

        if paragraph_open {
            self.output.push_str("</p>");
        }
    }

    fn ordered_list_open(&mut self, first: Option<&ElementNode>) {
        self.output.push_str("<ol");
        if let Some(ElementNode {
            element: Element::EnumItem {
                value: Some(value), ..
            },
            ..
        }) = first
        {
            write!(self.output, " start=\"{value}\"").unwrap();
        }
        self.output.push('>');
    }

    fn list_item(&mut self, node: &ElementNode) {
        self.output.push_str("<li");
        if let Element::EnumItem {
            value: Some(value), ..
        } = &node.element
        {
            write!(self.output, " value=\"{value}\"").unwrap();
        }
        self.range_attributes(node);
        self.output.push('>');
        match &node.element {
            Element::ListItem(body) | Element::EnumItem { body, .. } => self.flow_content(body),
            element => self.element(element, node, RenderPosition::Block),
        }
        self.output.push_str("</li>");
    }

    fn term_item(&mut self, node: &ElementNode) {
        let Element::TermItem { term, description } = &node.element else {
            return;
        };
        self.output.push_str("<dt");
        self.range_attributes(node);
        self.output.push('>');
        self.inline_content(term);
        self.output.push_str("</dt><dd>");
        self.flow_content(description);
        self.output.push_str("</dd>");
    }

    fn task_item(&mut self, node: &ElementNode) {
        let Element::TaskItem { checked, body } = &node.element else {
            return;
        };
        self.output.push_str("<li class=\"notist-task-item\"");
        self.range_attributes(node);
        self.output.push_str("><input type=\"checkbox\" disabled");
        if *checked {
            self.output.push_str(" checked");
        }
        self.output.push_str(" aria-label=\"");
        self.output
            .push_str(if *checked { "Completed" } else { "Incomplete" });
        self.output.push_str(" task\">");
        self.flow_content(body);
        self.output.push_str("</li>");
    }

    fn table_row(
        &mut self,
        cells: &[ElementNode],
        placements: &[TableCellPlacement],
        tag: &str,
        alignments: &[TableAlignment],
    ) {
        self.output.push_str("<tr>");
        for placement in placements {
            let Some(cell) = cells.get(placement.cell_index) else {
                continue;
            };
            write!(self.output, "<{tag}").unwrap();
            let (body, colspan, rowspan) = match &cell.element {
                Element::TableCell {
                    body,
                    colspan,
                    rowspan,
                } => (body, *colspan, *rowspan),
                _ => continue,
            };
            if let Some(class) = alignments
                .get(placement.column as usize)
                .and_then(|alignment| table_alignment_class(*alignment))
            {
                write!(self.output, " class=\"{class}\"").unwrap();
            }
            if colspan > 1 {
                write!(self.output, " colspan=\"{colspan}\"").unwrap();
            }
            if rowspan > 1 {
                write!(self.output, " rowspan=\"{rowspan}\"").unwrap();
            }
            self.range_attributes(cell);
            self.output.push('>');
            self.flow_content(body);
            write!(self.output, "</{tag}>").unwrap();
        }
        self.output.push_str("</tr>");
    }

    fn element(&mut self, element: &Element, node: &ElementNode, position: RenderPosition) {
        match element {
            Element::Text(text) => {
                self.output.push_str("<span class=\"notist-text\"");
                self.range_attributes(node);
                self.output.push('>');
                escape_text(&mut self.output, text);
                self.output.push_str("</span>");
            }
            Element::Reference(reference) => self.reference(reference, node),
            Element::Parbreak => self.output.push_str("<br><br>"),
            Element::Paragraph(body) => {
                self.output.push_str("<p");
                self.range_attributes(node);
                self.output.push('>');
                self.inline_content(body);
                self.output.push_str("</p>");
            }
            Element::Strong(body) => {
                self.output.push_str("<strong");
                self.range_attributes(node);
                self.output.push('>');
                self.inline_content(body);
                self.output.push_str("</strong>");
            }
            Element::Emph(body) => {
                self.output.push_str("<em");
                self.range_attributes(node);
                self.output.push('>');
                self.inline_content(body);
                self.output.push_str("</em>");
            }
            Element::Strike(body) => {
                self.output.push_str("<s");
                self.range_attributes(node);
                self.output.push('>');
                self.inline_content(body);
                self.output.push_str("</s>");
            }
            Element::Insert(body) => {
                self.output.push_str("<ins");
                self.range_attributes(node);
                self.output.push('>');
                self.inline_content(body);
                self.output.push_str("</ins>");
            }
            Element::Spoiler(body) => {
                self.output.push_str(
                    "<span class=\"notist-spoiler\" tabindex=\"0\" title=\"Focus or hover to reveal\"",
                );
                self.range_attributes(node);
                self.output.push('>');
                self.inline_content(body);
                self.output.push_str("</span>");
            }
            Element::Highlight(body) => {
                self.output.push_str("<mark");
                self.range_attributes(node);
                self.output.push('>');
                self.inline_content(body);
                self.output.push_str("</mark>");
            }
            Element::Underline(body) => {
                self.output.push_str("<u");
                self.range_attributes(node);
                self.output.push('>');
                self.inline_content(body);
                self.output.push_str("</u>");
            }
            Element::Keyboard(body) => {
                self.output.push_str("<kbd class=\"notist-keyboard\"");
                self.range_attributes(node);
                self.output.push('>');
                self.inline_content(body);
                self.output.push_str("</kbd>");
            }
            Element::Sample(body) => {
                self.output.push_str("<samp class=\"notist-sample\"");
                self.range_attributes(node);
                self.output.push('>');
                self.inline_content(body);
                self.output.push_str("</samp>");
            }
            Element::Super(body) => {
                self.output.push_str("<sup");
                self.range_attributes(node);
                self.output.push('>');
                self.inline_content(body);
                self.output.push_str("</sup>");
            }
            Element::Sub(body) => {
                self.output.push_str("<sub");
                self.range_attributes(node);
                self.output.push('>');
                self.inline_content(body);
                self.output.push_str("</sub>");
            }
            Element::Footnote(body) => {
                let number = self.footnotes.len() + 1;
                self.footnotes.push(body.clone());
                write!(
                    self.output,
                    "<sup class=\"notist-footnote-ref\" id=\"notist-footnote-ref-{number}\"><a href=\"#notist-footnote-{number}\" aria-label=\"Footnote {number}\">{number}</a></sup>"
                )
                .unwrap();
            }
            Element::Comment(_) => {}
            Element::Math { text, block } => {
                let tag = if *block { "div" } else { "span" };
                write!(self.output, "<{tag} class=\"notist-math\"").unwrap();
                self.range_attributes(node);
                self.output.push('>');
                escape_text(&mut self.output, text);
                write!(self.output, "</{tag}>").unwrap();
            }
            Element::Abbr { term, expansion } => {
                self.output.push_str("<abbr title=\"");
                escape_attribute(&mut self.output, expansion);
                self.output.push('"');
                self.range_attributes(node);
                self.output.push('>');
                escape_text(&mut self.output, term);
                self.output.push_str("</abbr>");
            }
            Element::Time { datetime, body } => {
                self.output.push_str("<time datetime=\"");
                escape_attribute(&mut self.output, datetime);
                self.output.push('"');
                self.range_attributes(node);
                self.output.push('>');
                self.inline_content(body);
                self.output.push_str("</time>");
            }
            Element::Citation { key, locator } => {
                self.output
                    .push_str("<cite class=\"notist-citation\" data-notist-key=\"");
                escape_attribute(&mut self.output, key);
                self.output.push('"');
                self.range_attributes(node);
                self.output.push_str(">[");
                escape_text(&mut self.output, key);
                if let Some(locator) = locator {
                    self.output.push_str(", ");
                    escape_text(&mut self.output, locator);
                }
                self.output.push_str("]</cite>");
            }
            Element::Link {
                destination,
                title,
                body,
            } => {
                let safe_destination = safe_url(destination);
                self.output.push_str(if safe_destination.is_some() {
                    "<a class=\"notist-link\" href=\""
                } else {
                    "<a class=\"notist-link notist-url-unsafe\" aria-disabled=\"true\""
                });
                if let Some(destination) = safe_destination {
                    escape_attribute(&mut self.output, destination);
                    self.output.push('"');
                }
                if let Some(title) = title {
                    self.output.push_str(" title=\"");
                    escape_attribute(&mut self.output, title);
                    self.output.push('"');
                }
                self.range_attributes(node);
                self.output.push('>');
                self.inline_content(body);
                self.output.push_str("</a>");
            }
            Element::Image {
                source,
                alt,
                title,
                width,
                height,
            } => {
                self.output.push_str("<img class=\"notist-image");
                if safe_url(source).is_none() {
                    self.output.push_str(" notist-url-unsafe");
                }
                self.output.push_str("\" src=\"");
                if let Some(source) = safe_url(source) {
                    escape_attribute(&mut self.output, source);
                }
                self.output.push_str("\" alt=\"");
                escape_attribute(&mut self.output, alt);
                self.output.push('"');
                if let Some(title) = title {
                    self.output.push_str(" title=\"");
                    escape_attribute(&mut self.output, title);
                    self.output.push('"');
                }
                if let Some(width) = width {
                    write!(self.output, " width=\"{width}\"").unwrap();
                }
                if let Some(height) = height {
                    write!(self.output, " height=\"{height}\"").unwrap();
                }
                self.output.push_str(" loading=\"lazy\"");
                self.range_attributes(node);
                self.output.push('>');
            }
            Element::Figure {
                source,
                alt,
                title,
                caption,
            } => {
                self.output.push_str("<figure class=\"notist-figure\"");
                self.range_attributes(node);
                self.output.push_str("><img class=\"notist-image\" src=\"");
                if let Some(source) = safe_url(source) {
                    escape_attribute(&mut self.output, source);
                }
                self.output.push_str("\" alt=\"");
                escape_attribute(&mut self.output, alt);
                self.output.push('"');
                if let Some(title) = title {
                    self.output.push_str(" title=\"");
                    escape_attribute(&mut self.output, title);
                    self.output.push('"');
                }
                self.output.push_str(" loading=\"lazy\"><figcaption>");
                self.flow_content(caption);
                self.output.push_str("</figcaption></figure>");
            }
            Element::Video {
                source,
                poster,
                controls,
            } => {
                self.output.push_str("<video class=\"notist-video\" src=\"");
                if let Some(source) = safe_url(source) {
                    escape_attribute(&mut self.output, source);
                }
                self.output.push('"');
                if let Some(poster) = poster.as_deref().and_then(safe_url) {
                    self.output.push_str(" poster=\"");
                    escape_attribute(&mut self.output, poster);
                    self.output.push('"');
                }
                if *controls {
                    self.output.push_str(" controls");
                }
                self.range_attributes(node);
                self.output.push_str("></video>");
            }
            Element::Audio {
                source,
                controls,
                looping,
            } => {
                self.output.push_str("<audio class=\"notist-audio\" src=\"");
                if let Some(source) = safe_url(source) {
                    escape_attribute(&mut self.output, source);
                }
                self.output.push('"');
                if *controls {
                    self.output.push_str(" controls");
                }
                if *looping {
                    self.output.push_str(" loop");
                }
                self.range_attributes(node);
                self.output.push_str("></audio>");
            }
            Element::Linebreak => self.output.push_str("<br>"),
            Element::Rule => {
                self.output.push_str("<hr class=\"notist-rule\"");
                self.range_attributes(node);
                self.output.push('>');
            }
            Element::Pagebreak => {
                self.output.push_str("<hr class=\"notist-pagebreak\"");
                self.range_attributes(node);
                self.output.push('>');
            }
            Element::Heading { level, body } => {
                let level = (*level).clamp(1, 6);
                let id = self
                    .claim_source_id(node)
                    .unwrap_or_else(|| automatic_heading_id(node.range));
                write!(self.output, "<h{level} id=\"").unwrap();
                escape_attribute(&mut self.output, &id);
                self.output.push('"');
                self.range_data_attributes(node);
                self.output.push('>');
                self.inline_content(body);
                write!(self.output, "</h{level}>").unwrap();
            }
            Element::Outline { depth } => self.outline(*depth),
            Element::List { ordered, items } => {
                if *ordered {
                    self.output.push_str("<ol");
                    if let Some(ElementNode {
                        element:
                            Element::EnumItem {
                                value: Some(value), ..
                            },
                        ..
                    }) = items.first()
                    {
                        write!(self.output, " start=\"{value}\"").unwrap();
                    }
                } else {
                    self.output.push_str("<ul");
                }
                self.range_attributes(node);
                self.output.push('>');
                for item in items {
                    self.list_item(item);
                }
                self.output
                    .push_str(if *ordered { "</ol>" } else { "</ul>" });
            }
            Element::ListItem(body) => {
                self.output.push_str("<ul><li");
                self.range_attributes(node);
                self.output.push('>');
                self.flow_content(body);
                self.output.push_str("</li></ul>");
            }
            Element::EnumItem { value, body } => {
                self.output.push_str("<ol");
                if let Some(value) = value {
                    write!(self.output, " start=\"{value}\"").unwrap();
                }
                self.output.push_str("><li");
                if let Some(value) = value {
                    write!(self.output, " value=\"{value}\"").unwrap();
                }
                self.range_attributes(node);
                self.output.push('>');
                self.flow_content(body);
                self.output.push_str("</li></ol>");
            }
            Element::TermItem { .. } => {
                self.output.push_str("<dl>");
                self.term_item(node);
                self.output.push_str("</dl>");
            }
            Element::Terms { items } => {
                self.output.push_str("<dl");
                self.range_attributes(node);
                self.output.push('>');
                for item in items {
                    self.term_item(item);
                }
                self.output.push_str("</dl>");
            }
            Element::TaskItem { .. } => {
                self.output.push_str("<ul class=\"notist-task-list\">");
                self.task_item(node);
                self.output.push_str("</ul>");
            }
            Element::Tasks { items } => {
                self.output.push_str("<ul class=\"notist-task-list\"");
                self.range_attributes(node);
                self.output.push('>');
                for item in items {
                    self.task_item(item);
                }
                self.output.push_str("</ul>");
            }
            Element::Table {
                columns,
                header,
                alignments,
                caption,
                cells,
            } => {
                self.output.push_str("<div class=\"notist-table-wrapper\">");
                write!(self.output, "<table data-notist-columns=\"{columns}\"").unwrap();
                self.range_attributes(node);
                self.output.push('>');
                if let Some(caption) = caption {
                    self.output.push_str("<caption>");
                    self.flow_content(caption);
                    self.output.push_str("</caption>");
                }
                let rows = table_layout(*columns, cells).unwrap_or_else(|_| {
                    vec![
                        cells
                            .iter()
                            .enumerate()
                            .map(|(cell_index, _)| TableCellPlacement {
                                cell_index,
                                column: cell_index.min(u16::MAX as usize) as u16,
                            })
                            .collect(),
                    ]
                });
                if *header {
                    self.output.push_str("<thead>");
                    if let Some(row) = rows.first() {
                        self.table_row(cells, row, "th", alignments);
                    }
                    self.output.push_str("</thead>");
                }
                let body_rows = if *header {
                    rows.iter().skip(1).collect::<Vec<_>>()
                } else {
                    rows.iter().collect::<Vec<_>>()
                };
                if !body_rows.is_empty() {
                    self.output.push_str("<tbody>");
                    for row in body_rows {
                        self.table_row(cells, row, "td", alignments);
                    }
                    self.output.push_str("</tbody>");
                }
                self.output.push_str("</table></div>");
            }
            Element::TableCell { body, .. } => {
                self.output.push_str("<div class=\"notist-table-cell\">");
                self.flow_content(body);
                self.output.push_str("</div>");
            }
            Element::Quote { body, attribution } => {
                self.output.push_str("<blockquote");
                self.range_attributes(node);
                self.output.push('>');
                self.flow_content(body);
                if let Some(attribution) = attribution {
                    self.output.push_str("<footer><cite>");
                    self.inline_content(attribution);
                    self.output.push_str("</cite></footer>");
                }
                self.output.push_str("</blockquote>");
            }
            Element::Callout { kind, title, body } => {
                self.output
                    .push_str("<aside class=\"notist-callout\" data-notist-kind=\"");
                escape_attribute(&mut self.output, kind);
                self.output.push('"');
                self.range_attributes(node);
                self.output.push('>');
                if let Some(title) = title {
                    self.output.push_str("<div class=\"notist-callout-title\">");
                    self.inline_content(title);
                    self.output.push_str("</div>");
                }
                self.flow_content(body);
                self.output.push_str("</aside>");
            }
            Element::Details {
                summary,
                open,
                body,
            } => {
                self.output.push_str("<details class=\"notist-details\"");
                if *open {
                    self.output.push_str(" open");
                }
                self.range_attributes(node);
                self.output.push_str("><summary>");
                if let Some(summary) = summary {
                    self.inline_content(summary);
                } else {
                    escape_text(&mut self.output, "Details");
                }
                self.output.push_str("</summary>");
                self.flow_content(body);
                self.output.push_str("</details>");
            }
            Element::Raw {
                text,
                block,
                language,
            } => self.raw(text, *block, language.as_deref(), node),
            Element::Custom { name, body, block } => {
                let tag = container_tag(*block, position);
                write!(
                    self.output,
                    "<{tag} class=\"notist-custom\" data-notist-name=\""
                )
                .unwrap();
                escape_attribute(&mut self.output, name);
                self.output.push('"');
                self.range_attributes(node);
                self.output.push('>');
                if tag == "div" {
                    self.flow_content(body);
                } else {
                    self.inline_content(body);
                }
                write!(self.output, "</{tag}>").unwrap();
            }
            Element::UnresolvedCall {
                name,
                arguments,
                trailing,
                block,
            } => self.unresolved_call(
                name,
                arguments.as_deref(),
                trailing.as_ref(),
                *block,
                node,
                position,
            ),
        }
    }

    fn reference(&mut self, reference: &WikiReference, node: &ElementNode) {
        let target = match &reference.module {
            ModuleReference::Absolute(_) => reference.module.resolve_from(&ModulePath::root()),
            _ => self
                .options
                .current_module
                .and_then(|current| reference.module.resolve_from(current)),
        };

        let href = target.as_ref().and_then(|target| {
            self.reference_resolver.map_or_else(
                || Some(self.default_reference_href(target, reference.label.as_deref())),
                |resolver| resolver(target, reference.label.as_deref()),
            )
        });

        if let Some(href) = href.as_deref() {
            self.output
                .push_str("<a class=\"notist-reference\" href=\"");
            escape_attribute(&mut self.output, href);
            self.output.push('"');
        } else {
            self.output
                .push_str("<span class=\"notist-reference notist-reference-unresolved\"");
        }

        self.range_attributes(node);
        self.output.push('>');
        self.reference_text(reference);

        if href.is_some() {
            self.output.push_str("</a>");
        } else {
            self.output.push_str("</span>");
        }
    }

    fn default_reference_href(&self, target: &ModulePath, label: Option<&str>) -> String {
        let mut href = self.options.module_url_prefix.to_owned();
        href.extend(utf8_percent_encode(&target.to_string(), NON_ALPHANUMERIC));
        if let Some(label) = label {
            href.push('#');
            href.extend(utf8_percent_encode(label, NON_ALPHANUMERIC));
        }
        href
    }

    fn reference_text(&mut self, reference: &WikiReference) {
        let mut text = String::new();
        match &reference.module {
            ModuleReference::Absolute(segments) => {
                text.push_str("vault");
                for segment in segments {
                    text.push_str("::");
                    text.push_str(segment);
                }
            }
            ModuleReference::Relative(segments) => {
                if segments.is_empty() {
                    text.push_str("self");
                } else {
                    text.push_str(&segments.join("::"));
                }
            }
            ModuleReference::Parent { levels, remainder } => {
                for index in 0..*levels {
                    if index > 0 {
                        text.push_str("::");
                    }
                    text.push_str("super");
                }
                for segment in remainder {
                    if !text.is_empty() {
                        text.push_str("::");
                    }
                    text.push_str(segment);
                }
            }
        }
        if let Some(label) = &reference.label {
            text.push('#');
            text.push_str(label);
        }
        escape_text(&mut self.output, &text);
    }

    fn raw(&mut self, text: &str, block: bool, language: Option<&str>, node: &ElementNode) {
        if block {
            self.output.push_str("<pre");
            self.range_attributes(node);
            self.output.push_str("><code");
        } else {
            self.output.push_str("<code");
            self.range_attributes(node);
        }
        if let Some(language) = language {
            self.output.push_str(" class=\"language-");
            escape_attribute(&mut self.output, language);
            self.output.push('"');
        }
        self.output.push('>');
        escape_text(&mut self.output, text);
        if block {
            self.output.push_str("</code></pre>");
        } else {
            self.output.push_str("</code>");
        }
    }

    fn unresolved_call(
        &mut self,
        name: &str,
        arguments: Option<&str>,
        trailing: Option<&Content>,
        block: bool,
        node: &ElementNode,
        position: RenderPosition,
    ) {
        let tag = container_tag(block, position);
        write!(
            self.output,
            "<{tag} class=\"notist-unresolved-call\" data-notist-name=\""
        )
        .unwrap();
        escape_attribute(&mut self.output, name);
        self.output.push('"');
        if let Some(arguments) = arguments {
            self.output.push_str(" data-notist-arguments=\"");
            escape_attribute(&mut self.output, arguments);
            self.output.push('"');
        }
        self.range_attributes(node);
        self.output.push('>');
        if let Some(body) = trailing {
            if tag == "div" {
                self.flow_content(body);
            } else {
                self.inline_content(body);
            }
        }
        write!(self.output, "</{tag}>").unwrap();
    }

    fn range_attributes(&mut self, node: &ElementNode) {
        if let Some(id) = self.claim_source_id(node) {
            self.output.push_str(" id=\"");
            escape_attribute(&mut self.output, &id);
            self.output.push('"');
        }
        self.range_data_attributes(node);
    }

    fn claim_source_id(&mut self, node: &ElementNode) -> Option<String> {
        self.source_id_resolver
            .and_then(|resolver| resolver(node.range))
            .filter(|id| self.emitted_source_ids.insert(id.clone()))
    }

    fn range_data_attributes(&mut self, node: &ElementNode) {
        write!(
            self.output,
            " data-notist-start=\"{}\" data-notist-end=\"{}\"",
            node.range.start, node.range.end
        )
        .unwrap();
    }

    fn outline(&mut self, depth: u8) {
        self.output
            .push_str("<nav class=\"notist-outline\" aria-label=\"Table of contents\"><ol>");
        for entry in self.outline_entries.clone() {
            if entry.level <= depth {
                self.output.push_str("<li class=\"notist-outline-level-");
                write!(self.output, "{}\"><a href=\"#", entry.level).unwrap();
                escape_attribute(&mut self.output, &entry.id);
                self.output.push_str("\">");
                escape_text(&mut self.output, &entry.text);
                self.output.push_str("</a></li>");
            }
        }
        self.output.push_str("</ol></nav>");
    }
}

#[derive(Clone)]
struct OutlineEntry {
    level: u8,
    id: String,
    text: String,
}

fn collect_outline_entries(
    document: &StructuredDocument,
    source_id_resolver: Option<&SourceIdResolver<'_>>,
) -> Vec<OutlineEntry> {
    document
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Element(node) => match &node.element {
                Element::Heading { level, body } => Some(OutlineEntry {
                    level: *level,
                    id: source_id_resolver
                        .and_then(|resolver| resolver(node.range))
                        .unwrap_or_else(|| automatic_heading_id(node.range)),
                    text: content_plain_text(body),
                }),
                _ => None,
            },
        })
        .collect()
}

fn automatic_heading_id(range: TextRange) -> String {
    format!("notist-heading-{}", range.start)
}

fn content_plain_text(content: &Content) -> String {
    content
        .elements
        .iter()
        .map(|node| match &node.element {
            Element::Text(text) => text.clone(),
            Element::Strong(body)
            | Element::Emph(body)
            | Element::Strike(body)
            | Element::Insert(body)
            | Element::Spoiler(body)
            | Element::Highlight(body)
            | Element::Underline(body)
            | Element::Keyboard(body)
            | Element::Sample(body)
            | Element::Super(body)
            | Element::Sub(body) => content_plain_text(body),
            Element::Link { body, .. } => content_plain_text(body),
            Element::Raw { text, .. } | Element::Math { text, .. } => text.clone(),
            Element::Abbr { term, .. } => term.clone(),
            Element::Time { body, .. } => content_plain_text(body),
            Element::Citation { key, .. } => key.clone(),
            _ => String::new(),
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RenderPosition {
    Inline,
    Block,
}

fn container_tag(block: bool, position: RenderPosition) -> &'static str {
    if block || position == RenderPosition::Block {
        "div"
    } else {
        "span"
    }
}

fn table_alignment_class(alignment: TableAlignment) -> Option<&'static str> {
    match alignment {
        TableAlignment::Default => None,
        TableAlignment::Left => Some("notist-table-align-left"),
        TableAlignment::Center => Some("notist-table-align-center"),
        TableAlignment::Right => Some("notist-table-align-right"),
    }
}

fn escape_text(output: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            _ => output.push(character),
        }
    }
}

fn escape_attribute(output: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(character),
        }
    }
}

fn safe_url(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    let scheme_end = value.find(':');
    let path_boundary = value.find(['/', '?', '#']);
    if let Some(scheme_end) = scheme_end
        && path_boundary.is_none_or(|boundary| scheme_end < boundary)
    {
        let scheme = &value[..scheme_end];
        if !scheme.chars().enumerate().all(|(index, character)| {
            character.is_ascii_alphabetic()
                || (index > 0
                    && (character.is_ascii_digit() || matches!(character, '+' | '-' | '.')))
        }) || !matches!(
            scheme.to_ascii_lowercase().as_str(),
            "http" | "https" | "mailto" | "tel"
        ) {
            return None;
        }
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use notist_eval::{Evaluator, structure};
    use notist_model::{
        Block, Content, Element, ElementNode, ModulePath, StructuredDocument, TextRange,
    };

    use super::*;

    fn node(element: Element, start: usize, end: usize) -> ElementNode {
        ElementNode {
            element,
            range: TextRange::new(start, end),
        }
    }

    #[test]
    fn renders_evaluated_document_structure() {
        let evaluation = Evaluator::default().evaluate(
            "#heading(level=2)[Title]\n\nBefore after\n\n#quote[First\n\nSecond]\n\n#raw(r#\"\"\"\nfn main() {}\n\"\"\"#, lang=\"rust\")",
        );
        let structured = structure(evaluation);

        let html = render(&structured.document);

        assert!(html.starts_with("<h2 id=\"notist-heading-0\" data-notist-start=\"0\""));
        assert!(html.contains("<p><span class=\"notist-text\""));
        assert!(html.contains("<blockquote"));
        assert!(html.contains(">First</span></p><p>"));
        assert!(html.contains("<pre"));
        assert!(html.contains("<code class=\"language-rust\">fn main() {}</code></pre>"));
    }

    #[test]
    fn renders_explicit_text_like_plain_markup() {
        let evaluator = Evaluator::default();
        let plain = render(&structure(evaluator.evaluate("plain < text")).document);
        let explicit = render(&structure(evaluator.evaluate("#text(\"plain < text\")")).document);
        assert!(plain.contains("plain &lt; text"));
        assert!(explicit.contains("plain &lt; text"));
        assert!(explicit.contains("class=\"notist-text\""));
    }

    #[test]
    fn renders_plain_and_explicit_paragraph_elements() {
        let evaluator = Evaluator::default();
        for source in ["plain paragraph", "#paragraph[explicit paragraph]"] {
            let html = render(&structure(evaluator.evaluate(source)).document);
            assert!(html.starts_with("<p data-notist-start=\"0\""));
            assert!(html.ends_with("</p>"));
        }
    }

    #[test]
    fn renders_explicit_code_function_as_inline_or_block_code() {
        let evaluator = Evaluator::default();
        let inline = render(&structure(evaluator.evaluate("#code(\"a < b\")")).document);
        assert!(inline.contains("<code"));
        assert!(inline.contains("a &lt; b</code>"));
        assert!(!inline.contains("<pre"));

        let block = render(
            &structure(evaluator.evaluate("#code(\"a < b\", lang=\"txt\", block=true)")).document,
        );
        assert!(block.contains("<pre"));
        assert!(block.contains("<code class=\"language-txt\">a &lt; b</code></pre>"));
    }

    #[test]
    fn escapes_text_attributes_and_raw_bodies() {
        let document = StructuredDocument {
            blocks: vec![Block::Element(node(
                Element::Custom {
                    name: "x\" onclick=\"bad".into(),
                    body: Content::single(Element::Text("<&>".into()), TextRange::new(2, 5)),
                    block: true,
                },
                0,
                5,
            ))],
        };

        let html = render(&document);

        assert!(html.contains("data-notist-name=\"x&quot; onclick=&quot;bad\""));
        assert!(html.contains("&lt;&amp;&gt;"));
        assert!(!html.contains("onclick=\"bad\""));
    }

    #[test]
    fn resolves_and_encodes_reference_links() {
        let evaluation =
            Evaluator::default().evaluate("[[intro page#A B]] [[super::index]] [[vault::shared]]");
        let structured = structure(evaluation);
        let current = ModulePath::from_segments(["notes".into(), "today".into()]);
        let options = RenderOptions {
            current_module: Some(&current),
            module_url_prefix: "/preview?module=",
        };

        let html = render_with_options(&structured.document, &options);

        assert!(html.contains(
            "href=\"/preview?module=vault%3A%3Anotes%3A%3Atoday%3A%3Aintro%20page#A%20B\""
        ));
        assert!(html.contains("href=\"/preview?module=vault%3A%3Anotes%3A%3Aindex\""));
        assert!(html.contains("href=\"/preview?module=vault%3A%3Ashared\""));
    }

    #[test]
    fn leaves_relative_references_unclickable_without_a_current_module() {
        let evaluation = Evaluator::default().evaluate("[[child]] [[vault::shared]]");
        let structured = structure(evaluation);

        let html = render(&structured.document);

        assert!(html.contains("notist-reference-unresolved"));
        assert!(html.contains("href=\"?module=vault%3A%3Ashared\""));
    }

    #[test]
    fn renders_explicit_ref_function_like_wiki_sugar() {
        let current = ModulePath::from_segments(["notes".into()]);
        let options = RenderOptions {
            current_module: Some(&current),
            module_url_prefix: "?module=",
        };
        let evaluator = Evaluator::default();
        let explicit = render_with_options(
            &structure(evaluator.evaluate("#ref(\"child\")")).document,
            &options,
        );
        let sugar = render_with_options(
            &structure(evaluator.evaluate("[[child]]")).document,
            &options,
        );
        assert!(explicit.contains("href=\"?module=vault%3A%3Anotes%3A%3Achild\""));
        assert!(sugar.contains("href=\"?module=vault%3A%3Anotes%3A%3Achild\""));
    }

    #[test]
    fn uses_a_caller_provided_reference_resolver() {
        let evaluation = Evaluator::default().evaluate("[[child]] [[missing]]");
        let structured = structure(evaluation);
        let current = ModulePath::root();
        let options = RenderOptions {
            current_module: Some(&current),
            module_url_prefix: "",
        };
        let resolver = |target: &ModulePath, _label: Option<&str>| {
            (target.segments() == ["child"]).then(|| "child/".into())
        };

        let html = render_with_reference_resolver(&structured.document, &options, &resolver);

        assert!(html.contains("href=\"child/\""));
        assert!(html.contains("notist-reference-unresolved"));
    }

    #[test]
    fn renders_strong_content_and_groups_nested_list_items() {
        let item = |text: &str, start| {
            node(
                Element::ListItem(Content::single(
                    Element::Text(text.into()),
                    TextRange::new(start, start + text.len()),
                )),
                start,
                start + text.len(),
            )
        };
        let document = StructuredDocument {
            blocks: vec![
                Block::Element(node(
                    Element::Paragraph(Content::single(
                        Element::Strong(Content::single(
                            Element::Text("important".into()),
                            TextRange::new(0, 9),
                        )),
                        TextRange::new(0, 9),
                    )),
                    0,
                    9,
                )),
                Block::Element(node(
                    Element::Quote {
                        body: Content {
                            elements: vec![item("one", 10), item("two", 14)],
                        },
                        attribution: None,
                    },
                    10,
                    17,
                )),
            ],
        };

        let html = render(&document);

        assert!(html.contains("<strong data-notist-start=\"0\" data-notist-end=\"9\">"));
        assert_eq!(html.matches("<ul>").count(), 1);
        assert_eq!(html.matches("<li").count(), 2);
    }

    #[test]
    fn renders_ordered_list_items_as_an_ordered_list() {
        let evaluation = Evaluator::default().evaluate("#enum::item[First]\n#enum::item[Second]");
        let structured = structure(evaluation);
        let html = render(&structured.document);
        assert_eq!(html.matches("<ol").count(), 1);
        assert_eq!(html.matches("<li").count(), 2);
        assert!(html.contains("First") && html.contains("Second"));
    }

    #[test]
    fn renders_explicit_list_containers() {
        let evaluation = Evaluator::default().evaluate(
            "#list[#list::item[One]#list::item[Two]]#enum[#enum::item(value=3)[Three]#enum::item[Four]]",
        );
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<ul data-notist-start="));
        assert!(html.contains("<ol start=\"3\" data-notist-start="));
        assert_eq!(html.matches("<li").count(), 4);
    }

    #[test]
    fn preserves_explicit_ordered_list_values() {
        let evaluation = Evaluator::default()
            .evaluate("#enum[#enum::item(value=4)[Fourth]#enum::item(value=9)[Ninth]]");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<ol start=\"4\""));
        assert!(html.contains("<li value=\"4\""));
        assert!(html.contains("<li value=\"9\""));
    }

    #[test]
    fn renders_indented_mixed_nested_lists() {
        let evaluation =
            Evaluator::default().evaluate("- parent\n  + first child\n  + second child\n- sibling");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert_eq!(html.matches("<ul").count(), 1);
        assert_eq!(html.matches("<ol>").count(), 1);
        assert_eq!(html.matches("<li").count(), 4);
        assert!(html.contains("first child") && html.contains("sibling"));
    }

    #[test]
    fn renders_tables_as_rows_and_cells() {
        let evaluation = Evaluator::default().evaluate(
            "#table(columns=2)[#table::cell[One]#table::cell[Two]#table::cell[Three]#table::cell[Four]]",
        );
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<div class=\"notist-table-wrapper\"><table"));
        assert!(html.contains("<table data-notist-columns=\"2\""));
        assert_eq!(html.matches("<tr>").count(), 2);
        assert_eq!(html.matches("<td").count(), 4);
        assert!(html.contains("One") && html.contains("Four"));
        assert!(html.contains("</table></div>"));
    }

    #[test]
    fn renders_table_cell_spans() {
        let evaluation = Evaluator::default().evaluate(
            "#table(columns=3, align=\"left,center,right\")[#table::cell(colspan=2)[Wide]#table::cell(rowspan=2)[Tall]#table::cell[Next]#table::cell[Last]]",
        );
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains(" colspan=\"2\""));
        assert!(html.contains(" rowspan=\"2\""));
        assert_eq!(html.matches("notist-table-align-left").count(), 2);
        assert_eq!(html.matches("notist-table-align-center").count(), 1);
        assert_eq!(html.matches("notist-table-align-right").count(), 1);
    }

    #[test]
    fn renders_table_headers_semantically() {
        let evaluation =
            Evaluator::default().evaluate("| Name | Value |\n| --- | :---: |\n| one | two |");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert_eq!(html.matches("<thead>").count(), 1);
        assert_eq!(html.matches("</th>").count(), 2);
        assert_eq!(html.matches("<td").count(), 2);
        assert_eq!(html.matches("<tbody>").count(), 1);
        assert_eq!(html.matches("notist-table-align-center").count(), 2);
    }

    #[test]
    fn renders_rich_pipe_table_cells() {
        let evaluation = Evaluator::default().evaluate(
            "| Code | Reference | Content |\n| --- | --- | --- |\n| `a|b` | [[guide]] | #strong[x | y] |",
        );
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<code"));
        assert!(html.contains("a|b"));
        assert!(html.contains("class=\"notist-reference"));
        assert!(html.contains("<strong"));
        assert_eq!(html.matches("</th>").count(), 3);
        assert_eq!(html.matches("<td").count(), 3);
    }

    #[test]
    fn renders_table_captions() {
        let evaluation = Evaluator::default().evaluate("| A | B |\n| C | D |\n: *Inventory*");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<caption>"));
        assert!(html.contains("Inventory"));
        assert!(html.contains("</caption>"));
    }

    #[test]
    fn renders_escaped_pipes_inside_table_cells() {
        let evaluation = Evaluator::default().evaluate("| A \\| B | C |");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert_eq!(html.matches("<td").count(), 2);
        assert!(html.contains(">A </span>"));
        assert!(html.contains(">|</span>"));
        assert!(html.contains("> B</span>"));
        assert!(!html.contains("A \\| B"));
    }

    #[test]
    fn renders_inline_elements_with_semantic_tags() {
        let evaluation = Evaluator::default().evaluate(
            "#strong[bold] #emph[slanted] #link(\"https://example.test\", \"Docs\")[site]#linebreak()",
        );
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<strong"));
        assert!(html.contains("<em"));
        assert!(html.contains("<a class=\"notist-link\" href=\"https://example.test\""));
        assert!(html.contains("title=\"Docs\""));
        assert!(html.contains("<br>"));
    }

    #[test]
    fn renders_keyboard_input_semantically() {
        let evaluation = Evaluator::default().evaluate("Press #kbd[Ctrl + S].");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<kbd class=\"notist-keyboard\""));
        assert!(html.contains("Ctrl + S"));
        assert!(html.contains("</kbd>"));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn renders_sample_output_semantically() {
        let evaluation = Evaluator::default().evaluate("#samp[Saved 3 files]");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<samp class=\"notist-sample\""));
        assert!(html.contains("Saved 3 files"));
        assert!(html.contains("</samp>"));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn renders_machine_readable_time_semantically() {
        let evaluation = Evaluator::default().evaluate("#time(\"2026-07-21\")[July 21]");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<time datetime=\"2026-07-21\""));
        assert!(html.contains("July 21"));
        assert!(html.contains("</time>"));
    }

    #[test]
    fn renders_bare_email_addresses_as_mailto_links() {
        let evaluation = Evaluator::default().evaluate("Contact hello@example.test.");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("href=\"mailto:hello@example.test\""));
        assert!(html.contains(">hello@example.test</span>"));
    }

    #[test]
    fn renders_images_with_escaped_attributes() {
        let evaluation = Evaluator::default().evaluate(
            "#image(source=r#\"image\\\".png\"#, alt=\"A < B\", title=\"Flow & details\", width=640, height=480)",
        );
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("class=\"notist-image\""));
        assert!(html.contains("src=\"image\\&quot;.png\""));
        assert!(html.contains("alt=\"A &lt; B\""));
        assert!(html.contains("title=\"Flow &amp; details\""));
        assert!(html.contains("width=\"640\""));
        assert!(html.contains("height=\"480\""));
        assert!(html.contains("loading=\"lazy\""));
    }

    #[test]
    fn renders_figures_with_captions() {
        let evaluation = Evaluator::default()
            .evaluate("#figure(source=\"diagram.png\", title=\"Flow\")[Build *flow*]");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<figure class=\"notist-figure\""));
        assert!(html.contains("title=\"Flow\""));
        assert!(html.contains("<figcaption>"));
        assert!(html.contains("<strong"));
        assert!(html.contains("</figcaption></figure>"));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn renders_video_media() {
        let evaluation =
            Evaluator::default().evaluate("#video(source=\"movie.mp4\", poster=\"poster.png\")");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<video class=\"notist-video\" src=\"movie.mp4\""));
        assert!(html.contains("poster=\"poster.png\""));
        assert!(html.contains(" controls"));
        assert!(html.contains("</video>"));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn renders_audio_media() {
        let evaluation = Evaluator::default().evaluate("#audio(\"sound.ogg\", loop=true)");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<audio class=\"notist-audio\" src=\"sound.ogg\""));
        assert!(html.contains(" controls"));
        assert!(html.contains(" loop"));
        assert!(html.contains("</audio>"));
    }

    #[test]
    fn renders_strike_content() {
        let evaluation = Evaluator::default().evaluate("~~obsolete~~");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<s data-notist-start=\"0\""));
        assert!(html.contains("obsolete"));
        assert!(html.contains("</s>"));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn renders_insert_content() {
        let evaluation = Evaluator::default().evaluate("++replacement++");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<ins data-notist-start=\"0\""));
        assert!(html.contains("replacement"));
        assert!(html.contains("</ins>"));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn renders_focusable_spoiler_content() {
        let evaluation = Evaluator::default().evaluate(">!hidden ending!<");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<span class=\"notist-spoiler\" tabindex=\"0\""));
        assert!(html.contains("title=\"Focus or hover to reveal\""));
        assert!(html.contains("hidden ending"));
        assert!(html.contains("</span>"));
    }

    #[test]
    fn renders_heading_and_explicit_quote() {
        let evaluator = Evaluator::default();
        let heading = render(&structure(evaluator.evaluate("= Title")).document);
        let second_heading = render(&structure(evaluator.evaluate("== Subtitle")).document);
        let quote = render(&structure(evaluator.evaluate("#quote[Quoted]")).document);
        assert!(heading.contains("<h1"));
        assert!(second_heading.contains("<h2"));
        assert!(quote.contains("<blockquote"));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn renders_outlines_with_heading_anchors() {
        let evaluation = Evaluator::default().evaluate(
            "#heading[Top]\n#heading(level=2)[Nested]\n#heading(level=4)[Hidden]\n#outline(depth=2)",
        );
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("id=\"notist-heading-0\""));
        assert!(
            html.contains("<nav class=\"notist-outline\" aria-label=\"Table of contents\"><ol>")
        );
        assert!(html.contains("href=\"#notist-heading-0\""));
        assert!(html.contains("href=\"#notist-heading-14\""));
        assert!(!html.contains("notist-outline-level-4"));
    }

    #[test]
    fn renders_quote_attribution_as_cite() {
        let evaluation =
            Evaluator::default().evaluate("#quote(attribution=[Ada Lovelace])[That brain of mine]");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<blockquote"));
        assert!(html.contains("<footer><cite>"));
        assert!(html.contains("Ada Lovelace"));
    }

    #[test]
    fn renders_nested_explicit_quotes_as_nested_blockquotes() {
        let evaluation = Evaluator::default().evaluate("#quote[#quote[Nested quotation]]");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert_eq!(html.matches("<blockquote").count(), 2);
        assert!(html.contains("Nested quotation"));
    }

    #[test]
    fn renders_callouts_as_semantic_asides() {
        let evaluation =
            Evaluator::default().evaluate("#callout(kind=\"tip\", title=[Tip])[Use *small* steps]");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<aside class=\"notist-callout\" data-notist-kind=\"tip\""));
        assert!(html.contains("<div class=\"notist-callout-title\">"));
        assert!(html.contains("<strong"));
        assert!(html.contains("</aside>"));
    }

    #[test]
    fn renders_details_disclosure() {
        let evaluation =
            Evaluator::default().evaluate("#details(summary=[More], open=true)[Hidden content]");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<details class=\"notist-details\" open"));
        assert!(html.contains("<summary>"));
        assert!(html.contains("Hidden content"));
        assert!(html.contains("</details>"));

        let default_summary = Evaluator::default().evaluate("#details[Hidden content]");
        let html = render(&structure(default_summary).document);
        assert!(html.contains("<summary>Details</summary>"));
    }

    #[test]
    fn renders_inline_content_inside_block_sugar() {
        let evaluation = Evaluator::default().evaluate("- *bold*\n- _slanted_");
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<li"));
        assert!(html.contains("<strong"));
        assert!(html.contains("<em"));
    }

    #[test]
    fn renders_rule_and_pagebreak_elements() {
        let evaluation = Evaluator::default().evaluate("#rule()\n\n#pagebreak()");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("class=\"notist-rule\""));
        assert!(html.contains("class=\"notist-pagebreak\""));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn renders_definition_lists() {
        let evaluation =
            Evaluator::default().evaluate("/ API: Application interface\n/ URL: Address");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert_eq!(html.matches("<dl").count(), 1);
        assert_eq!(html.matches("<dt").count(), 2);
        assert_eq!(html.matches("<dd>").count(), 2);
        assert!(html.contains("Application interface"));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn renders_explicit_terms_and_task_containers() {
        let evaluation = Evaluator::default().evaluate(
            "#terms[#terms::item(term=[API])[Interface]]#task[#task::item[Todo]#task::item(checked=true)[Done]]",
        );
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<dl data-notist-start="));
        assert!(html.contains("<ul class=\"notist-task-list\" data-notist-start="));
        assert_eq!(html.matches("type=\"checkbox\" disabled").count(), 2);
        assert_eq!(html.matches(" disabled checked").count(), 1);
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn renders_nested_definition_lists() {
        let evaluation =
            Evaluator::default().evaluate("/ API: Interface\n  / HTTP: Transport\n/ URL: Address");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert_eq!(html.matches("<dl").count(), 2);
        assert_eq!(html.matches("<dt").count(), 3);
        assert!(html.contains("HTTP") && html.contains("Address"));
    }

    #[test]
    fn renders_task_lists_with_disabled_checkboxes() {
        let evaluation = Evaluator::default().evaluate("- [ ] Write tests\n- [x] Ship");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert_eq!(html.matches("class=\"notist-task-list\"").count(), 1);
        assert_eq!(html.matches("type=\"checkbox\" disabled").count(), 2);
        assert_eq!(html.matches(" disabled checked").count(), 1);
        assert!(html.contains("aria-label=\"Incomplete task\""));
        assert!(html.contains("aria-label=\"Completed task\""));
    }

    #[test]
    fn renders_nested_task_lists() {
        let evaluation =
            Evaluator::default().evaluate("- [ ] Parent\n  - [x] Child\n- [x] Sibling");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert_eq!(html.matches("class=\"notist-task-list\"").count(), 2);
        assert_eq!(html.matches("type=\"checkbox\" disabled").count(), 3);
        assert!(html.contains("Child") && html.contains("Sibling"));
    }

    #[test]
    fn preserves_unresolved_trailing_content_and_bodyless_calls() {
        let document = StructuredDocument {
            blocks: vec![
                Block::Element(node(
                    Element::Paragraph(Content {
                        elements: vec![node(
                            Element::UnresolvedCall {
                                name: "plugin::inline".into(),
                                arguments: Some("kind=\"tip\"".into()),
                                trailing: Some(Content::single(
                                    Element::Text("visible".into()),
                                    TextRange::new(10, 17),
                                )),
                                block: false,
                            },
                            0,
                            18,
                        )],
                    }),
                    0,
                    18,
                )),
                Block::Element(node(
                    Element::UnresolvedCall {
                        name: "plugin::bodyless".into(),
                        arguments: None,
                        trailing: None,
                        block: true,
                    },
                    19,
                    40,
                )),
            ],
        };

        let html = render(&document);

        assert!(html.contains("<span class=\"notist-unresolved-call\""));
        assert!(html.contains("data-notist-arguments=\"kind=&quot;tip&quot;\""));
        assert!(html.contains(">visible</span></span>"));
        assert!(html.contains("<div class=\"notist-unresolved-call\""));
        assert!(html.contains("data-notist-name=\"plugin::bodyless\""));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn renders_additional_inline_elements_with_semantic_tags() {
        let evaluation = Evaluator::default().evaluate("==marked==__under__^2^~i~");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<mark data-notist-start=\"0\""));
        assert!(html.contains("<u data-notist-start=\"10\""));
        assert!(html.contains("<sup data-notist-start=\"19\""));
        assert!(html.contains("<sub data-notist-start=\"22\""));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn collects_and_links_footnotes() {
        let evaluation =
            Evaluator::default().evaluate("First^[Source *one*] and second#footnote[Source two].");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("id=\"notist-footnote-ref-1\""));
        assert!(html.contains("id=\"notist-footnote-ref-2\""));
        assert!(html.contains("<section class=\"notist-footnotes\""));
        assert!(html.contains("id=\"notist-footnote-1\""));
        assert!(html.contains("href=\"#notist-footnote-ref-2\""));
        assert!(html.contains("<strong"));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn omits_comments_from_rendered_html() {
        let evaluation = Evaluator::default().evaluate("Visible %%secret%% text");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("Visible"));
        assert!(html.contains("text"));
        assert!(!html.contains("secret"));
    }

    #[test]
    fn renders_math_containers_with_escaped_source() {
        let evaluation = Evaluator::default().evaluate("$x < y$ $$a & b$$");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<span class=\"notist-math\""));
        assert!(html.contains("x &lt; y"));
        assert!(html.contains("<div class=\"notist-math\""));
        assert!(html.contains("a &amp; b"));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn renders_abbreviations_with_escaped_expansion() {
        let evaluation =
            Evaluator::default().evaluate("#abbr(term=\"A&B\", expansion=\"Alpha < Beta\")");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<abbr title=\"Alpha &lt; Beta\""));
        assert!(html.contains("A&amp;B</abbr>"));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn renders_semantic_citations() {
        let evaluation = Evaluator::default().evaluate("[@doe&roe, p. <17>]");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<cite class=\"notist-citation\" data-notist-key=\"doe&amp;roe\""));
        assert!(html.contains("[doe&amp;roe, p. &lt;17&gt;]</cite>"));
    }

    #[test]
    fn unsafe_url_schemes_degrade_without_executable_attributes() {
        let evaluation = Evaluator::default().evaluate(
            "#link(destination=\"javascript:alert(1)\")[Click] \
             #image(source=\"data:text/html,<script>alert(1)</script>\", alt=\"visible\")",
        );
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("notist-url-unsafe"));
        assert!(html.contains("Click"));
        assert!(html.contains("alt=\"visible\""));
        assert!(!html.contains("javascript:"));
        assert!(!html.contains("data:text/html"));
        assert!(!html.contains("<script>"));
    }
}
