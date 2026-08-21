//! Semantic HTML rendering for structured Notist documents.

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use notist_eval::{ElementTree, instance_node_to_legacy, instances_to_legacy_content};
use notist_model::{
    Block, Content, CustomField, Element, ElementNode, FieldValue, InstanceNode, ModulePath,
    ModuleReference, StructuredDocument, TableAlignment, TableCellPlacement, TextRange,
    WikiReference, table_layout,
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

/// Resolves an absolute module target and optional label to an HTML URL.
pub type ReferenceResolver<'a> = dyn Fn(&ModulePath, Option<&str>) -> Option<String> + 'a;

/// A source annotation projected onto the rendered DOM.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedAnnotation {
    /// The source range of the annotated scope.
    pub scope: TextRange,
    /// The optional explicit scope id.
    pub id: Option<String>,
    /// Classes appended to the HTML `class` attribute of the projected element.
    pub classes: Vec<String>,
    /// Tags exposed as a space-separated `data-notist-tag` attribute.
    pub tags: Vec<String>,
    /// Key-value properties exposed as `data-notist-{key}` attributes.
    pub properties: Vec<(String, String)>,
}

/// A document heading with its assigned HTML anchor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedHeading {
    /// The one-based heading level.
    pub level: u8,
    /// The HTML anchor assigned to the heading.
    pub id: String,
    /// The plain text of the heading body.
    pub text: String,
}

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

/// Input passed to a custom HTML renderer for one plugin element.
pub struct CustomRenderInput<'a> {
    /// The plugin element name.
    pub name: &'a str,
    /// The evaluated body content.
    pub body: &'a Content,
    /// Whether the element is block-level.
    pub block: bool,
    /// Serialized constructor fields.
    pub fields: &'a [CustomField],
}

/// A target-specific renderer for a plugin element.
pub trait CustomHtmlRenderer: Send + Sync {
    /// The plugin element name this renderer handles.
    fn element_name(&self) -> &str;

    /// Renders the element into `output`. Return `true` if handled.
    fn render(&self, input: &CustomRenderInput<'_>, output: &mut String) -> bool;
}

/// A registry of plugin HTML renderers.
pub struct HtmlRendererRegistry {
    renderers: Vec<Box<dyn CustomHtmlRenderer>>,
}

impl Default for HtmlRendererRegistry {
    fn default() -> Self {
        let mut registry = Self {
            renderers: Vec::new(),
        };
        registry.register(ShaderHtmlRenderer);
        registry
    }
}

impl HtmlRendererRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self {
            renderers: Vec::new(),
        }
    }

    /// Registers a renderer.
    pub fn register(&mut self, renderer: impl CustomHtmlRenderer + 'static) {
        self.renderers.push(Box::new(renderer));
    }

    /// Tries all renderers matching the element name.
    ///
    /// Renderers may declare either the qualified name (`demo::box`) or the
    /// local name (`box`). Matching the local suffix keeps renderer manifests
    /// stable when package namespaces are introduced later.
    pub fn render(&self, input: &CustomRenderInput<'_>, output: &mut String) -> bool {
        let local = input.name.rsplit("::").next().unwrap_or(input.name);
        for renderer in &self.renderers {
            let declared = renderer.element_name();
            if (declared == input.name || declared == local) && renderer.render(input, output) {
                return true;
            }
        }
        false
    }
}

/// Registers a manifest-declared Web Component projection.
///
/// The renderer emits the custom element and scalar constructor fields as
/// `data-*` attributes. The package's JS/CSS assets are injected into the page
/// head by the CLI build layer. Untrusted field and body values remain escaped
/// exactly like every other HTML projection path.
pub fn register_web_component_renderer(
    registry: &mut HtmlRendererRegistry,
    element_name: &str,
    tag: &str,
) {
    registry.register(WebComponentHtmlRenderer {
        element_name: element_name.to_owned(),
        tag: tag.to_owned(),
    });
}

/// A generic projection for plugin Web Components declared in `plugin.json`.
pub struct WebComponentHtmlRenderer {
    element_name: String,
    tag: String,
}

impl CustomHtmlRenderer for WebComponentHtmlRenderer {
    fn element_name(&self) -> &str {
        &self.element_name
    }

    fn render(&self, input: &CustomRenderInput<'_>, output: &mut String) -> bool {
        output.push('<');
        output.push_str(&self.tag);
        output.push_str(" class=\"notist-web-component\" data-notist-element=\"");
        escape_attribute(output, input.name);
        output.push('"');
        for field in input.fields {
            let value = match &field.value {
                notist_model::ElementValue::None => continue,
                notist_model::ElementValue::Bool(value) => value.to_string(),
                notist_model::ElementValue::Int(value) => value.to_string(),
                notist_model::ElementValue::Float(value) => value.to_string(),
                notist_model::ElementValue::String(value) => value.clone(),
                notist_model::ElementValue::Content(_) | notist_model::ElementValue::Array(_) => {
                    continue;
                }
            };
            output.push_str(" data-");
            escape_attribute(output, &field.name);
            output.push_str("=\"");
            escape_attribute(output, &value);
            output.push('"');
        }
        output.push('>');
        if !input.body.elements.is_empty() {
            let fallback = content_plain_text(input.body);
            output.push_str("<p>");
            escape_text(output, &fallback);
            output.push_str("</p>");
        }
        output.push_str("</");
        output.push_str(&self.tag);
        output.push('>');
        true
    }
}

/// Built-in Shadertoy-like renderer for the `shader` plugin.
struct ShaderHtmlRenderer;

impl CustomHtmlRenderer for ShaderHtmlRenderer {
    fn element_name(&self) -> &str {
        "shader"
    }

    fn render(&self, input: &CustomRenderInput<'_>, output: &mut String) -> bool {
        let mut source = String::new();
        let mut width = 800i64;
        let mut height = 600i64;
        for field in input.fields {
            match (field.name.as_str(), &field.value) {
                ("source", notist_model::ElementValue::String(value)) => source = value.clone(),
                ("width", notist_model::ElementValue::Int(value)) => width = *value,
                ("height", notist_model::ElementValue::Int(value)) => height = *value,
                _ => {}
            }
        }
        if source.is_empty() {
            return false;
        }

        output.push_str("<notist-shader class=\"notist-shader\" data-shader-source=\"");
        escape_attribute(output, &source);
        output.push_str("\" data-width=\"");
        write!(output, "{width}").unwrap();
        output.push_str("\" data-height=\"");
        write!(output, "{height}").unwrap();
        output.push_str("\">");
        if !input.body.elements.is_empty() {
            let fallback = content_plain_text(input.body);
            output.push_str("<p>");
            escape_text(output, &fallback);
            output.push_str("</p>");
        }
        output.push_str("</notist-shader>");
        true
    }
}

/// Renders a structured document using the default options.
pub fn render(document: &StructuredDocument) -> String {
    render_with_options(document, &RenderOptions::default())
}

/// Renders a structured document as an HTML fragment.
pub fn render_with_options(document: &StructuredDocument, options: &RenderOptions<'_>) -> String {
    render_internal(
        document,
        options,
        None,
        &[],
        &HtmlRendererRegistry::default(),
    )
}

/// Renders a document using a caller-provided module reference URL resolver.
///
/// Returning `None` leaves the reference visible but unclickable.
pub fn render_with_reference_resolver(
    document: &StructuredDocument,
    options: &RenderOptions<'_>,
    resolver: &ReferenceResolver<'_>,
) -> String {
    render_internal(
        document,
        options,
        Some(resolver),
        &[],
        &HtmlRendererRegistry::default(),
    )
}

/// Renders a document with caller-resolved module-reference URLs and source annotations.
pub fn render_with_resolvers(
    document: &StructuredDocument,
    options: &RenderOptions<'_>,
    reference_resolver: &ReferenceResolver<'_>,
    annotations: &[RenderedAnnotation],
) -> String {
    render_internal(
        document,
        options,
        Some(reference_resolver),
        annotations,
        &HtmlRendererRegistry::default(),
    )
}

/// Renders a document with a custom plugin renderer registry.
pub fn render_with_renderers(
    document: &StructuredDocument,
    options: &RenderOptions<'_>,
    reference_resolver: &ReferenceResolver<'_>,
    annotations: &[RenderedAnnotation],
    renderers: &HtmlRendererRegistry,
) -> String {
    render_internal(
        document,
        options,
        Some(reference_resolver),
        annotations,
        renderers,
    )
}

/// Renders a canonical [`ElementTree`] through the legacy projection bridge.
///
/// New hosts should call this entry point instead of constructing a
/// `StructuredDocument` manually; projection and rendering remain one
/// data-flow step, and the canonical tree is the stable input shape.
pub fn render_element_tree(tree: &ElementTree) -> String {
    render_element_tree_with_renderers(
        tree,
        &RenderOptions::default(),
        &|_, _| None,
        &[],
        &HtmlRendererRegistry::default(),
    )
}

/// Renders an [`ElementTree`] with caller-provided projection options,
/// reference resolution, annotations, and plugin renderers.
pub fn render_element_tree_with_renderers(
    tree: &ElementTree,
    options: &RenderOptions<'_>,
    reference_resolver: &ReferenceResolver<'_>,
    annotations: &[RenderedAnnotation],
    renderers: &HtmlRendererRegistry,
) -> String {
    let plan = AnchorPlan::compute_tree(tree, annotations);
    let mut renderer = Renderer {
        output: String::new(),
        options,
        reference_resolver: Some(reference_resolver),
        annotations,
        renderers,
        plan,
        current_block: None,
        inherited_coverage: Vec::new(),
    };
    renderer.element_tree(tree);
    renderer.output
}

/// Computes the resolvable anchor labels of a document: explicit scope ids first,
/// then heading default ids (heading plain text), each mapped to its HTML anchor.
pub fn module_anchors(
    document: &StructuredDocument,
    annotations: &[RenderedAnnotation],
) -> Vec<(String, String)> {
    AnchorPlan::compute(document, annotations).labels
}

/// Collects the top-level headings of a document with their assigned HTML anchors.
pub fn outline_entries(
    document: &StructuredDocument,
    annotations: &[RenderedAnnotation],
) -> Vec<RenderedHeading> {
    let plan = AnchorPlan::compute(document, annotations);
    collect_outline_entries(document, &plan)
}

/// Computes resolvable anchor labels directly from a canonical tree.
pub fn module_anchors_tree(
    tree: &ElementTree,
    annotations: &[RenderedAnnotation],
) -> Vec<(String, String)> {
    AnchorPlan::compute_tree(tree, annotations).labels
}

/// Collects top-level headings directly from a canonical tree.
pub fn outline_entries_tree(
    tree: &ElementTree,
    annotations: &[RenderedAnnotation],
) -> Vec<RenderedHeading> {
    let plan = AnchorPlan::compute_tree(tree, annotations);
    collect_outline_entries_tree(tree, &plan)
}

fn render_internal<'a>(
    document: &StructuredDocument,
    options: &'a RenderOptions<'a>,
    resolver: Option<&'a ReferenceResolver<'a>>,
    annotations: &'a [RenderedAnnotation],
    renderers: &'a HtmlRendererRegistry,
) -> String {
    let plan = AnchorPlan::compute(document, annotations);
    let mut renderer = Renderer {
        output: String::new(),
        options,
        reference_resolver: resolver,
        annotations,
        renderers,
        plan,
        current_block: None,
        inherited_coverage: Vec::new(),
    };
    renderer.document(document);
    renderer.output
}

struct Renderer<'a, 'options> {
    output: String,
    options: &'options RenderOptions<'a>,
    reference_resolver: Option<&'options ReferenceResolver<'options>>,
    annotations: &'options [RenderedAnnotation],
    renderers: &'options HtmlRendererRegistry,
    plan: AnchorPlan,
    /// Range key of the top-level block currently being rendered, used to look
    /// up inline wrapper candidates. `None` outside block rendering.
    current_block: Option<(usize, usize)>,
    /// Indices of the annotations whose wrapping span is already open around
    /// an ancestor inline element; their coverage is inherited, not re-wrapped.
    inherited_coverage: Vec<usize>,
}

impl Renderer<'_, '_> {
    fn document(&mut self, document: &StructuredDocument) {
        for block in &document.blocks {
            self.block(block);
        }
    }

    /// Renders the canonical tree directly. Sections are emitted from their
    /// `core::section` instance; every other node is projected per leaf just
    /// in time for the existing target renderer.
    fn element_tree(&mut self, tree: &ElementTree) {
        for root in &tree.roots {
            self.tree_node(root);
        }
    }

    fn tree_node(&mut self, node: &InstanceNode) {
        if node.instance.is_core("section") {
            self.tree_section(node);
            return;
        }
        self.current_block = Some(range_key(node.range));
        self.tree_element(node, RenderPosition::Block);
    }

    fn tree_section(&mut self, node: &InstanceNode) {
        let Some(heading_node) = node.instance.body.first() else {
            return;
        };
        let range = node.range;
        let key = range_key(range);
        self.current_block = Some(key);
        self.output.push_str("<section");
        if let Some(projection) = self.plan.projections.get(&key) {
            if !projection.classes.is_empty() {
                self.output.push_str(" class=\"");
                escape_attribute(&mut self.output, &projection.classes.join(" "));
                self.output.push('"');
            }
            if !projection.tags.is_empty() {
                self.output.push_str(" data-notist-tag=\"");
                escape_attribute(&mut self.output, &projection.tags.join(" "));
                self.output.push('"');
            }
            for (property, value) in &projection.properties {
                self.output.push_str(" data-notist-");
                self.output.push_str(&property_attribute_key(property));
                self.output.push_str("=\"");
                escape_attribute(&mut self.output, value);
                self.output.push('"');
            }
        }
        write!(
            self.output,
            " data-notist-start=\"{}\" data-notist-end=\"{}\"",
            range.start, range.end
        )
        .unwrap();
        self.output.push('>');
        self.tree_node(heading_node);
        for child in &node.instance.body[1..] {
            self.tree_node(child);
        }
        self.output.push_str("</section>");
    }

    fn tree_element(&mut self, node: &InstanceNode, position: RenderPosition) {
        let instance = &node.instance;
        let Some(local) = instance.name.core_local() else {
            let Some(legacy) = instance_node_to_legacy(node) else {
                return;
            };
            self.element(&legacy.element, &legacy, position);
            return;
        };
        match local {
            "text" => {
                let Some(FieldValue::String(text)) = instance.field("text") else {
                    return;
                };
                self.output.push_str("<span class=\"notist-text\"");
                self.range_attributes_range(node.range);
                self.output.push('>');
                escape_text(&mut self.output, text);
                self.output.push_str("</span>");
            }
            "paragraph" => {
                self.output.push_str("<p");
                self.projected_class_attribute_range(node.range);
                self.range_attributes_range(node.range);
                self.output.push('>');
                self.tree_body_inline_content(&instance.body);
                self.output.push_str("</p>");
            }
            "heading" => {
                let level = match instance.field("level") {
                    Some(FieldValue::Int(level)) => (*level).clamp(1, 6),
                    _ => 1,
                };
                write!(self.output, "<h{level}").unwrap();
                self.projected_class_attribute_range(node.range);
                self.range_attributes_range(node.range);
                self.output.push('>');
                self.tree_body_inline_content(&instance.body);
                write!(self.output, "</h{level}>").unwrap();
            }
            "strong" | "emph" | "strike" | "underline" => {
                let tag = match local {
                    "strong" => "strong",
                    "emph" => "em",
                    "strike" => "s",
                    _ => "u",
                };
                self.output.push('<');
                self.output.push_str(tag);
                self.range_attributes_range(node.range);
                self.output.push('>');
                self.tree_body_inline_content(&instance.body);
                self.output.push_str("</");
                self.output.push_str(tag);
                self.output.push('>');
            }
            "rule" => {
                self.output.push_str("<hr class=\"notist-rule\"");
                self.range_attributes_range(node.range);
                self.output.push('>');
            }
            _ => {
                let Some(legacy) = instance_node_to_legacy(node) else {
                    return;
                };
                self.element(&legacy.element, &legacy, position);
            }
        }
    }

    /// Projects a canonical inline body to legacy `Content` and renders it with
    /// the existing coverage-aware inline renderer.
    fn tree_body_inline_content(&mut self, body: &[InstanceNode]) {
        if let Some(content) = instances_to_legacy_content(body) {
            self.inline_content(&content);
        }
    }

    fn block(&mut self, block: &Block) {
        match block {
            Block::Element(node) => {
                self.current_block = Some(range_key(node.range));
                self.element(&node.element, node, RenderPosition::Block);
            }
            Block::Section { heading, body, .. } => {
                // D0010: sections render as nested <section> nodes; section-
                // level annotation entries project onto the section tag.
                let range = block.range();
                let key = range_key(range);
                self.current_block = Some(key);
                self.output.push_str("<section");
                if let Some(projection) = self.plan.projections.get(&key) {
                    if !projection.classes.is_empty() {
                        self.output.push_str(" class=\"");
                        escape_attribute(&mut self.output, &projection.classes.join(" "));
                        self.output.push('"');
                    }
                    if !projection.tags.is_empty() {
                        self.output.push_str(" data-notist-tag=\"");
                        escape_attribute(&mut self.output, &projection.tags.join(" "));
                        self.output.push('"');
                    }
                    for (property, value) in &projection.properties {
                        self.output.push_str(" data-notist-");
                        self.output.push_str(&property_attribute_key(property));
                        self.output.push_str("=\"");
                        escape_attribute(&mut self.output, value);
                        self.output.push('"');
                    }
                }
                write!(
                    self.output,
                    " data-notist-start=\"{}\" data-notist-end=\"{}\"",
                    range.start, range.end
                )
                .unwrap();
                self.output.push('>');
                self.block(&Block::Element(heading.clone()));
                for child in body {
                    self.block(child);
                }
                self.output.push_str("</section>");
            }
        }
    }

    /// Renders a figure body with framing whitespace-only Text and Parbreak
    /// nodes trimmed: the body content block usually contributes indentation
    /// and a framing newline that should not become empty paragraphs around
    /// the wrapped block.
    fn figure_body(&mut self, content: &Content) {
        let is_framing = |node: &ElementNode| {
            matches!(&node.element, Element::Parbreak)
                || matches!(&node.element, Element::Text(text) if text.trim().is_empty())
        };
        let first = content
            .elements
            .iter()
            .position(|node| !is_framing(node))
            .unwrap_or(content.elements.len());
        let last = content
            .elements
            .iter()
            .rposition(|node| !is_framing(node))
            .map_or(first, |index| index + 1);
        let trimmed = Content {
            elements: content.elements[first..last].to_vec(),
        };
        self.flow_content(&trimmed);
    }

    fn inline_content(&mut self, content: &Content) {
        let mut open_coverage = Vec::new();
        for node in &content.elements {
            self.inline_element_with_coverage(node, &mut open_coverage);
        }
        self.annotation_span_close(&mut open_coverage);
    }

    fn flow_content(&mut self, content: &Content) {
        let mut paragraph_open = false;
        let mut open_coverage = Vec::new();
        let mut index = 0;

        while index < content.elements.len() {
            let node = &content.elements[index];
            if node.element.is_inline() {
                if !paragraph_open {
                    self.output.push_str("<p>");
                    paragraph_open = true;
                }
                self.inline_element_with_coverage(node, &mut open_coverage);
                index += 1;
                continue;
            }

            if paragraph_open {
                // Annotation spans never cross block boundaries: any open span
                // closes before the paragraph tag does.
                self.annotation_span_close(&mut open_coverage);
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
                element => {
                    self.element(element, node, RenderPosition::Block);
                    index += 1;
                }
            }
        }

        if paragraph_open {
            self.annotation_span_close(&mut open_coverage);
            self.output.push_str("</p>");
        }
    }

    /// Renders one inline element of a content sequence, transitioning the
    /// open annotation span when the element's coverage differs from the
    /// currently open one.
    fn inline_element_with_coverage(&mut self, node: &ElementNode, open: &mut Vec<usize>) {
        let coverage = self.inline_coverage(node);
        // D0010: a text node straddling an annotation boundary is split into
        // fragments so the covered fragment is wrapped and the rest stays
        // plain.
        if let Element::Text(text) = &node.element {
            let partial = self.partial_coverage(node);
            if !partial.is_empty() {
                self.render_split_text(text, node.range, &coverage, &partial, open);
                return;
            }
        }
        if coverage != *open {
            self.annotation_span_close(open);
            if !coverage.is_empty() {
                self.annotation_span_open(&coverage);
            }
            *open = coverage;
        }
        // The open span already covers the rendered element: its descendants
        // inherit the coverage instead of being wrapped again.
        self.inherited_coverage.extend(open.iter());
        self.element(&node.element, node, RenderPosition::Inline);
        let inherited = self.inherited_coverage.len() - open.len();
        self.inherited_coverage.truncate(inherited);
    }

    /// Returns wrapper candidates whose scope intersects but does not fully
    /// contain the element range (D0010 text splitting).
    fn partial_coverage(&self, node: &ElementNode) -> Vec<usize> {
        if !node.element.is_inline() {
            return Vec::new();
        }
        let Some(candidates) = self
            .current_block
            .and_then(|block| self.plan.inline_wrappers.get(&block))
        else {
            return Vec::new();
        };
        candidates
            .iter()
            .copied()
            .filter(|index| {
                !self.inherited_coverage.contains(index)
                    && !contains(self.annotations[*index].scope, node.range)
                    && intersects(self.annotations[*index].scope, node.range)
            })
            .collect()
    }

    /// Renders a text node split at annotation boundaries: each fragment is
    /// wrapped by exactly the annotations covering it (D0010), with correct
    /// span nesting.
    fn render_split_text(
        &mut self,
        text: &str,
        range: TextRange,
        fully: &[usize],
        partial: &[usize],
        open: &mut Vec<usize>,
    ) {
        let mut boundaries = vec![range.start, range.end];
        for &index in partial {
            let scope = self.annotations[index].scope;
            boundaries.push(scope.start.clamp(range.start, range.end));
            boundaries.push(scope.end.clamp(range.start, range.end));
        }
        boundaries.sort_unstable();
        boundaries.dedup();
        for pair in boundaries.windows(2) {
            let (start, end) = (pair[0], pair[1]);
            if start >= end {
                continue;
            }
            let mut covered: Vec<usize> = fully.to_vec();
            for &index in partial {
                let scope = self.annotations[index].scope;
                if scope.start <= start && end <= scope.end && !covered.contains(&index) {
                    covered.push(index);
                }
            }
            covered.sort_unstable();
            if covered != *open {
                self.annotation_span_close(open);
                if !covered.is_empty() {
                    self.annotation_span_open(&covered);
                }
                *open = covered;
            }
            let value_start = (start - range.start).min(text.len());
            let value_end = (end - range.start).min(text.len());
            let fragment =
                &text[floor_char_boundary(text, value_start)..floor_char_boundary(text, value_end)];
            escape_text(&mut self.output, fragment);
        }
    }

    /// Returns the indices of the inline wrapper annotations covering an
    /// element of the current block: the annotation scope must fully contain
    /// the element range, and the annotation must not already wrap an ancestor
    /// element. Coverage is resolved at inline element granularity.
    fn inline_coverage(&self, node: &ElementNode) -> Vec<usize> {
        if !node.element.is_inline() {
            return Vec::new();
        }
        let Some(candidates) = self
            .current_block
            .and_then(|block| self.plan.inline_wrappers.get(&block))
        else {
            return Vec::new();
        };
        candidates
            .iter()
            .copied()
            .filter(|index| {
                !self.inherited_coverage.contains(index)
                    && contains(self.annotations[*index].scope, node.range)
            })
            .collect()
    }

    /// Opens a `<span class="notist-annotated">` fragment carrying the
    /// aggregated attributes of the covering annotations, using the same
    /// attribute rules as block projections.
    fn annotation_span_open(&mut self, coverage: &[usize]) {
        self.output.push_str("<span class=\"notist-annotated");
        for &index in coverage {
            for class in &self.annotations[index].classes {
                self.output.push(' ');
                escape_attribute(&mut self.output, class);
            }
        }
        self.output.push('"');
        let tags: Vec<&str> = coverage
            .iter()
            .flat_map(|&index| self.annotations[index].tags.iter())
            .map(String::as_str)
            .collect();
        if !tags.is_empty() {
            self.output.push_str(" data-notist-tag=\"");
            escape_attribute(&mut self.output, &tags.join(" "));
            self.output.push('"');
        }
        for &index in coverage {
            for (key, value) in &self.annotations[index].properties {
                self.output.push_str(" data-notist-");
                self.output.push_str(&property_attribute_key(key));
                self.output.push_str("=\"");
                escape_attribute(&mut self.output, value);
                self.output.push('"');
            }
        }
        self.output.push('>');
    }

    /// Closes the open annotation span of a content sequence, if any.
    fn annotation_span_close(&mut self, open: &mut Vec<usize>) {
        if !open.is_empty() {
            self.output.push_str("</span>");
            open.clear();
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

    fn list_item(&mut self, node: &ElementNode) {
        self.output.push_str("<li");
        if let Element::EnumItem {
            value: Some(value), ..
        } = &node.element
        {
            write!(self.output, " value=\"{value}\"").unwrap();
        }
        self.projected_class_attribute(node);
        self.range_attributes(node);
        self.output.push('>');
        match &node.element {
            Element::ListItem(body) | Element::EnumItem { body, .. } => self.flow_content(body),
            element => self.element(element, node, RenderPosition::Block),
        }
        self.output.push_str("</li>");
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
            // Parbreak does not render: paragraph structure is already given
            // by shaping (D0010). Reached only by direct callers; the normal
            // flow path skips it.
            Element::Parbreak => {}
            Element::Paragraph(body) => {
                self.output.push_str("<p");
                self.projected_class_attribute(node);
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
            Element::Underline(body) => {
                self.output.push_str("<u");
                self.range_attributes(node);
                self.output.push('>');
                self.inline_content(body);
                self.output.push_str("</u>");
            }
            Element::Heading { level, body } => {
                let level = (*level).clamp(1, 6);
                write!(self.output, "<h{level}").unwrap();
                self.projected_class_attribute(node);
                self.range_attributes(node);
                self.output.push('>');
                self.inline_content(body);
                write!(self.output, "</h{level}>").unwrap();
            }
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
                self.projected_class_attribute(node);
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
                self.projected_class_attribute(node);
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
                self.projected_class_attribute(node);
                self.range_attributes(node);
                self.output.push('>');
                self.flow_content(body);
                self.output.push_str("</li></ol>");
            }
            Element::Figure {
                body,
                kind,
                supplement,
                caption,
            } => {
                self.output.push_str("<figure class=\"notist-figure");
                self.projected_class_suffix(node);
                self.output.push_str("\" data-notist-kind=\"");
                escape_attribute(&mut self.output, kind);
                self.output.push('"');
                self.range_attributes(node);
                self.output.push('>');
                self.figure_body(body);
                if let Some(caption) = caption {
                    self.output.push_str("<figcaption>");
                    if let Some(supplement) = supplement {
                        self.inline_content(supplement);
                        escape_text(&mut self.output, ": ");
                    }
                    self.inline_content(caption);
                    self.output.push_str("</figcaption>");
                }
                self.output.push_str("</figure>");
            }
            Element::TableCell { body, .. } => {
                self.output.push_str("<div class=\"notist-table-cell");
                self.projected_class_suffix(node);
                self.output.push('"');
                self.range_attributes(node);
                self.output.push('>');
                self.flow_content(body);
                self.output.push_str("</div>");
            }
            Element::Table {
                columns,
                header,
                alignments,
                cells,
            } => {
                self.output.push_str("<div class=\"notist-table-wrapper");
                self.projected_class_suffix(node);
                write!(self.output, "\"><table data-notist-columns=\"{columns}\"").unwrap();
                self.range_attributes(node);
                self.output.push('>');
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
            Element::Rule => {
                self.output.push_str("<hr class=\"notist-rule\"");
                self.range_attributes(node);
                self.output.push('>');
            }
            Element::Callout { kind, title, body } => {
                self.output.push_str("<aside class=\"notist-callout");
                self.projected_class_suffix(node);
                self.output.push_str("\" data-notist-kind=\"");
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
                self.output.push_str("<details class=\"notist-details");
                self.projected_class_suffix(node);
                self.output.push('"');
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
            Element::Custom {
                name,
                body,
                block,
                fields,
            } => {
                let input = CustomRenderInput {
                    name,
                    body,
                    block: *block,
                    fields,
                };
                if self.renderers.render(&input, &mut self.output) {
                    return;
                }
                let tag = container_tag(*block, position);
                self.output.push('<');
                self.output.push_str(tag);
                self.output.push_str(" class=\"notist-custom");
                self.projected_class_suffix(node);
                self.output.push_str("\" data-notist-name=\"");
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
        let mut text = reference.module.to_string();
        if let Some(label) = &reference.label {
            text.push('#');
            text.push_str(label);
        }
        escape_text(&mut self.output, &text);
    }

    fn raw(&mut self, text: &str, block: bool, language: Option<&str>, node: &ElementNode) {
        if block {
            self.output.push_str("<pre");
            self.projected_class_attribute(node);
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
        self.output.push('<');
        self.output.push_str(tag);
        self.output.push_str(" class=\"notist-unresolved-call");
        self.projected_class_suffix(node);
        self.output.push_str("\" data-notist-name=\"");
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
        self.range_attributes_range(node.range);
    }

    fn range_attributes_range(&mut self, range: TextRange) {
        let key = range_key(range);
        if let Some(anchor) = self.plan.element_anchors.get(&key) {
            self.output.push_str(" id=\"");
            escape_attribute(&mut self.output, anchor);
            self.output.push('"');
        }
        if let Some(projection) = self.plan.projections.get(&key) {
            if !projection.tags.is_empty() {
                self.output.push_str(" data-notist-tag=\"");
                escape_attribute(&mut self.output, &projection.tags.join(" "));
                self.output.push('"');
            }
            for (key, value) in &projection.properties {
                self.output.push_str(" data-notist-");
                self.output.push_str(&property_attribute_key(key));
                self.output.push_str("=\"");
                escape_attribute(&mut self.output, value);
                self.output.push('"');
            }
        }
        write!(
            self.output,
            " data-notist-start=\"{}\" data-notist-end=\"{}\"",
            range.start, range.end
        )
        .unwrap();
    }

    /// Writes a complete `class` attribute holding the classes projected onto a
    /// block-level element that has no fixed class of its own.
    fn projected_class_attribute(&mut self, node: &ElementNode) {
        self.projected_class_attribute_range(node.range);
    }

    fn projected_class_attribute_range(&mut self, range: TextRange) {
        let Some(projection) = self.plan.projections.get(&range_key(range)) else {
            return;
        };
        if projection.classes.is_empty() {
            return;
        }
        self.output.push_str(" class=\"");
        escape_attribute(&mut self.output, &projection.classes.join(" "));
        self.output.push('"');
    }

    /// Appends the classes projected onto an element to a fixed `class`
    /// attribute value that the render site is already writing.
    fn projected_class_suffix(&mut self, node: &ElementNode) {
        let Some(projection) = self.plan.projections.get(&range_key(node.range)) else {
            return;
        };
        for class in &projection.classes {
            self.output.push(' ');
            escape_attribute(&mut self.output, class);
        }
    }
}

/// Precomputed anchor, projection, and inline-wrapper assignments for one
/// rendered document.
///
/// The renderer emits every anchor from this plan, so the fragment and
/// [`module_anchors`] always agree on the label-to-anchor mapping.
struct AnchorPlan {
    /// Resolvable labels in registration order: explicit scope ids first, then
    /// heading default ids. Each label appears once; the first occurrence wins.
    labels: Vec<(String, String)>,
    /// Assigned HTML anchors keyed by element range.
    element_anchors: HashMap<(usize, usize), String>,
    /// Annotation attributes projected onto fully covered top-level blocks,
    /// keyed by block range.
    projections: HashMap<(usize, usize), Projection>,
    /// Indices of annotations partially overlapping a top-level block, keyed by
    /// block range. The renderer wraps the fully covered inline elements of
    /// such a block into `<span class="notist-annotated">` fragments.
    inline_wrappers: HashMap<(usize, usize), Vec<usize>>,
}

/// Annotation attributes projected onto a fully covered block element.
#[derive(Default)]
struct Projection {
    classes: Vec<String>,
    tags: Vec<String>,
    properties: Vec<(String, String)>,
}

/// One document element seen by the anchor-planning walk.
struct WalkedElement {
    range: TextRange,
    /// The plain text of the heading body, for heading elements.
    heading_text: Option<String>,
}

impl AnchorPlan {
    fn compute(document: &StructuredDocument, annotations: &[RenderedAnnotation]) -> Self {
        let mut elements = Vec::new();
        for block in &document.blocks {
            walk_block(block, &mut elements);
        }

        // Id claiming keeps the historical rule: the first element in document
        // order whose range falls inside the annotation scope receives the id,
        // and each id is emitted at most once.
        let mut claimed_ids = HashSet::new();
        let mut explicit_ids: Vec<((usize, usize), &str)> = Vec::new();
        for element in &elements {
            let key = range_key(element.range);
            if let Some(annotation) = annotations.iter().find(|annotation| {
                annotation.id.is_some() && contains(annotation.scope, element.range)
            }) {
                let id = annotation.id.as_deref().expect("id presence checked above");
                if claimed_ids.insert(id) {
                    explicit_ids.push((key, id));
                }
            }
        }

        // Class/tag/property projection is classified per annotation against
        // the top-level blocks:
        //
        // - A block fully contained in the annotation scope receives the
        //   projection on its own tag; a scope covering several blocks
        //   projects onto every covered block.
        // - A block partially overlapped by the scope cannot carry the
        //   attributes on its tag. The annotation is registered as an inline
        //   wrapper candidate for the block instead, and the renderer wraps
        //   the fully covered inline elements of the block into
        //   `<span class="notist-annotated">` fragments (see
        //   `Renderer::inline_coverage`).
        //
        // Annotations matching neither case produce no output.
        let mut projections: HashMap<(usize, usize), Projection> = HashMap::new();
        let mut inline_wrappers: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
        for (annotation_index, annotation) in annotations.iter().enumerate() {
            if !has_projection(annotation) {
                continue;
            }
            for block in &document.blocks {
                // D0010: a fully covered section receives the projection on
                // its <section> node; partially covered blocks fall back to
                // inline wrapping at their leaves.
                if let Some(key) = projection_target(block, annotation.scope) {
                    let projection = projections.entry(key).or_default();
                    projection
                        .classes
                        .extend(annotation.classes.iter().cloned());
                    projection.tags.extend(annotation.tags.iter().cloned());
                    projection
                        .properties
                        .extend(annotation.properties.iter().cloned());
                } else if intersects(annotation.scope, block.range()) {
                    register_inline_wrappers(
                        block,
                        annotation.scope,
                        annotation_index,
                        &mut inline_wrappers,
                    );
                }
            }
        }

        let mut used_anchors = HashSet::new();
        let mut seen_labels = HashSet::new();
        let mut labels = Vec::new();
        let mut element_anchors = HashMap::new();
        // Pass 1: explicit scope ids in document order.
        for (key, id) in explicit_ids {
            assign_anchor(
                id,
                key,
                &mut used_anchors,
                &mut seen_labels,
                &mut labels,
                &mut element_anchors,
            );
        }
        // Pass 2: heading default ids (heading plain text) in document order.
        // A heading that already carries an explicit id keeps it: the explicit
        // id always overrides the default text id.
        for element in &elements {
            let Some(text) = element.heading_text.as_deref() else {
                continue;
            };
            let key = range_key(element.range);
            if element_anchors.contains_key(&key) {
                continue;
            }
            assign_anchor(
                text,
                key,
                &mut used_anchors,
                &mut seen_labels,
                &mut labels,
                &mut element_anchors,
            );
        }

        Self {
            labels,
            element_anchors,
            projections,
            inline_wrappers,
        }
    }
    fn compute_tree(tree: &ElementTree, annotations: &[RenderedAnnotation]) -> Self {
        let mut elements = Vec::new();
        for root in &tree.roots {
            walk_tree_node(root, &mut elements);
        }

        let mut claimed_ids = HashSet::new();
        let mut explicit_ids: Vec<((usize, usize), &str)> = Vec::new();
        for element in &elements {
            let key = range_key(element.range);
            if let Some(annotation) = annotations.iter().find(|annotation| {
                annotation.id.is_some() && contains(annotation.scope, element.range)
            }) {
                let id = annotation.id.as_deref().expect("id presence checked above");
                if claimed_ids.insert(id) {
                    explicit_ids.push((key, id));
                }
            }
        }

        let mut projections: HashMap<(usize, usize), Projection> = HashMap::new();
        let mut inline_wrappers: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
        for (annotation_index, annotation) in annotations.iter().enumerate() {
            if !has_projection(annotation) {
                continue;
            }
            for root in &tree.roots {
                if let Some(key) = tree_projection_target(root, annotation.scope) {
                    let projection = projections.entry(key).or_default();
                    projection
                        .classes
                        .extend(annotation.classes.iter().cloned());
                    projection.tags.extend(annotation.tags.iter().cloned());
                    projection
                        .properties
                        .extend(annotation.properties.iter().cloned());
                } else if intersects(annotation.scope, root.range) {
                    tree_register_inline_wrappers(
                        root,
                        annotation.scope,
                        annotation_index,
                        &mut inline_wrappers,
                    );
                }
            }
        }

        let mut used_anchors = HashSet::new();
        let mut seen_labels = HashSet::new();
        let mut labels = Vec::new();
        let mut element_anchors = HashMap::new();
        for (key, id) in explicit_ids {
            assign_anchor(
                id,
                key,
                &mut used_anchors,
                &mut seen_labels,
                &mut labels,
                &mut element_anchors,
            );
        }
        for element in &elements {
            let Some(text) = element.heading_text.as_deref() else {
                continue;
            };
            let key = range_key(element.range);
            if element_anchors.contains_key(&key) {
                continue;
            }
            assign_anchor(
                text,
                key,
                &mut used_anchors,
                &mut seen_labels,
                &mut labels,
                &mut element_anchors,
            );
        }

        Self {
            labels,
            element_anchors,
            projections,
            inline_wrappers,
        }
    }
}

/// Assigns the HTML anchor for one label/element pair, deduplicating anchors
/// within the module and falling back to `loc-<byte offset>` anchors that
/// are a pure function of the element's source start position (E08).
fn assign_anchor(
    label: &str,
    key: (usize, usize),
    used_anchors: &mut HashSet<String>,
    seen_labels: &mut HashSet<String>,
    labels: &mut Vec<(String, String)>,
    element_anchors: &mut HashMap<(usize, usize), String>,
) {
    // The fallback anchor is a pure function of the element's source start
    // byte offset: deterministic and mutually derivable with the source
    // position, independent of document-order history (E08).
    let anchor = if is_valid_anchor(label) && !used_anchors.contains(label) {
        label.to_owned()
    } else {
        format!("loc-{}", key.0)
    };
    used_anchors.insert(anchor.clone());
    element_anchors.insert(key, anchor.clone());
    if seen_labels.insert(label.to_owned()) {
        labels.push((label.to_owned(), anchor));
    }
}

/// Returns whether a label can be used directly as an HTML id anchor: non-empty,
/// starting with a Unicode letter or `_`, followed by Unicode letters, digits,
/// hyphens, or underscores.
fn is_valid_anchor(label: &str) -> bool {
    let mut characters = label.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    (first.is_alphabetic() || first == '_')
        && characters.all(|character| character.is_alphanumeric() || matches!(character, '-' | '_'))
}

fn table_alignment_class(alignment: TableAlignment) -> Option<&'static str> {
    match alignment {
        TableAlignment::Default => None,
        TableAlignment::Left => Some("notist-table-align-left"),
        TableAlignment::Center => Some("notist-table-align-center"),
        TableAlignment::Right => Some("notist-table-align-right"),
    }
}

fn range_key(range: TextRange) -> (usize, usize) {
    (range.start, range.end)
}

fn contains(scope: TextRange, range: TextRange) -> bool {
    scope.start <= range.start && range.end <= scope.end
}

/// Returns whether two ranges share at least one byte.
fn intersects(a: TextRange, b: TextRange) -> bool {
    a.start < b.end && b.start < a.end
}

fn has_projection(annotation: &RenderedAnnotation) -> bool {
    !annotation.classes.is_empty()
        || !annotation.tags.is_empty()
        || !annotation.properties.is_empty()
}

/// Maps an annotation property key onto a `data-notist-*` attribute suffix:
/// every character outside `[a-z0-9-]` becomes a hyphen.
fn property_attribute_key(key: &str) -> String {
    key.chars()
        .map(|character| {
            if character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect()
}

/// Collects every element of the document in render (depth-first) order.
/// Walks a structured block (recursing through sections) for anchor and
/// projection planning (D0010).
fn walk_block(block: &Block, output: &mut Vec<WalkedElement>) {
    match block {
        Block::Element(node) => walk_element(node, output),
        Block::Section { heading, body, .. } => {
            walk_element(heading, output);
            for child in body {
                walk_block(child, output);
            }
        }
    }
}

/// Returns the range key of the smallest block or section fully contained in
/// `scope`, preferring the section itself when it is covered (D0010: section
/// entries project onto the Section node).
fn projection_target(block: &Block, scope: TextRange) -> Option<(usize, usize)> {
    match block {
        Block::Element(node) => contains(scope, node.range).then(|| range_key(node.range)),
        Block::Section { body, .. } => {
            if contains(scope, block.range()) {
                return Some(range_key(block.range()));
            }
            for child in body {
                if let Some(key) = projection_target(child, scope) {
                    return Some(key);
                }
            }
            None
        }
    }
}

/// Registers inline wrapper candidates for every leaf element intersecting a
/// partially covering annotation scope.
fn register_inline_wrappers(
    block: &Block,
    scope: TextRange,
    annotation_index: usize,
    wrappers: &mut HashMap<(usize, usize), Vec<usize>>,
) {
    match block {
        Block::Element(node) => {
            wrappers
                .entry(range_key(node.range))
                .or_default()
                .push(annotation_index);
        }
        Block::Section { heading, body, .. } => {
            if intersects(scope, heading.range) {
                wrappers
                    .entry(range_key(heading.range))
                    .or_default()
                    .push(annotation_index);
            }
            for child in body {
                if intersects(scope, child.range()) {
                    register_inline_wrappers(child, scope, annotation_index, wrappers);
                }
            }
        }
    }
}

fn tree_projection_target(node: &InstanceNode, scope: TextRange) -> Option<(usize, usize)> {
    if node.instance.is_core("section") {
        if contains(scope, node.range) {
            return Some(range_key(node.range));
        }
        // The section heading is an inline-wrapper target in the legacy block
        // model, not a projection child block; skip it here exactly like
        // `projection_target(Block::Section)`.
        for child in &node.instance.body[1..] {
            if let Some(key) = tree_projection_target(child, scope) {
                return Some(key);
            }
        }
        return None;
    }
    contains(scope, node.range).then(|| range_key(node.range))
}

fn tree_register_inline_wrappers(
    node: &InstanceNode,
    scope: TextRange,
    annotation_index: usize,
    wrappers: &mut HashMap<(usize, usize), Vec<usize>>,
) {
    if node.instance.is_core("section") {
        if !intersects(scope, node.range) {
            return;
        }
        for child in &node.instance.body {
            if intersects(scope, child.range) {
                tree_register_inline_wrappers(child, scope, annotation_index, wrappers);
            }
        }
        return;
    }
    if intersects(scope, node.range) {
        wrappers
            .entry(range_key(node.range))
            .or_default()
            .push(annotation_index);
    }
}

fn tree_heading_text(node: &InstanceNode) -> Option<String> {
    if !node.instance.is_core("heading") {
        return None;
    }
    Some(content_plain_text(
        &instances_to_legacy_content(&node.instance.body).unwrap_or_default(),
    ))
}

fn walk_tree_node(node: &InstanceNode, output: &mut Vec<WalkedElement>) {
    output.push(WalkedElement {
        range: node.range,
        heading_text: tree_heading_text(node),
    });
    let Some(local) = node.instance.name.core_local() else {
        walk_tree_nodes(&node.instance.body, output);
        return;
    };
    match local {
        "paragraph" | "strong" | "emph" | "strike" | "underline" | "heading" | "item"
        | "custom" | "unresolved-call" | "section" => walk_tree_nodes(&node.instance.body, output),
        "callout" => {
            if let Some(FieldValue::Content(nodes)) = node.instance.field("title") {
                walk_tree_nodes(nodes, output);
            }
            walk_tree_nodes(&node.instance.body, output);
        }
        "details" => {
            if let Some(FieldValue::Content(nodes)) = node.instance.field("summary") {
                walk_tree_nodes(nodes, output);
            }
            walk_tree_nodes(&node.instance.body, output);
        }
        _ => {}
    }
}

fn walk_tree_nodes(nodes: &[InstanceNode], output: &mut Vec<WalkedElement>) {
    for node in nodes {
        walk_tree_node(node, output);
    }
}

fn collect_outline_entries_tree(tree: &ElementTree, plan: &AnchorPlan) -> Vec<RenderedHeading> {
    fn heading_record(node: &InstanceNode, plan: &AnchorPlan) -> Option<RenderedHeading> {
        if !node.instance.is_core("heading") {
            return None;
        }
        let level = match node.instance.field("level")? {
            FieldValue::Int(level) => u8::try_from(*level).ok()?,
            _ => return None,
        };
        Some(RenderedHeading {
            level,
            id: plan
                .element_anchors
                .get(&range_key(node.range))
                .expect("headings always receive an anchor")
                .clone(),
            text: tree_heading_text(node)?,
        })
    }

    fn walk(nodes: &[InstanceNode], plan: &AnchorPlan, output: &mut Vec<RenderedHeading>) {
        for node in nodes {
            if node.instance.is_core("section") {
                if let Some(heading) = node.instance.body.first()
                    && let Some(record) = heading_record(heading, plan)
                {
                    output.push(record);
                }
                walk(&node.instance.body[1..], plan, output);
            } else if let Some(record) = heading_record(node, plan) {
                output.push(record);
            }
        }
    }

    let mut output = Vec::new();
    walk(&tree.roots, plan, &mut output);
    output
}

fn walk_element(node: &ElementNode, output: &mut Vec<WalkedElement>) {
    let heading_text = match &node.element {
        Element::Heading { body, .. } => Some(content_plain_text(body)),
        _ => None,
    };
    output.push(WalkedElement {
        range: node.range,
        heading_text,
    });
    walk_element_children(&node.element, output);
}

fn walk_content(content: &Content, output: &mut Vec<WalkedElement>) {
    for node in &content.elements {
        walk_element(node, output);
    }
}

fn walk_element_children(element: &Element, output: &mut Vec<WalkedElement>) {
    match element {
        Element::Paragraph(body)
        | Element::Strong(body)
        | Element::Emph(body)
        | Element::Strike(body)
        | Element::Underline(body)
        | Element::Heading { body, .. }
        | Element::ListItem(body)
        | Element::EnumItem { body, .. }
        | Element::Custom { body, .. } => walk_content(body, output),
        Element::Callout { title, body, .. } => {
            if let Some(title) = title {
                walk_content(title, output);
            }
            walk_content(body, output);
        }
        Element::Details { summary, body, .. } => {
            if let Some(summary) = summary {
                walk_content(summary, output);
            }
            walk_content(body, output);
        }
        Element::UnresolvedCall {
            trailing: Some(trailing),
            ..
        } => walk_content(trailing, output),
        _ => {}
    }
}

fn collect_outline_entries(
    document: &StructuredDocument,
    plan: &AnchorPlan,
) -> Vec<RenderedHeading> {
    fn walk(blocks: &[Block], plan: &AnchorPlan, output: &mut Vec<RenderedHeading>) {
        for block in blocks {
            match block {
                Block::Element(node) => {
                    if let Element::Heading { level, body } = &node.element {
                        output.push(RenderedHeading {
                            level: *level,
                            id: plan
                                .element_anchors
                                .get(&range_key(node.range))
                                .expect("headings always receive an anchor")
                                .clone(),
                            text: content_plain_text(body),
                        });
                    }
                }
                Block::Section { heading, body, .. } => {
                    if let Element::Heading {
                        level,
                        body: heading_body,
                    } = &heading.element
                    {
                        output.push(RenderedHeading {
                            level: *level,
                            id: plan
                                .element_anchors
                                .get(&range_key(heading.range))
                                .expect("headings always receive an anchor")
                                .clone(),
                            text: content_plain_text(heading_body),
                        });
                    }
                    walk(body, plan, output);
                }
            }
        }
    }
    let mut output = Vec::new();
    walk(&document.blocks, plan, &mut output);
    output
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
            | Element::Underline(body) => content_plain_text(body),
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

/// Rounds a byte offset down to the nearest UTF-8 character boundary.
fn floor_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
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
    fn renders_canonical_element_tree_entry_point() {
        let evaluation = Evaluator::default().evaluate_stream("= Title\n\nBody");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render_element_tree(&evaluation.tree);
        assert!(html.starts_with("<section "), "{html}");
        assert!(html.contains("<h1 id=\"Title\""), "{html}");
        assert!(html.contains("Body</span>"), "{html}");
    }

    #[test]
    fn renders_evaluated_document_structure() {
        let evaluation = Evaluator::default().evaluate(
            "#heading(level=2)[Title]\n\nBefore after\n\n#details[First\n\nSecond]\n\n#raw(r#\"\"\"\nfn main() {}\n\"\"\"#, lang=\"rust\", block=true)",
        );
        let structured = structure(evaluation);

        let html = render(&structured.document);

        // D0010: the heading and its content form a nested <section>.
        assert!(html.starts_with("<section data-notist-start=\"0\""));
        assert!(html.contains("<h2 id=\"Title\" data-notist-start=\"0\""));
        assert!(html.contains("<p><span class=\"notist-text\""));
        assert!(html.contains("<details"));
        assert!(html.contains(">First</span></p><p>"));
        assert!(html.contains("<pre"));
        assert!(html.contains("<code class=\"language-rust\">fn main() {}</code></pre>"));
    }

    #[test]
    fn renders_manifest_web_component_with_scalar_fields() {
        let document = StructuredDocument {
            blocks: vec![Block::Element(node(
                Element::Custom {
                    name: "card::card".into(),
                    body: Content::single(Element::Text("fallback".into()), TextRange::new(2, 10)),
                    block: true,
                    fields: vec![
                        CustomField {
                            name: "title".into(),
                            value: notist_model::ElementValue::String("Hello & welcome".into()),
                        },
                        CustomField {
                            name: "count".into(),
                            value: notist_model::ElementValue::Int(3),
                        },
                    ],
                },
                0,
                5,
            ))],
        };
        let mut renderers = HtmlRendererRegistry::new();
        register_web_component_renderer(&mut renderers, "card", "notist-card");
        let html = render_with_renderers(
            &document,
            &RenderOptions::default(),
            &|_, _| None,
            &[],
            &renderers,
        );
        assert!(html.contains("<notist-card"));
        assert!(html.contains("data-notist-element=\"card::card\""));
        assert!(html.contains("data-title=\"Hello &amp; welcome\""));
        assert!(html.contains("data-count=\"3\""));
        assert!(html.contains("<p>fallback</p>"));
        assert!(!html.contains("<script"));
    }

    #[test]
    fn renders_shader_plugin_as_webgpu_canvas() {
        let document = StructuredDocument {
            blocks: vec![Block::Element(node(
                Element::Custom {
                    name: "shader::shader".into(),
                    body: Content::single(
                        Element::Text("fallback".into()),
                        TextRange::new(2, 10),
                    ),
                    block: true,
                    fields: vec![
                        CustomField {
                            name: "source".into(),
                            value: notist_model::ElementValue::String(
                                "fn mainImage(fragCoord: vec2<f32>) -> vec4<f32> { return vec4<f32>(fragCoord, 0.0, 1.0); }".into(),
                            ),
                        },
                        CustomField {
                            name: "width".into(),
                            value: notist_model::ElementValue::Int(320),
                        },
                        CustomField {
                            name: "height".into(),
                            value: notist_model::ElementValue::Int(200),
                        },
                    ],
                },
                0,
                5,
            ))],
        };

        let html = render(&document);
        assert!(html.contains("<notist-shader"));
        assert!(html.contains("class=\"notist-shader\""));
        assert!(html.contains("data-shader-source="));
        assert!(html.contains("data-width=\"320\""));
        assert!(html.contains("data-height=\"200\""));
        assert!(html.contains("</notist-shader>"));
        assert!(!html.contains("<script"));
    }

    #[test]
    fn partially_covered_text_nodes_split_into_wrapped_fragments() {
        // D0010: a text node straddling an annotation boundary is split; only
        // the covered fragment is wrapped.
        let evaluation = Evaluator::default().evaluate("abcdef");
        let structured = structure(evaluation);
        let annotations = vec![RenderedAnnotation {
            scope: TextRange::new(2, 4),
            id: None,
            classes: vec!["mark".into()],
            tags: Vec::new(),
            properties: Vec::new(),
        }];
        let html = render_with_resolvers(
            &structured.document,
            &RenderOptions::default(),
            &|_: &ModulePath, _: Option<&str>| None,
            &annotations,
        );
        assert!(html.contains("ab<span class=\"notist-annotated mark\""));
        assert!(html.contains("cd</span>ef"));
    }

    #[test]
    fn sections_nest_and_receive_section_level_projection() {
        let evaluation =
            Evaluator::default().evaluate("= 一级\n\n段落\n\n== 二级\n\n内文\n\n= 一级二\n");
        let structured = structure(evaluation);
        let html = render(&structured.document);
        // Two sibling top-level sections; the second-level heading nests
        // inside the first section.
        assert_eq!(html.matches("<section data-notist-start").count(), 3);
        assert!(html.contains("<h1 id=\"一级\""));
        assert!(html.contains("<h2 id=\"二级\""));
        // Section-level annotations project onto the covering <section> node.
        let annotations = vec![RenderedAnnotation {
            scope: TextRange::new(0, 60),
            id: None,
            classes: vec!["wip".into()],
            tags: vec!["draft".into()],
            properties: vec![("status".into(), "draft".into())],
        }];
        let html = render_with_resolvers(
            &structured.document,
            &RenderOptions::default(),
            &|_: &ModulePath, _: Option<&str>| None,
            &annotations,
        );
        assert!(html.contains("<section class=\"wip\""));
        assert!(html.contains("data-notist-tag=\"draft\""));
        assert!(html.contains("data-notist-status=\"draft\""));
    }

    #[test]
    fn renders_plain_paragraph_element() {
        let evaluator = Evaluator::default();
        let html = render(&structure(evaluator.evaluate("plain paragraph")).document);
        assert!(html.starts_with("<p data-notist-start=\"0\""));
        assert!(html.ends_with("</p>"));
    }

    #[test]
    fn escapes_text_attributes_and_raw_bodies() {
        let document = StructuredDocument {
            blocks: vec![Block::Element(node(
                Element::Custom {
                    name: "x\" onclick=\"bad".into(),
                    body: Content::single(Element::Text("<&>".into()), TextRange::new(2, 5)),
                    block: true,
                    fields: Vec::new(),
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
    fn uses_heading_text_as_the_default_anchor() {
        let evaluation = Evaluator::default().evaluate("= 简介\n\n正文");
        let structured = structure(evaluation);

        let html = render(&structured.document);

        assert!(html.contains("<h1 id=\"简介\""));
        assert_eq!(
            module_anchors(&structured.document, &[]),
            vec![("简介".to_owned(), "简介".to_owned())]
        );
    }

    #[test]
    fn falls_back_to_loc_anchors_for_invalid_heading_text() {
        let evaluation = Evaluator::default().evaluate("#heading[1st steps]");
        let structured = structure(evaluation);

        let html = render(&structured.document);

        assert!(html.contains("<h1 id=\"loc-0\""));
        assert_eq!(
            module_anchors(&structured.document, &[]),
            vec![("1st steps".to_owned(), "loc-0".to_owned())]
        );
    }

    #[test]
    fn deduplicates_repeated_heading_anchors() {
        let evaluation = Evaluator::default().evaluate("= Intro\n\n= Intro");
        let structured = structure(evaluation);

        let html = render(&structured.document);

        assert!(html.contains("<h1 id=\"Intro\""));
        assert!(html.contains("<h1 id=\"loc-9\""));
        assert_eq!(
            module_anchors(&structured.document, &[]),
            vec![("Intro".to_owned(), "Intro".to_owned())]
        );
    }

    #[test]
    fn explicit_ids_override_heading_text_anchors() {
        let document = StructuredDocument {
            blocks: vec![
                Block::Element(node(
                    Element::Heading {
                        level: 1,
                        body: Content::single(Element::Text("Intro".into()), TextRange::new(1, 6)),
                    },
                    0,
                    7,
                )),
                Block::Element(node(
                    Element::Paragraph(Content::single(
                        Element::Text("quoted".into()),
                        TextRange::new(18, 24),
                    )),
                    8,
                    25,
                )),
            ],
        };
        let annotations = vec![RenderedAnnotation {
            scope: TextRange::new(8, 25),
            id: Some("Intro".into()),
            classes: Vec::new(),
            tags: Vec::new(),
            properties: Vec::new(),
        }];

        let anchors = module_anchors(&document, &annotations);
        assert_eq!(anchors, vec![("Intro".to_owned(), "Intro".to_owned())]);

        let html = render_with_resolvers(
            &document,
            &RenderOptions::default(),
            &|_: &ModulePath, _: Option<&str>| None,
            &annotations,
        );
        assert!(html.contains("<h1 id=\"loc-0\""));
        assert!(html.contains("<p id=\"Intro\""));
    }

    #[test]
    fn projects_annotation_attributes_onto_block_elements() {
        let document = StructuredDocument {
            blocks: vec![Block::Element(node(
                Element::Paragraph(Content::single(
                    Element::Text("quoted".into()),
                    TextRange::new(8, 14),
                )),
                0,
                15,
            ))],
        };
        let annotations = vec![RenderedAnnotation {
            scope: TextRange::new(0, 15),
            id: Some("quote-id".into()),
            classes: vec!["hero".into()],
            tags: vec!["design".into(), "wip".into()],
            properties: vec![
                ("status".into(), "draft".into()),
                ("Reviewed".into(), "yes & no".into()),
            ],
        }];

        let html = render_with_resolvers(
            &document,
            &RenderOptions::default(),
            &|_: &ModulePath, _: Option<&str>| None,
            &annotations,
        );

        assert!(html.contains("<p class=\"hero\" id=\"quote-id\""));
        assert!(html.contains("data-notist-tag=\"design wip\""));
        assert!(html.contains("data-notist-status=\"draft\""));
        // Characters outside [a-z0-9-] become hyphens in attribute keys.
        assert!(html.contains("data-notist--eviewed=\"yes &amp; no\""));
    }

    #[test]
    fn merges_projected_classes_into_fixed_class_attributes() {
        let document = StructuredDocument {
            blocks: vec![Block::Element(node(
                Element::Callout {
                    kind: "tip".into(),
                    title: None,
                    body: Content::single(Element::Text("hint".into()), TextRange::new(5, 9)),
                },
                0,
                10,
            ))],
        };
        let annotations = vec![RenderedAnnotation {
            scope: TextRange::new(0, 10),
            id: None,
            classes: vec!["hero".into()],
            tags: Vec::new(),
            properties: Vec::new(),
        }];

        let html = render_with_resolvers(
            &document,
            &RenderOptions::default(),
            &|_: &ModulePath, _: Option<&str>| None,
            &annotations,
        );

        assert!(html.contains("<aside class=\"notist-callout hero\""));
    }

    #[test]
    fn assigns_explicit_ids_to_inline_elements() {
        let document = StructuredDocument {
            blocks: vec![Block::Element(node(
                Element::Paragraph(Content::single(
                    Element::Strong(Content::single(
                        Element::Text("bold".into()),
                        TextRange::new(9, 13),
                    )),
                    TextRange::new(2, 14),
                )),
                0,
                20,
            ))],
        };
        let annotations = vec![RenderedAnnotation {
            scope: TextRange::new(2, 14),
            id: Some("bold".into()),
            classes: Vec::new(),
            tags: Vec::new(),
            properties: Vec::new(),
        }];

        let html = render_with_resolvers(
            &document,
            &RenderOptions::default(),
            &|_: &ModulePath, _: Option<&str>| None,
            &annotations,
        );

        assert!(html.contains("<strong id=\"bold\""));
    }

    #[test]
    fn projects_multi_block_scopes_onto_every_covered_block() {
        let document = StructuredDocument {
            blocks: vec![
                Block::Element(node(
                    Element::Paragraph(Content::single(
                        Element::Text("first".into()),
                        TextRange::new(1, 6),
                    )),
                    0,
                    10,
                )),
                Block::Element(node(
                    Element::Paragraph(Content::single(
                        Element::Text("second".into()),
                        TextRange::new(12, 18),
                    )),
                    11,
                    20,
                )),
            ],
        };
        let annotations = vec![RenderedAnnotation {
            scope: TextRange::new(0, 20),
            id: Some("multi".into()),
            classes: vec!["hero".into()],
            tags: vec!["design".into()],
            properties: vec![("status".into(), "draft".into())],
        }];

        let html = render_with_resolvers(
            &document,
            &RenderOptions::default(),
            &|_: &ModulePath, _: Option<&str>| None,
            &annotations,
        );

        assert!(html.contains(
            "<p class=\"hero\" id=\"multi\" data-notist-tag=\"design\" data-notist-status=\"draft\" data-notist-start=\"0\""
        ));
        assert!(html.contains(
            "<p class=\"hero\" data-notist-tag=\"design\" data-notist-status=\"draft\" data-notist-start=\"11\""
        ));
        // The id anchor is unique: only the first covered block carries it.
        assert_eq!(html.matches("id=\"multi\"").count(), 1);
        // Fully covered blocks are projected, never span-wrapped.
        assert!(!html.contains("notist-annotated"));
    }

    #[test]
    fn wraps_partially_covered_inline_elements_in_annotation_spans() {
        let document = StructuredDocument {
            blocks: vec![Block::Element(node(
                Element::Paragraph(Content {
                    elements: vec![
                        node(Element::Text("before ".into()), 0, 7),
                        node(
                            Element::Strong(Content::single(
                                Element::Text("bold".into()),
                                TextRange::new(9, 13),
                            )),
                            7,
                            14,
                        ),
                        node(Element::Text(" after".into()), 14, 20),
                    ],
                }),
                0,
                20,
            ))],
        };
        let annotations = vec![RenderedAnnotation {
            scope: TextRange::new(7, 25),
            id: Some("bold".into()),
            classes: vec!["hero".into()],
            tags: vec!["design".into()],
            properties: vec![("status".into(), "draft".into())],
        }];

        let html = render_with_resolvers(
            &document,
            &RenderOptions::default(),
            &|_: &ModulePath, _: Option<&str>| None,
            &annotations,
        );

        // Adjacent inline elements covered by the same annotation merge into
        // one span; the id anchor still lands on the first covered element.
        assert!(html.contains(
            "<span class=\"notist-annotated hero\" data-notist-tag=\"design\" data-notist-status=\"draft\"><strong id=\"bold\""
        ));
        // The uncovered prefix stays outside the span.
        assert!(html.contains("before </span><span class=\"notist-annotated"));
        // The span closes before the paragraph tag.
        assert!(html.contains(" after</span></span></p>"));
        assert_eq!(html.matches("notist-annotated").count(), 1);
    }

    #[test]
    fn aggregates_annotations_covering_the_same_inline_run() {
        let document = StructuredDocument {
            blocks: vec![Block::Element(node(
                Element::Paragraph(Content {
                    elements: vec![
                        node(Element::Text("a".into()), 0, 1),
                        node(Element::Text("b".into()), 1, 2),
                    ],
                }),
                0,
                20,
            ))],
        };
        let annotations = vec![
            RenderedAnnotation {
                scope: TextRange::new(0, 5),
                id: None,
                classes: vec!["one".into()],
                tags: vec!["x".into()],
                properties: Vec::new(),
            },
            RenderedAnnotation {
                scope: TextRange::new(1, 5),
                id: None,
                classes: vec!["two".into()],
                tags: Vec::new(),
                properties: vec![("k".into(), "v".into())],
            },
        ];

        let html = render_with_resolvers(
            &document,
            &RenderOptions::default(),
            &|_: &ModulePath, _: Option<&str>| None,
            &annotations,
        );

        // The coverage set changes between the two texts, so the run splits
        // into two spans; the second aggregates both annotations.
        assert!(html.contains("<span class=\"notist-annotated one\" data-notist-tag=\"x\">"));
        assert!(html.contains(
            "</span><span class=\"notist-annotated one two\" data-notist-tag=\"x\" data-notist-k=\"v\">"
        ));
        assert_eq!(html.matches("notist-annotated").count(), 2);
    }

    #[test]
    fn wraps_inline_runs_inside_flow_content_paragraphs() {
        let document = StructuredDocument {
            blocks: vec![Block::Element(node(
                Element::Callout {
                    kind: "tip".into(),
                    title: None,
                    body: Content {
                        elements: vec![
                            node(Element::Text("a".into()), 5, 6),
                            node(Element::Text("b".into()), 7, 8),
                        ],
                    },
                },
                0,
                40,
            ))],
        };
        let annotations = vec![RenderedAnnotation {
            scope: TextRange::new(0, 20),
            id: None,
            classes: vec!["hero".into()],
            tags: Vec::new(),
            properties: Vec::new(),
        }];

        let html = render_with_resolvers(
            &document,
            &RenderOptions::default(),
            &|_: &ModulePath, _: Option<&str>| None,
            &annotations,
        );

        // A partially covered block carries no projection on its own tag...
        assert!(html.contains(
            "<aside class=\"notist-callout\" data-notist-kind=\"tip\" data-notist-start=\"0\""
        ));
        // ...while its automatically grouped paragraph wraps the covered
        // inline run, closing the span before the paragraph tag.
        assert!(html.contains("<p><span class=\"notist-annotated hero\">"));
        assert!(html.contains("</span></p></aside>"));
    }

    #[test]
    fn combines_block_projection_with_inline_wrapping_for_mixed_scopes() {
        let document = StructuredDocument {
            blocks: vec![
                Block::Element(node(
                    Element::Paragraph(Content::single(
                        Element::Text("first".into()),
                        TextRange::new(1, 6),
                    )),
                    0,
                    10,
                )),
                Block::Element(node(
                    Element::Paragraph(Content::single(
                        Element::Text("second".into()),
                        TextRange::new(12, 18),
                    )),
                    11,
                    20,
                )),
                Block::Element(node(
                    Element::Paragraph(Content {
                        elements: vec![
                            node(Element::Text("x".into()), 22, 23),
                            node(Element::Text("y".into()), 35, 36),
                        ],
                    }),
                    21,
                    40,
                )),
            ],
        };
        let annotations = vec![RenderedAnnotation {
            scope: TextRange::new(0, 30),
            id: Some("mix".into()),
            classes: vec!["hero".into()],
            tags: vec!["t".into()],
            properties: Vec::new(),
        }];

        let html = render_with_resolvers(
            &document,
            &RenderOptions::default(),
            &|_: &ModulePath, _: Option<&str>| None,
            &annotations,
        );

        // The first two blocks are fully covered: projection on their tags.
        assert!(html.contains(
            "<p class=\"hero\" id=\"mix\" data-notist-tag=\"t\" data-notist-start=\"0\""
        ));
        assert!(html.contains("<p class=\"hero\" data-notist-tag=\"t\" data-notist-start=\"11\""));
        // The third block is partially covered: no projection on its tag, and
        // only the fully covered inline element is wrapped.
        assert!(html.contains(
            "<p data-notist-start=\"21\" data-notist-end=\"40\"><span class=\"notist-annotated hero\" data-notist-tag=\"t\">"
        ));
        assert!(html.contains(">x</span></span><span class=\"notist-text\""));
        assert_eq!(html.matches("id=\"mix\"").count(), 1);
        assert_eq!(html.matches("notist-annotated").count(), 1);
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
                    Element::Details {
                        summary: None,
                        open: false,
                        body: Content {
                            elements: vec![item("one", 10), item("two", 14)],
                        },
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
        let evaluation = Evaluator::default()
            .evaluate("#item(ordered=true)[First]\n#item(ordered=true)[Second]");
        let structured = structure(evaluation);
        let html = render(&structured.document);
        assert_eq!(html.matches("<ol").count(), 1);
        assert_eq!(html.matches("<li").count(), 2);
        assert!(html.contains("First") && html.contains("Second"));
    }

    #[test]
    fn renders_explicit_items_grouped_into_containers() {
        let evaluation = Evaluator::default()
            .evaluate("#item[One]#item[Two]#item(ordered=true)[Three]#item(ordered=true)[Four]");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<ul data-notist-start="));
        assert!(html.contains("<ol data-notist-start="));
        assert_eq!(html.matches("<li").count(), 4);
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
    fn renders_pipe_tables_as_semantic_html() {
        let evaluation =
            Evaluator::default().evaluate("| Name | Value |\n| :--- | ---: |\n| one | 1 |\n");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<div class=\"notist-table-wrapper\">"));
        assert!(html.contains("<table data-notist-columns=\"2\""));
        assert!(html.contains("<thead><tr><th class=\"notist-table-align-left\""));
        assert!(html.contains(">Name</span></p></th>"));
        assert!(html.contains("<th class=\"notist-table-align-right\""));
        assert!(html.contains("<tbody><tr><td"));
        assert!(html.contains(">1</span></p></td></tr></tbody>"));
        assert!(html.contains("</table></div>"));
    }

    #[test]
    fn renders_figure_wrapper_with_caption() {
        let evaluation = Evaluator::default().evaluate(
            "#figure(caption: [Cap], supplement: [Tab], kind: \"table\")[\n  #table(columns: 2)[#table-cell[A] #table-cell[B]]\n]",
        );
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<figure class=\"notist-figure\" data-notist-kind=\"table\""));
        assert!(!html.contains("<figure class=\"notist-figure\" data-notist-kind=\"table\"><p>"));
        assert!(html.contains("<div class=\"notist-table-wrapper\">"));
        assert!(html.contains("<figcaption>"));
        assert!(html.contains(">Tab</span>: "));
        assert!(html.contains(">Cap</span></figcaption>"));
        assert!(html.contains("</figure>"));
    }

    #[test]
    fn renders_inline_elements_with_semantic_tags() {
        let evaluation = Evaluator::default()
            .evaluate("#strong[bold] #emph[slanted] #strike[gone] #underline[under]");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("<strong"));
        assert!(html.contains("<em"));
        assert!(html.contains("<s"));
        assert!(html.contains("<u"));
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
    fn renders_heading_levels() {
        let evaluator = Evaluator::default();
        let heading = render(&structure(evaluator.evaluate("= Title")).document);
        let second_heading = render(&structure(evaluator.evaluate("== Subtitle")).document);
        assert!(heading.contains("<h1"));
        assert!(second_heading.contains("<h2"));
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
        assert!(html.contains("id=\"Top\""));
        assert!(
            html.contains("<nav class=\"notist-outline\" aria-label=\"Table of contents\"><ol>")
        );
        assert!(html.contains("href=\"#Top\""));
        assert!(html.contains("href=\"#Nested\""));
        assert!(!html.contains("notist-outline-level-4"));
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
    fn renders_rule_elements() {
        let evaluation = Evaluator::default().evaluate("#rule()");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("class=\"notist-rule\""));
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
    fn unsafe_url_references_render_without_executable_hrefs() {
        // R10: external url references are syntactically legal; the renderer
        // must never emit them as clickable hrefs.
        let evaluation = Evaluator::default()
            .evaluate("[[javascript:alert(1)]] [[data:text/html,<script>alert(1)</script>]]");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let html = render(&structure(evaluation).document);
        assert!(html.contains("notist-reference-unresolved"));
        assert!(!html.contains("href=\"javascript:"));
        assert!(!html.contains("href=\"data:text/html"));
        assert!(!html.contains("<script>"));
    }
}
