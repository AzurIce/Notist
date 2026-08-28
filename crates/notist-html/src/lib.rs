//! Semantic HTML projection and rendering for structured Notist documents.

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use notist_eval::ElementTree;
use notist_model::{
    ModulePath, ModuleReference, Node, NodeValue, TableAlignment, TableCellPlacement, TextRange,
    Target, table_layout_nodes,
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

/// A target-side projection contribution.
pub trait HtmlProjectionHandler: Send + Sync {
    /// The semantic element name this handler handles.
    fn element_name(&self) -> &str;

    /// Projects one semantic node into zero or more target data nodes.
    fn project(&self, node: &Node) -> Option<Vec<Node>>;
}

/// A registry that reduces a formed semantic forest into HTML data nodes.
pub struct HtmlProjectionRegistry {
    handlers: Vec<Box<dyn HtmlProjectionHandler>>,
}

impl Default for HtmlProjectionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HtmlProjectionRegistry {
    const MAX_PROJECTION_DEPTH: usize = 64;

    /// Creates an empty projection registry.
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
        }
    }

    /// Registers a target-side projection handler.
    pub fn register(&mut self, handler: impl HtmlProjectionHandler + 'static) {
        self.handlers.push(Box::new(handler));
    }

    /// Reduces a formed tree and returns its projected copy.
    pub fn reduce_tree(&self, tree: &ElementTree) -> ElementTree {
        ElementTree {
            roots: self.reduce_nodes(tree.roots.clone(), 0),
        }
    }

    /// Alias for [`Self::reduce_tree`].
    pub fn project_tree(&self, tree: &ElementTree) -> ElementTree {
        self.reduce_tree(tree)
    }

    fn reduce_nodes(&self, nodes: Vec<Node>, depth: usize) -> Vec<Node> {
        nodes
            .into_iter()
            .flat_map(|node| self.reduce_node(node, depth))
            .collect()
    }

    fn reduce_node(&self, mut node: Node, depth: usize) -> Vec<Node> {
        node.children = self.reduce_nodes(node.children, 0);
        for (_, value) in &mut node.args {
            *value = self.reduce_value(std::mem::replace(value, NodeValue::None), 0);
        }

        if node.name.starts_with("core::") || node.name.starts_with("html::") {
            return vec![node];
        }
        if depth >= Self::MAX_PROJECTION_DEPTH {
            return vec![fallback_projection_node(node)];
        }

        let projected = self
            .handlers
            .iter()
            .find(|handler| element_name_matches(handler.element_name(), &node.name))
            .and_then(|handler| {
                tracing::trace!(
                    target: "notist_html",
                    element = %node.name,
                    handler = handler.element_name(),
                    "projection handler matched"
                );
                handler.project(&node)
            });
        match projected {
            Some(nodes) => self.reduce_nodes(nodes, depth + 1),
            None => {
                tracing::debug!(
                    target: "notist_html",
                    element = %node.name,
                    fallback_tag = %fallback_html_tag(&node.name),
                    "no projection handler; emitting fallback tag"
                );
                vec![fallback_projection_node(node)]
            }
        }
    }

    fn reduce_value(&self, value: NodeValue, depth: usize) -> NodeValue {
        match value {
            NodeValue::Stream(nodes) => NodeValue::Stream(self.reduce_nodes(nodes, depth)),
            NodeValue::Array(values) => NodeValue::Array(
                values
                    .into_iter()
                    .map(|value| self.reduce_value(value, depth))
                    .collect(),
            ),
            value => value,
        }
    }
}

/// A registry of target-side HTML projections.
pub struct HtmlRendererRegistry {
    projections: HtmlProjectionRegistry,
}

impl Default for HtmlRendererRegistry {
    fn default() -> Self {
        let mut registry = Self::new();
        registry.register_projection(ShaderHtmlProjectionHandler);
        registry
    }
}

impl HtmlRendererRegistry {
    /// Creates an empty projection registry used by the HTML renderer.
    pub fn new() -> Self {
        Self {
            projections: HtmlProjectionRegistry::new(),
        }
    }

    /// Registers a target-side projection handler.
    pub fn register_projection(&mut self, handler: impl HtmlProjectionHandler + 'static) {
        self.projections.register(handler);
    }

    fn project_tree(&self, tree: &ElementTree) -> ElementTree {
        self.projections.project_tree(tree)
    }
}

/// Registers a manifest-declared Web Component projection.
///
/// The projection emits an `html::*` data node. The package's JS/CSS assets
/// are injected into the page head by the CLI build layer; manifest validation
/// remains the authority for the declared custom-element tag.
pub fn register_web_component_renderer(
    registry: &mut HtmlRendererRegistry,
    element_name: &str,
    tag: &str,
) {
    tracing::debug!(
        target: "notist_html",
        element = element_name,
        tag,
        "registered declarative web-component projection"
    );
    registry.register_projection(WebComponentHtmlRenderer {
        element_name: element_name.to_owned(),
        tag: tag.to_owned(),
    });
}

/// A generic projection for plugin Web Components declared in `plugin.json`.
pub struct WebComponentHtmlRenderer {
    element_name: String,
    tag: String,
}

impl HtmlProjectionHandler for WebComponentHtmlRenderer {
    fn element_name(&self) -> &str {
        &self.element_name
    }

    fn project(&self, node: &Node) -> Option<Vec<Node>> {
        let mut projected = if node.block {
            Node::block_call(format!("html::{}", self.tag), node.range)
        } else {
            Node::call(format!("html::{}", self.tag), node.range)
        };
        projected.args.push((
            "class".to_owned(),
            NodeValue::String("notist-web-component".to_owned()),
        ));
        projected.args.push((
            "data-notist-element".to_owned(),
            NodeValue::String(node.name.clone()),
        ));
        projected.args.extend(
            node.args
                .iter()
                .filter(|(_, value)| is_scalar_value(value))
                .map(|(name, value)| (format!("data-{name}"), value.clone())),
        );
        projected.children = node.children.clone();
        Some(vec![projected])
    }
}

/// Built-in Shadertoy-like projection for the `shader` plugin.
struct ShaderHtmlProjectionHandler;

impl HtmlProjectionHandler for ShaderHtmlProjectionHandler {
    fn element_name(&self) -> &str {
        "shader"
    }

    fn project(&self, node: &Node) -> Option<Vec<Node>> {
        let mut source = None;
        let mut width = 800i64;
        let mut height = 600i64;
        for (name, value) in &node.args {
            match (name.as_str(), value) {
                ("source", NodeValue::String(value)) => source = Some(value.clone()),
                ("width", NodeValue::Int(value)) => width = *value,
                ("height", NodeValue::Int(value)) => height = *value,
                _ => {}
            }
        }
        let source = source.filter(|source| !source.is_empty())?;
        let mut projected = if node.block {
            Node::block_call("html::notist-shader", node.range)
        } else {
            Node::call("html::notist-shader", node.range)
        };
        projected.args = vec![
            (
                "class".to_owned(),
                NodeValue::String("notist-shader".to_owned()),
            ),
            (
                "data-notist-element".to_owned(),
                NodeValue::String(node.name.clone()),
            ),
            ("data-shader-source".to_owned(), NodeValue::String(source)),
            ("data-width".to_owned(), NodeValue::Int(width)),
            ("data-height".to_owned(), NodeValue::Int(height)),
        ];
        projected.children = node.children.clone();
        Some(vec![projected])
    }
}

fn element_name_matches(declared: &str, actual: &str) -> bool {
    declared == actual || declared == actual.rsplit("::").next().unwrap_or(actual)
}

fn is_scalar_value(value: &NodeValue) -> bool {
    matches!(
        value,
        NodeValue::None
            | NodeValue::Bool(_)
            | NodeValue::Int(_)
            | NodeValue::Float(_)
            | NodeValue::String(_)
    )
}

fn fallback_projection_node(node: Node) -> Node {
    let name = node.name.clone();
    let tag = fallback_html_tag(&name);
    let mut projected = if node.block {
        Node::block_call(format!("html::{tag}"), node.range)
    } else {
        Node::call(format!("html::{tag}"), node.range)
    };
    projected.args = node.args;
    projected
        .args
        .push(("data-notist-element".to_owned(), NodeValue::String(name)));
    projected.children = node.children;
    projected
}

fn fallback_html_tag(name: &str) -> String {
    let source = name.split("::").collect::<Vec<_>>().join("-");
    let mut tag = source
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    while tag.starts_with('-') {
        tag.remove(0);
    }
    if tag.is_empty() {
        return "notist-element".to_owned();
    }
    if !tag.starts_with(|character: char| character.is_ascii_alphabetic()) {
        tag.insert_str(0, "notist-");
    }
    tag
}

fn scalar_value_string(value: &NodeValue) -> Option<String> {
    match value {
        NodeValue::None => None,
        NodeValue::Bool(value) => Some(value.to_string()),
        NodeValue::Int(value) => Some(value.to_string()),
        NodeValue::Float(value) => Some(value.to_string()),
        NodeValue::String(value) => Some(value.clone()),
        NodeValue::Stream(_) | NodeValue::Array(_) | NodeValue::Target(_) => None,
    }
}

fn html_attribute_name(name: &str) -> String {
    if name == "class" || name == "id" {
        return name.to_owned();
    }
    let safe = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    if name.starts_with("data-") || name.starts_with("aria-") {
        if safe.is_empty() {
            "data-value".to_owned()
        } else {
            safe
        }
    } else if safe.is_empty() {
        "data-value".to_owned()
    } else {
        format!("data-{safe}")
    }
}

/// Renders a canonical [`ElementTree`] with the default options.
///
/// The canonical tree is the stable input shape. Without a reference
/// resolver, links fall back to the default module-url encoding.
pub fn render_element_tree(tree: &ElementTree) -> String {
    render_element_tree_with_renderers(
        tree,
        &RenderOptions::default(),
        None,
        &[],
        &HtmlRendererRegistry::default(),
    )
}

/// Renders an [`ElementTree`] with caller-provided projection options,
/// reference resolution, and annotations.
///
/// The registry projects plugin nodes into `html::*` data nodes before the
/// serializer renders them. A `None` reference resolver falls back to the
/// default module-url encoding; a resolver returning `None` leaves the
/// reference visible but unclickable.
pub fn render_element_tree_with_renderers(
    tree: &ElementTree,
    options: &RenderOptions<'_>,
    reference_resolver: Option<&ReferenceResolver<'_>>,
    annotations: &[RenderedAnnotation],
    renderers: &HtmlRendererRegistry,
) -> String {
    let projected_tree = renderers.project_tree(tree);
    let plan = AnchorPlan::compute_tree(&projected_tree, annotations);
    let mut renderer = Renderer {
        output: String::new(),
        options,
        reference_resolver,
        annotations,
        plan,
        current_block: None,
        inherited_coverage: Vec::new(),
    };
    renderer.element_tree(&projected_tree);
    renderer.output
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

struct Renderer<'a, 'options> {
    output: String,
    options: &'options RenderOptions<'a>,
    reference_resolver: Option<&'options ReferenceResolver<'options>>,
    annotations: &'options [RenderedAnnotation],
    plan: AnchorPlan,
    /// Range key of the top-level block currently being rendered, used to look
    /// up inline wrapper candidates. `None` outside block rendering.
    current_block: Option<(usize, usize)>,
    /// Indices of the annotations whose wrapping span is already open around
    /// an ancestor inline element; their coverage is inherited, not re-wrapped.
    inherited_coverage: Vec<usize>,
}

impl Renderer<'_, '_> {
    /// Renders the already-projected tree directly. Sections are emitted from
    /// their `core::section` node; plugin nodes have already become `html::*`
    /// data nodes before reaching this serializer.
    fn element_tree(&mut self, tree: &ElementTree) {
        for root in &tree.roots {
            self.tree_node(root);
        }
    }

    fn tree_node(&mut self, node: &Node) {
        if node.is_core("section") {
            self.tree_section(node);
            return;
        }
        self.current_block = Some(range_key(node.range));
        self.tree_element(node, RenderPosition::Block);
    }

    fn tree_section(&mut self, node: &Node) {
        let Some(heading_node) = node.children.first() else {
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
        for child in &node.children[1..] {
            self.tree_node(child);
        }
        self.output.push_str("</section>");
    }

    fn tree_element(&mut self, node: &Node, position: RenderPosition) {
        if let Some(local) = node.name.strip_prefix("html::") {
            self.tree_html_element(local, node);
            return;
        }
        let Some(local) = node.core_local() else {
            return;
        };
        match local {
            "text" => {
                let Some(NodeValue::String(text)) = node.get("text") else {
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
                self.tree_body_inline_content(&node.children);
                self.output.push_str("</p>");
            }
            "heading" => {
                let level = match node.get("level") {
                    Some(NodeValue::Int(level)) => (*level).clamp(1, 6),
                    _ => 1,
                };
                write!(self.output, "<h{level}").unwrap();
                self.projected_class_attribute_range(node.range);
                self.range_attributes_range(node.range);
                self.output.push('>');
                self.tree_body_inline_content(&node.children);
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
                self.tree_body_inline_content(&node.children);
                self.output.push_str("</");
                self.output.push_str(tag);
                self.output.push('>');
            }
            "rule" => {
                self.output.push_str("<hr class=\"notist-rule\"");
                self.range_attributes_range(node.range);
                self.output.push('>');
            }
            "reference" => {
                match node.get("target") {
                    Some(NodeValue::Target(reference)) => {
                        self.reference_range(reference, node.range, true);
                    }
                    Some(NodeValue::String(url)) => {
                        self.external_reference_range(url, node.range);
                    }
                    _ => {
                        // Legacy nodes carry a raw `url` spelling.
                        let Some(NodeValue::String(url)) = node.get("url") else {
                            return;
                        };
                        if let Ok(reference) = notist_syntax::parse_reference_url(url) {
                            self.reference_range(&reference, node.range, false);
                        }
                    }
                }
            }
            "raw" => {
                let Some(NodeValue::String(text)) = node.get("source") else {
                    return;
                };
                let block = matches!(node.get("block"), Some(NodeValue::Bool(true)));
                let language = match node.get("lang") {
                    Some(NodeValue::String(language)) => Some(language.as_str()),
                    _ => None,
                };
                self.raw_range(text, block, language, node.range);
            }
            "callout" => {
                let kind = match node.get("kind") {
                    Some(NodeValue::String(kind)) => kind.as_str(),
                    _ => "note",
                };
                self.output.push_str("<aside class=\"notist-callout");
                self.projected_class_suffix_range(node.range);
                self.output.push_str("\" data-notist-kind=\"");
                escape_attribute(&mut self.output, kind);
                self.output.push('"');
                self.range_attributes_range(node.range);
                self.output.push('>');
                if let Some(NodeValue::Stream(title)) = node.get("title") {
                    self.output.push_str("<div class=\"notist-callout-title\">");
                    self.tree_inline_content(title);
                    self.output.push_str("</div>");
                }
                self.tree_flow_content(&node.children);
                self.output.push_str("</aside>");
            }
            "details" => {
                let open = matches!(node.get("open"), Some(NodeValue::Bool(true)));
                self.output.push_str("<details class=\"notist-details");
                self.projected_class_suffix_range(node.range);
                self.output.push('"');
                if open {
                    self.output.push_str(" open");
                }
                self.range_attributes_range(node.range);
                self.output.push_str("><summary>");
                if let Some(NodeValue::Stream(summary)) = node.get("summary") {
                    self.tree_inline_content(summary);
                } else {
                    escape_text(&mut self.output, "Details");
                }
                self.output.push_str("</summary>");
                self.tree_flow_content(&node.children);
                self.output.push_str("</details>");
            }
            "figure" => {
                let kind = match node.get("kind") {
                    Some(NodeValue::String(kind)) => kind.clone(),
                    _ => "figure".to_owned(),
                };
                self.output.push_str("<figure class=\"notist-figure");
                self.projected_class_suffix_range(node.range);
                self.output.push_str("\" data-notist-kind=\"");
                escape_attribute(&mut self.output, &kind);
                self.output.push('"');
                self.range_attributes_range(node.range);
                self.output.push('>');
                self.tree_figure_body(&node.children);
                if let Some(NodeValue::Stream(caption)) = node.get("caption") {
                    self.output.push_str("<figcaption>");
                    if let Some(NodeValue::Stream(supplement)) = node.get("supplement") {
                        self.tree_inline_content(supplement);
                        escape_text(&mut self.output, ": ");
                    }
                    self.tree_inline_content(caption);
                    self.output.push_str("</figcaption>");
                }
                self.output.push_str("</figure>");
            }
            "unresolved-call" => {
                let Some(NodeValue::String(name)) = node.get("name") else {
                    return;
                };
                let arguments = match node.get("arguments") {
                    Some(NodeValue::String(arguments)) => Some(arguments.as_str()),
                    _ => None,
                };
                let block = node.block;
                self.unresolved_call_range(
                    name,
                    arguments,
                    &node.children,
                    block,
                    node.range,
                    position,
                );
            }
            "item" => self.tree_list_item(node),
            "list" => self.tree_list(node),
            "table-cell" => {
                self.output.push_str("<div class=\"notist-table-cell");
                self.projected_class_suffix_range(node.range);
                self.output.push('"');
                self.range_attributes_range(node.range);
                self.output.push('>');
                self.tree_body_flow_content(&node.children);
                self.output.push_str("</div>");
            }
            "table" => self.tree_table(node),
            _ => {}
        }
    }

    fn tree_html_element(&mut self, local: &str, node: &Node) {
        let tag = fallback_html_tag(local);
        self.output.push('<');
        self.output.push_str(&tag);
        let annotation_classes = self
            .plan
            .projections
            .get(&range_key(node.range))
            .map(|projection| projection.classes.join(" "))
            .filter(|classes| !classes.is_empty());
        let mut class_written = false;
        for (name, value) in &node.args {
            let Some(value) = scalar_value_string(value) else {
                continue;
            };
            let attribute = html_attribute_name(name);
            self.output.push(' ');
            self.output.push_str(&attribute);
            self.output.push_str("=\"");
            escape_attribute(&mut self.output, &value);
            if attribute == "class" {
                if let Some(classes) = &annotation_classes {
                    self.output.push(' ');
                    escape_attribute(&mut self.output, classes);
                }
                class_written = true;
            }
            self.output.push('"');
        }
        if !class_written && let Some(classes) = annotation_classes {
            self.output.push_str(" class=\"");
            escape_attribute(&mut self.output, &classes);
            self.output.push('"');
        }
        self.range_attributes_range(node.range);
        self.output.push('>');
        if node.block {
            self.tree_flow_content(&node.children);
        } else {
            self.tree_inline_content(&node.children);
        }
        self.output.push_str("</");
        self.output.push_str(&tag);
        self.output.push('>');
    }

    fn tree_list_item(&mut self, node: &Node) {
        let ordered = matches!(node.get("ordered"), Some(NodeValue::Bool(true)));
        let value = match node.get("value") {
            Some(NodeValue::Int(value)) => Some(*value),
            _ => None,
        };
        self.output.push_str("<li");
        if let Some(value) = value {
            write!(self.output, " value=\"{value}\"").unwrap();
        }
        self.projected_class_attribute_range(node.range);
        self.range_attributes_range(node.range);
        self.output.push('>');
        self.tree_body_flow_content(&node.children);
        self.output.push_str("</li>");
        let _ = ordered;
    }

    fn tree_list(&mut self, node: &Node) {
        let ordered = matches!(node.get("ordered"), Some(NodeValue::Bool(true)));
        if ordered {
            self.output.push_str("<ol");
            if let Some(first) = node.children.first()
                && let Some(NodeValue::Int(value)) = first.get("value")
            {
                write!(self.output, " start=\"{value}\"").unwrap();
            }
        } else {
            self.output.push_str("<ul");
        }
        self.projected_class_attribute_range(node.range);
        self.range_attributes_range(node.range);
        self.output.push('>');
        for child in &node.children {
            if child.is_core("item") {
                self.tree_list_item(child);
            } else {
                self.tree_node(child);
            }
        }
        self.output
            .push_str(if ordered { "</ol>" } else { "</ul>" });
    }

    fn tree_table_row(
        &mut self,
        cells: &[Node],
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
            let colspan = match cell.get("colspan") {
                Some(NodeValue::Int(value)) => u16::try_from(*value).unwrap_or(1),
                _ => 1,
            };
            let rowspan = match cell.get("rowspan") {
                Some(NodeValue::Int(value)) => u16::try_from(*value).unwrap_or(1),
                _ => 1,
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
            self.range_attributes_range(cell.range);
            self.output.push('>');
            self.tree_body_flow_content(&cell.children);
            write!(self.output, "</{tag}>").unwrap();
        }
        self.output.push_str("</tr>");
    }

    fn tree_table(&mut self, node: &Node) {
        let columns = match node.get("columns") {
            Some(NodeValue::Int(columns)) => u16::try_from(*columns).unwrap_or(1),
            _ => 1,
        };
        let header = matches!(node.get("header"), Some(NodeValue::Bool(true)));
        let alignments = match node.get("align") {
            Some(NodeValue::String(align)) => tree_alignments(Some(align), columns as usize),
            _ => tree_alignments(None, columns as usize),
        };
        let cells = &node.children;
        let rows = table_layout_nodes(columns, cells).unwrap_or_else(|_| {
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

        self.output.push_str("<div class=\"notist-table-wrapper");
        self.projected_class_suffix_range(node.range);
        write!(self.output, "\"><table data-notist-columns=\"{columns}\"").unwrap();
        self.range_attributes_range(node.range);
        self.output.push('>');
        if header {
            self.output.push_str("<thead>");
            if let Some(row) = rows.first() {
                self.tree_table_row(cells, row, "th", &alignments);
            }
            self.output.push_str("</thead>");
        }
        let body_rows = if header {
            rows.iter().skip(1).collect::<Vec<_>>()
        } else {
            rows.iter().collect::<Vec<_>>()
        };
        if !body_rows.is_empty() {
            self.output.push_str("<tbody>");
            for row in body_rows {
                self.tree_table_row(cells, row, "td", &alignments);
            }
            self.output.push_str("</tbody>");
        }
        self.output.push_str("</table></div>");
    }

    /// Renders a canonical inline body with the coverage-aware inline
    /// renderer.
    fn tree_inline_content(&mut self, nodes: &[Node]) {
        let mut open_coverage = Vec::new();
        for node in nodes {
            self.tree_inline_element_with_coverage(node, &mut open_coverage);
        }
        self.annotation_span_close(&mut open_coverage);
    }

    fn tree_inline_element_with_coverage(&mut self, node: &Node, open_coverage: &mut Vec<usize>) {
        let coverage = self.tree_inline_coverage(node);
        if node.is_core("text") {
            let partial = self.tree_partial_coverage(node);
            if !partial.is_empty()
                && let Some(NodeValue::String(text)) = node.get("text")
            {
                self.render_split_text(text, node.range, &coverage, &partial, open_coverage);
                return;
            }
        }
        if coverage != *open_coverage {
            self.annotation_span_close(open_coverage);
            if !coverage.is_empty() {
                self.annotation_span_open(&coverage);
            }
            *open_coverage = coverage;
        }
        self.inherited_coverage.extend(open_coverage.iter());
        self.tree_element(node, RenderPosition::Inline);
        let inherited = self.inherited_coverage.len() - open_coverage.len();
        self.inherited_coverage.truncate(inherited);
    }

    fn tree_inline_coverage(&self, node: &Node) -> Vec<usize> {
        if !node_is_inline(node) {
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

    fn tree_partial_coverage(&self, node: &Node) -> Vec<usize> {
        if !node_is_inline(node) {
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

    fn tree_flow_content(&mut self, nodes: &[Node]) {
        let mut paragraph_open = false;
        let mut open_coverage = Vec::new();
        let mut index = 0;
        while index < nodes.len() {
            let node = &nodes[index];
            if node_is_inline(node) {
                if !paragraph_open {
                    self.output.push_str("<p>");
                    paragraph_open = true;
                }
                self.tree_inline_element_with_coverage(node, &mut open_coverage);
                index += 1;
                continue;
            }

            if paragraph_open {
                self.annotation_span_close(&mut open_coverage);
                self.output.push_str("</p>");
                paragraph_open = false;
            }

            if node.is_core("parbreak") {
                index += 1;
                continue;
            }
            if node.is_core("item") {
                let ordered = matches!(node.get("ordered"), Some(NodeValue::Bool(true)));
                if ordered {
                    self.output.push_str("<ol");
                    if let Some(NodeValue::Int(value)) = node.get("value") {
                        write!(self.output, " start=\"{value}\"").unwrap();
                    }
                    self.output.push('>');
                } else {
                    self.output.push_str("<ul>");
                }
                while index < nodes.len() {
                    let item = &nodes[index];
                    if !item.is_core("item")
                        || !matches!(
                            item.get("ordered"),
                            Some(NodeValue::Bool(value)) if *value == ordered
                        )
                    {
                        break;
                    }
                    self.tree_list_item(item);
                    index += 1;
                }
                self.output
                    .push_str(if ordered { "</ol>" } else { "</ul>" });
                continue;
            }
            self.tree_element(node, RenderPosition::Block);
            index += 1;
        }
        if paragraph_open {
            self.annotation_span_close(&mut open_coverage);
            self.output.push_str("</p>");
        }
    }

    fn tree_figure_body(&mut self, nodes: &[Node]) {
        let is_framing = |node: &Node| {
            node.is_core("parbreak")
                || (node.is_core("text")
                    && node.get("text").is_some_and(
                        |value| matches!(value, NodeValue::String(text) if text.trim().is_empty()),
                    ))
        };
        let first = nodes
            .iter()
            .position(|node| !is_framing(node))
            .unwrap_or(nodes.len());
        let last = nodes
            .iter()
            .rposition(|node| !is_framing(node))
            .map_or(first, |index| index + 1);
        self.tree_flow_content(&nodes[first..last]);
    }

    fn tree_body_inline_content(&mut self, body: &[Node]) {
        self.tree_inline_content(body);
    }

    fn tree_body_flow_content(&mut self, body: &[Node]) {
        self.tree_flow_content(body);
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

    fn reference_range(&mut self, reference: &Target, range: TextRange, slash: bool) {
        let target = match &reference.module {
            ModuleReference::Absolute(_) => reference.module.resolve_from(&ModulePath::root()),
            _ => self
                .options
                .current_module
                .and_then(|current| reference.module.resolve_from(current)),
        };

        let href = target.as_ref().and_then(|target| {
            self.reference_resolver.map_or_else(
                || Some(self.default_reference_href(target, reference.name.as_deref())),
                |resolver| resolver(target, reference.name.as_deref()),
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

        self.range_attributes_range(range);
        self.output.push('>');
        self.reference_text(reference, slash);

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

    fn external_reference_range(&mut self, url: &str, range: TextRange) {
        self.output
            .push_str("<span class=\"notist-reference notist-reference-external\"");
        self.range_attributes_range(range);
        self.output.push('>');
        escape_text(&mut self.output, url);
        self.output.push_str("</span>");
    }

    fn reference_text(&mut self, reference: &Target, slash: bool) {
        let mut text = reference.module.to_string();
        if let Some(name) = &reference.name {
            text.push(if slash { '/' } else { '#' });
            text.push_str(name);
        }
        escape_text(&mut self.output, &text);
    }

    fn raw_range(&mut self, text: &str, block: bool, language: Option<&str>, range: TextRange) {
        if block {
            self.output.push_str("<pre");
            self.projected_class_attribute_range(range);
            self.range_attributes_range(range);
            self.output.push_str("><code");
        } else {
            self.output.push_str("<code");
            self.range_attributes_range(range);
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

    fn unresolved_call_range(
        &mut self,
        name: &str,
        arguments: Option<&str>,
        trailing: &[Node],
        block: bool,
        range: TextRange,
        position: RenderPosition,
    ) {
        let tag = container_tag(block, position);
        self.output.push('<');
        self.output.push_str(tag);
        self.output.push_str(" class=\"notist-unresolved-call");
        self.projected_class_suffix_range(range);
        self.output.push_str("\" data-notist-name=\"");
        escape_attribute(&mut self.output, name);
        self.output.push('"');
        if let Some(arguments) = arguments {
            self.output.push_str(" data-notist-arguments=\"");
            escape_attribute(&mut self.output, arguments);
            self.output.push('"');
        }
        self.range_attributes_range(range);
        self.output.push('>');
        if !trailing.is_empty() {
            if tag == "div" {
                self.tree_flow_content(trailing);
            } else {
                self.tree_inline_content(trailing);
            }
        }
        write!(self.output, "</{tag}>").unwrap();
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

    fn projected_class_suffix_range(&mut self, range: TextRange) {
        let Some(projection) = self.plan.projections.get(&range_key(range)) else {
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
/// [`module_anchors_tree`] always agree on the label-to-anchor mapping.
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
                project_tree_annotation(
                    root,
                    annotation,
                    annotation_index,
                    &mut projections,
                    &mut inline_wrappers,
                );
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

fn tree_alignments(source: Option<&str>, columns: usize) -> Vec<TableAlignment> {
    let Some(source) = source else {
        return vec![TableAlignment::Default; columns];
    };
    source
        .split(',')
        .map(|value| match value.trim() {
            "default" | "" => TableAlignment::Default,
            "left" => TableAlignment::Left,
            "center" => TableAlignment::Center,
            "right" => TableAlignment::Right,
            _ => TableAlignment::Default,
        })
        .take(columns)
        .collect()
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

/// Extends one projection target with an annotation's classes, tags, and
/// properties.
fn extend_projection(projection: &mut Projection, annotation: &RenderedAnnotation) {
    projection
        .classes
        .extend(annotation.classes.iter().cloned());
    projection.tags.extend(annotation.tags.iter().cloned());
    projection
        .properties
        .extend(annotation.properties.iter().cloned());
}

/// Classifies one tree node against an annotation scope (D0010). A fully
/// covered section takes the projection itself without descending; headings
/// never carry tag projections and always fall back to inline wrapping.
fn project_tree_annotation(
    node: &Node,
    annotation: &RenderedAnnotation,
    annotation_index: usize,
    projections: &mut HashMap<(usize, usize), Projection>,
    inline_wrappers: &mut HashMap<(usize, usize), Vec<usize>>,
) {
    let key = range_key(node.range);
    if contains(annotation.scope, node.range) {
        if node.is_core("heading") {
            inline_wrappers
                .entry(key)
                .or_default()
                .push(annotation_index);
        } else {
            extend_projection(projections.entry(key).or_default(), annotation);
        }
        return;
    }
    if !intersects(annotation.scope, node.range) {
        return;
    }
    if node.is_core("section") {
        for child in &node.children {
            project_tree_annotation(
                child,
                annotation,
                annotation_index,
                projections,
                inline_wrappers,
            );
        }
        return;
    }
    inline_wrappers
        .entry(key)
        .or_default()
        .push(annotation_index);
}

fn tree_heading_text(node: &Node) -> Option<String> {
    if !node.is_core("heading") {
        return None;
    }
    Some(node_plain_text(&node.children))
}

fn walk_tree_node(node: &Node, output: &mut Vec<WalkedElement>) {
    output.push(WalkedElement {
        range: node.range,
        heading_text: tree_heading_text(node),
    });
    let Some(local) = node.core_local() else {
        walk_tree_nodes(&node.children, output);
        return;
    };
    match local {
        "paragraph" | "strong" | "emph" | "strike" | "underline" | "heading" | "item"
        | "custom" | "unresolved-call" | "section" => walk_tree_nodes(&node.children, output),
        "callout" => {
            if let Some(NodeValue::Stream(nodes)) = node.get("title") {
                walk_tree_nodes(nodes, output);
            }
            walk_tree_nodes(&node.children, output);
        }
        "details" => {
            if let Some(NodeValue::Stream(nodes)) = node.get("summary") {
                walk_tree_nodes(nodes, output);
            }
            walk_tree_nodes(&node.children, output);
        }
        _ => {}
    }
}

fn walk_tree_nodes(nodes: &[Node], output: &mut Vec<WalkedElement>) {
    for node in nodes {
        walk_tree_node(node, output);
    }
}

fn collect_outline_entries_tree(tree: &ElementTree, plan: &AnchorPlan) -> Vec<RenderedHeading> {
    fn heading_record(node: &Node, plan: &AnchorPlan) -> Option<RenderedHeading> {
        if !node.is_core("heading") {
            return None;
        }
        let level = match node.get("level")? {
            NodeValue::Int(level) => u8::try_from(*level).ok()?,
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

    fn walk(nodes: &[Node], plan: &AnchorPlan, output: &mut Vec<RenderedHeading>) {
        for node in nodes {
            if node.is_core("section") {
                if let Some(heading) = node.children.first()
                    && let Some(record) = heading_record(heading, plan)
                {
                    output.push(record);
                }
                walk(&node.children[1..], plan, output);
            } else if let Some(record) = heading_record(node, plan) {
                output.push(record);
            }
        }
    }

    let mut output = Vec::new();
    walk(&tree.roots, plan, &mut output);
    output
}

fn node_plain_text(nodes: &[Node]) -> String {
    nodes
        .iter()
        .map(|node| match node.core_local() {
            Some("text") => match node.get("text") {
                Some(NodeValue::String(text)) => text.clone(),
                _ => String::new(),
            },
            Some("paragraph" | "strong" | "emph" | "strike" | "underline") => {
                node_plain_text(&node.children)
            }
            _ => String::new(),
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RenderPosition {
    Inline,
    Block,
}

fn node_is_inline(node: &Node) -> bool {
    match node.core_local() {
        Some("text" | "reference" | "strong" | "emph" | "strike" | "underline") => true,
        Some(_) => false,
        None => !node.block,
    }
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
    use notist_eval::Evaluator;
    use notist_model::{ModulePath, Node, TextRange};
    use notist_plugin_core as core_plugin;

    use super::*;

    fn text(value: &str, start: usize, end: usize) -> Node {
        Node::call("core::text", TextRange::new(start, end)).arg("text", value)
    }

    fn paragraph(children: Vec<Node>, start: usize, end: usize) -> Node {
        let mut node = Node::block_call("core::paragraph", TextRange::new(start, end));
        node.children = children;
        node
    }

    fn tree(roots: Vec<Node>) -> ElementTree {
        ElementTree { roots }
    }

    fn evaluate(source: &str) -> notist_eval::Evaluation {
        let (registry, shaping) = core_plugin::registry();
        let evaluation = Evaluator::new(registry).evaluate_with_shaping(source, &shaping);
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        evaluation
    }

    struct RewriteHandler {
        from: &'static str,
        to: &'static str,
    }

    impl HtmlProjectionHandler for RewriteHandler {
        fn element_name(&self) -> &str {
            self.from
        }

        fn project(&self, node: &Node) -> Option<Vec<Node>> {
            let mut projected = if node.block {
                Node::block_call(self.to, node.range)
            } else {
                Node::call(self.to, node.range)
            };
            projected.args = node.args.clone();
            projected.children = node.children.clone();
            Some(vec![projected])
        }
    }

    #[test]
    fn renders_canonical_element_tree_entry_point() {
        let evaluation = evaluate("= Title\n\nBody");
        let html = render_element_tree(&evaluation.tree);
        assert!(html.starts_with("<section "), "{html}");
        assert!(html.contains("<h1 id=\"Title\""), "{html}");
        assert!(html.contains("Body</span>"), "{html}");
    }

    #[test]
    fn renders_evaluated_document_structure() {
        let evaluation = evaluate(
            "#heading(level=2)[Title]\n\nBefore after\n\n#details[First\n\nSecond]\n\n#raw(r#\"\"\"\nfn main() {}\n\"\"\"#, lang=\"rust\", block=true)",
        );

        let html = render_element_tree(&evaluation.tree);

        // D0010: the heading and its content form a nested <section>.
        assert!(html.starts_with("<section data-notist-start=\"0\""));
        assert!(html.contains("<h2 id=\"Title\" data-notist-start=\"0\""));
        assert!(html.contains(
            "<p data-notist-start=\"26\" data-notist-end=\"38\"><span class=\"notist-text\""
        ));
        assert!(html.contains("<details"));
        assert!(html.contains(">First</span></p><p data-notist-start=\"56\""));
        assert!(html.contains("<pre"));
        assert!(html.contains("<code class=\"language-rust\">fn main() {}</code></pre>"));
    }

    #[test]
    fn renders_soft_break_without_newline() {
        // Regression: a soft break inside a paragraph must not reach the HTML
        // output, or browsers collapse it into a stray space between CJK text.
        let evaluation = evaluate("第一段。\n第二段。");
        let html = render_element_tree(&evaluation.tree);
        assert!(!html.contains('\n'), "{html}");
        assert!(
            html.contains("第一段。</span><span class=\"notist-text\" data-notist-start=\"13\" data-notist-end=\"25\">第二段。"),
            "{html}"
        );
    }

    #[test]
    fn renders_manifest_web_component_with_scalar_fields() {
        let mut card = Node::block_call("card::card", TextRange::new(0, 5))
            .arg("title", "Hello & welcome")
            .arg("count", 3_i64);
        card.children = vec![text("fallback", 2, 10)];
        let mut renderers = HtmlRendererRegistry::new();
        register_web_component_renderer(&mut renderers, "card", "notist-card");
        let html = render_element_tree_with_renderers(
            &tree(vec![card]),
            &RenderOptions::default(),
            Some(&|_, _| None),
            &[],
            &renderers,
        );
        assert!(html.contains("<notist-card"));
        assert!(html.contains("data-notist-element=\"card::card\""));
        assert!(html.contains("data-title=\"Hello &amp; welcome\""));
        assert!(html.contains("data-count=\"3\""));
        assert!(html.contains(">fallback</span></p>"));
        assert!(!html.contains("<script"));
    }

    #[test]
    fn renders_unknown_qualified_calls_through_html_fallback() {
        let mut node = Node::block_call("demo::box", TextRange::new(0, 5)).arg("title", "<&>");
        node.children = vec![text("body", 1, 5)];

        let html = render_element_tree(&tree(vec![node]));

        assert!(html.contains("<demo-box"), "{html}");
        assert!(html.contains("data-title=\"&lt;&amp;&gt;\""), "{html}");
        assert!(html.contains("data-notist-element=\"demo::box\""), "{html}");
        assert!(html.contains(">body</span>"), "{html}");
    }

    #[test]
    fn projection_handlers_reenter_fixpoint_and_reduce_children() {
        let mut registry = HtmlProjectionRegistry::new();
        registry.register(RewriteHandler {
            from: "outer",
            to: "middle",
        });
        registry.register(RewriteHandler {
            from: "middle",
            to: "html::article",
        });
        let root = Node::block_call("outer", TextRange::new(0, 10))
            .child(Node::call("inner::child", TextRange::new(1, 6)).arg("title", "<&>"));

        let projected = registry.project_tree(&tree(vec![root]));

        assert_eq!(projected.roots[0].name, "html::article");
        assert_eq!(projected.roots[0].children[0].name, "html::inner-child");
        let html = render_element_tree(&projected);
        assert!(html.contains("<article"), "{html}");
        assert!(html.contains("data-title=\"&lt;&amp;&gt;\""), "{html}");
    }

    #[test]
    fn renders_mermaid_plugin_as_web_component_with_escaped_source() {
        let mut diagram = Node::block_call("mermaid::diagram", TextRange::new(0, 5))
            .arg("source", "flowchart LR\n  A --> B\n  B --> C & D <E>")
            .arg("theme", "dark");
        diagram.children = vec![text("caption", 2, 10)];
        let mut renderers = HtmlRendererRegistry::new();
        register_web_component_renderer(&mut renderers, "diagram", "notist-mermaid");

        let html = render_element_tree_with_renderers(
            &tree(vec![diagram]),
            &RenderOptions::default(),
            Some(&|_, _| None),
            &[],
            &renderers,
        );
        assert!(html.contains("<notist-mermaid"), "{html}");
        assert!(html.contains("class=\"notist-web-component\""));
        assert!(html.contains("data-notist-element=\"mermaid::diagram\""));
        assert!(
            html.contains(
                "data-source=\"flowchart LR\n  A --&gt; B\n  B --&gt; C &amp; D &lt;E&gt;\""
            ),
            "{html}"
        );
        assert!(html.contains("data-theme=\"dark\""));
        assert!(html.contains("</notist-mermaid>"));
        assert!(!html.contains("<script"));
    }

    #[test]
    fn renders_shader_plugin_as_webgpu_canvas() {
        let mut shader = Node::block_call("shader::shader", TextRange::new(0, 5))
            .arg(
                "source",
                "fn mainImage(fragCoord: vec2<f32>) -> vec4<f32> { return vec4<f32>(fragCoord, 0.0, 1.0); }",
            )
            .arg("width", 320_i64)
            .arg("height", 200_i64);
        shader.children = vec![text("fallback", 2, 10)];

        let html = render_element_tree(&tree(vec![shader]));
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
        let evaluation = evaluate("abcdef");
        let annotations = vec![RenderedAnnotation {
            scope: TextRange::new(2, 4),
            id: None,
            classes: vec!["mark".into()],
            tags: Vec::new(),
            properties: Vec::new(),
        }];
        let html = render_element_tree_with_renderers(
            &evaluation.tree,
            &RenderOptions::default(),
            Some(&|_: &ModulePath, _: Option<&str>| None),
            &annotations,
            &HtmlRendererRegistry::default(),
        );
        assert!(html.contains("ab<span class=\"notist-annotated mark\""));
        assert!(html.contains("cd</span>ef"));
    }

    #[test]
    fn sections_nest_and_receive_section_level_projection() {
        let evaluation = evaluate("= 一级\n\n段落\n\n== 二级\n\n内文\n\n= 一级二\n");
        let html = render_element_tree(&evaluation.tree);
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
        let html = render_element_tree_with_renderers(
            &evaluation.tree,
            &RenderOptions::default(),
            Some(&|_: &ModulePath, _: Option<&str>| None),
            &annotations,
            &HtmlRendererRegistry::default(),
        );
        assert!(html.contains("<section class=\"wip\""));
        assert!(html.contains("data-notist-tag=\"draft\""));
        assert!(html.contains("data-notist-status=\"draft\""));
    }

    #[test]
    fn renders_plain_paragraph_element() {
        let evaluation = evaluate("plain paragraph");
        let html = render_element_tree(&evaluation.tree);
        assert!(html.starts_with("<p data-notist-start=\"0\""));
        assert!(html.ends_with("</p>"));
    }

    #[test]
    fn escapes_text_attributes_and_raw_bodies() {
        let mut custom = Node::block_call("x\" onclick=\"bad", TextRange::new(0, 5));
        custom.children = vec![text("<&>", 2, 5)];

        let html = render_element_tree(&tree(vec![custom]));

        assert!(html.contains("data-notist-element=\"x&quot; onclick=&quot;bad\""));
        assert!(html.contains("&lt;&amp;&gt;"));
        assert!(!html.contains("onclick=\"bad\""));
    }

    #[test]
    fn resolves_and_encodes_reference_links() {
        let evaluation = evaluate("#<intro page/A B> #<super::index> #<vault::shared>");
        let current = ModulePath::from_segments(["notes".into(), "today".into()]);
        let options = RenderOptions {
            current_module: Some(&current),
            module_url_prefix: "/preview?module=",
        };

        let html = render_element_tree_with_renderers(
            &evaluation.tree,
            &options,
            None,
            &[],
            &HtmlRendererRegistry::default(),
        );

        assert!(html.contains(
            "href=\"/preview?module=vault%3A%3Anotes%3A%3Atoday%3A%3Aintro%20page#A%20B\""
        ));
        assert!(html.contains("href=\"/preview?module=vault%3A%3Anotes%3A%3Aindex\""));
        assert!(html.contains("href=\"/preview?module=vault%3A%3Ashared\""));
    }

    #[test]
    fn leaves_relative_references_unclickable_without_a_current_module() {
        let evaluation = evaluate("#<child> #<vault::shared>");

        let html = render_element_tree(&evaluation.tree);

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
        let explicit = evaluate("#link(<child>)");
        let sugar = evaluate("#<child>");
        let render = |evaluation: &notist_eval::Evaluation| {
            render_element_tree_with_renderers(
                &evaluation.tree,
                &options,
                None,
                &[],
                &HtmlRendererRegistry::default(),
            )
        };
        assert!(render(&explicit).contains("href=\"?module=vault%3A%3Anotes%3A%3Achild\""));
        assert!(render(&sugar).contains("href=\"?module=vault%3A%3Anotes%3A%3Achild\""));
    }

    #[test]
    fn uses_a_caller_provided_reference_resolver() {
        let evaluation = evaluate("#<child> #<missing>");
        let current = ModulePath::root();
        let options = RenderOptions {
            current_module: Some(&current),
            module_url_prefix: "",
        };
        let resolver = |target: &ModulePath, _label: Option<&str>| {
            (target.segments() == ["child"]).then(|| "child/".into())
        };

        let html = render_element_tree_with_renderers(
            &evaluation.tree,
            &options,
            Some(&resolver),
            &[],
            &HtmlRendererRegistry::default(),
        );

        assert!(html.contains("href=\"child/\""));
        assert!(html.contains("notist-reference-unresolved"));
    }

    #[test]
    fn uses_heading_text_as_the_default_anchor() {
        let evaluation = evaluate("= 简介\n\n正文");

        let html = render_element_tree(&evaluation.tree);

        assert!(html.contains("<h1 id=\"简介\""));
        assert_eq!(
            module_anchors_tree(&evaluation.tree, &[]),
            vec![("简介".to_owned(), "简介".to_owned())]
        );
    }

    #[test]
    fn falls_back_to_loc_anchors_for_invalid_heading_text() {
        let evaluation = evaluate("#heading[1st steps]");

        let html = render_element_tree(&evaluation.tree);

        assert!(html.contains("<h1 id=\"loc-0\""));
        assert_eq!(
            module_anchors_tree(&evaluation.tree, &[]),
            vec![("1st steps".to_owned(), "loc-0".to_owned())]
        );
    }

    #[test]
    fn deduplicates_repeated_heading_anchors() {
        let evaluation = evaluate("= Intro\n\n= Intro");

        let html = render_element_tree(&evaluation.tree);

        assert!(html.contains("<h1 id=\"Intro\""));
        assert!(html.contains("<h1 id=\"loc-9\""));
        assert_eq!(
            module_anchors_tree(&evaluation.tree, &[]),
            vec![("Intro".to_owned(), "Intro".to_owned())]
        );
    }

    #[test]
    fn explicit_ids_override_heading_text_anchors() {
        let mut heading =
            Node::block_call("core::heading", TextRange::new(0, 7)).arg("level", 1_i64);
        heading.children = vec![text("Intro", 1, 6)];
        let document = tree(vec![
            heading,
            paragraph(vec![text("quoted", 18, 24)], 8, 25),
        ]);
        let annotations = vec![RenderedAnnotation {
            scope: TextRange::new(8, 25),
            id: Some("Intro".into()),
            classes: Vec::new(),
            tags: Vec::new(),
            properties: Vec::new(),
        }];

        let anchors = module_anchors_tree(&document, &annotations);
        assert_eq!(anchors, vec![("Intro".to_owned(), "Intro".to_owned())]);

        let html = render_element_tree_with_renderers(
            &document,
            &RenderOptions::default(),
            Some(&|_: &ModulePath, _: Option<&str>| None),
            &annotations,
            &HtmlRendererRegistry::default(),
        );
        assert!(html.contains("<h1 id=\"loc-0\""));
        assert!(html.contains("<p id=\"Intro\""));
    }

    #[test]
    fn projects_annotation_attributes_onto_block_elements() {
        let document = tree(vec![paragraph(vec![text("quoted", 8, 14)], 0, 15)]);
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

        let html = render_element_tree_with_renderers(
            &document,
            &RenderOptions::default(),
            Some(&|_: &ModulePath, _: Option<&str>| None),
            &annotations,
            &HtmlRendererRegistry::default(),
        );

        assert!(html.contains("<p class=\"hero\" id=\"quote-id\""));
        assert!(html.contains("data-notist-tag=\"design wip\""));
        assert!(html.contains("data-notist-status=\"draft\""));
        // Characters outside [a-z0-9-] become hyphens in attribute keys.
        assert!(html.contains("data-notist--eviewed=\"yes &amp; no\""));
    }

    #[test]
    fn merges_projected_classes_into_fixed_class_attributes() {
        let mut callout =
            Node::block_call("core::callout", TextRange::new(0, 10)).arg("kind", "tip");
        callout.children = vec![text("hint", 5, 9)];
        let annotations = vec![RenderedAnnotation {
            scope: TextRange::new(0, 10),
            id: None,
            classes: vec!["hero".into()],
            tags: Vec::new(),
            properties: Vec::new(),
        }];

        let html = render_element_tree_with_renderers(
            &tree(vec![callout]),
            &RenderOptions::default(),
            Some(&|_: &ModulePath, _: Option<&str>| None),
            &annotations,
            &HtmlRendererRegistry::default(),
        );

        assert!(html.contains("<aside class=\"notist-callout hero\""));
    }

    #[test]
    fn assigns_explicit_ids_to_inline_elements() {
        let mut strong = Node::call("core::strong", TextRange::new(2, 14));
        strong.children = vec![text("bold", 9, 13)];
        let document = tree(vec![paragraph(vec![strong], 0, 20)]);
        let annotations = vec![RenderedAnnotation {
            scope: TextRange::new(2, 14),
            id: Some("bold".into()),
            classes: Vec::new(),
            tags: Vec::new(),
            properties: Vec::new(),
        }];

        let html = render_element_tree_with_renderers(
            &document,
            &RenderOptions::default(),
            Some(&|_: &ModulePath, _: Option<&str>| None),
            &annotations,
            &HtmlRendererRegistry::default(),
        );

        assert!(html.contains("<strong id=\"bold\""));
    }

    #[test]
    fn projects_multi_block_scopes_onto_every_covered_block() {
        let document = tree(vec![
            paragraph(vec![text("first", 1, 6)], 0, 10),
            paragraph(vec![text("second", 12, 18)], 11, 20),
        ]);
        let annotations = vec![RenderedAnnotation {
            scope: TextRange::new(0, 20),
            id: Some("multi".into()),
            classes: vec!["hero".into()],
            tags: vec!["design".into()],
            properties: vec![("status".into(), "draft".into())],
        }];

        let html = render_element_tree_with_renderers(
            &document,
            &RenderOptions::default(),
            Some(&|_: &ModulePath, _: Option<&str>| None),
            &annotations,
            &HtmlRendererRegistry::default(),
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
    fn projects_multi_block_annotation_scopes_nested_inside_sections() {
        // Regression: an annotation whose scope spans several blocks of one
        // section used to project onto only the first covered block. A
        // heading wraps everything below it into a single section, so this
        // is the realistic shape for `#[...]@anno` over multiple paragraphs.
        let source = "= Title\n\n#[\nfirst para\n\n- item one\n- item two\n\nlast para\n]@mark,.user\ntrailing tail";
        let evaluation = evaluate(source);
        // The scope covers `#[]`: both leading blocks fully, and the last
        // paragraph only partially (following text joins its paragraph).
        let annotations = vec![RenderedAnnotation {
            scope: TextRange::new(9, 59),
            id: None,
            classes: vec!["user".into()],
            tags: Vec::new(),
            properties: vec![("type".into(), "user".into())],
        }];

        let html = render_element_tree_with_renderers(
            &evaluation.tree,
            &RenderOptions::default(),
            Some(&|_, _| None),
            &annotations,
            &HtmlRendererRegistry::default(),
        );
        // Every fully covered block carries the projection on its own tag.
        assert!(
            html.contains("<p class=\"user\" data-notist-type=\"user\""),
            "{html}"
        );
        assert!(html.contains("<ul class=\"user\""), "{html}");
        // The partially overlapped last paragraph falls back to inline
        // wrapping instead of losing the annotation entirely.
        assert!(html.contains("notist-annotated user"), "{html}");
        assert_eq!(html.matches("class=\"user\"").count(), 2);
    }

    #[test]
    fn wraps_partially_covered_inline_elements_in_annotation_spans() {
        let mut strong = Node::call("core::strong", TextRange::new(7, 14));
        strong.children = vec![text("bold", 9, 13)];
        let document = tree(vec![paragraph(
            vec![text("before ", 0, 7), strong, text(" after", 14, 20)],
            0,
            20,
        )]);
        let annotations = vec![RenderedAnnotation {
            scope: TextRange::new(7, 25),
            id: Some("bold".into()),
            classes: vec!["hero".into()],
            tags: vec!["design".into()],
            properties: vec![("status".into(), "draft".into())],
        }];

        let html = render_element_tree_with_renderers(
            &document,
            &RenderOptions::default(),
            Some(&|_: &ModulePath, _: Option<&str>| None),
            &annotations,
            &HtmlRendererRegistry::default(),
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
        let document = tree(vec![paragraph(
            vec![text("a", 0, 1), text("b", 1, 2)],
            0,
            20,
        )]);
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

        let html = render_element_tree_with_renderers(
            &document,
            &RenderOptions::default(),
            Some(&|_: &ModulePath, _: Option<&str>| None),
            &annotations,
            &HtmlRendererRegistry::default(),
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
        let mut callout =
            Node::block_call("core::callout", TextRange::new(0, 40)).arg("kind", "tip");
        callout.children = vec![text("a", 5, 6), text("b", 7, 8)];
        let annotations = vec![RenderedAnnotation {
            scope: TextRange::new(0, 20),
            id: None,
            classes: vec!["hero".into()],
            tags: Vec::new(),
            properties: Vec::new(),
        }];

        let html = render_element_tree_with_renderers(
            &tree(vec![callout]),
            &RenderOptions::default(),
            Some(&|_: &ModulePath, _: Option<&str>| None),
            &annotations,
            &HtmlRendererRegistry::default(),
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
        let document = tree(vec![
            paragraph(vec![text("first", 1, 6)], 0, 10),
            paragraph(vec![text("second", 12, 18)], 11, 20),
            paragraph(vec![text("x", 22, 23), text("y", 35, 36)], 21, 40),
        ]);
        let annotations = vec![RenderedAnnotation {
            scope: TextRange::new(0, 30),
            id: Some("mix".into()),
            classes: vec!["hero".into()],
            tags: vec!["t".into()],
            properties: Vec::new(),
        }];

        let html = render_element_tree_with_renderers(
            &document,
            &RenderOptions::default(),
            Some(&|_: &ModulePath, _: Option<&str>| None),
            &annotations,
            &HtmlRendererRegistry::default(),
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
        let item = |value: &str, start| {
            let mut node =
                Node::block_call("core::item", TextRange::new(start, start + value.len()))
                    .arg("ordered", false);
            node.children = vec![text(value, start, start + value.len())];
            node
        };
        let mut strong = Node::call("core::strong", TextRange::new(0, 9));
        strong.children = vec![text("important", 0, 9)];
        let mut list = Node::block_call("core::list", TextRange::new(10, 17)).arg("ordered", false);
        list.children = vec![item("one", 10), item("two", 14)];
        let mut details = Node::block_call("core::details", TextRange::new(10, 17));
        details.children = vec![list];
        let document = tree(vec![paragraph(vec![strong], 0, 9), details]);

        let html = render_element_tree(&document);

        assert!(html.contains("<strong data-notist-start=\"0\" data-notist-end=\"9\">"));
        assert_eq!(html.matches("<ul").count(), 1);
        assert_eq!(html.matches("<li").count(), 2);
    }

    #[test]
    fn renders_ordered_list_items_as_an_ordered_list() {
        let evaluation = evaluate("#item(ordered=true)[First]\n#item(ordered=true)[Second]");
        let html = render_element_tree(&evaluation.tree);
        assert_eq!(html.matches("<ol").count(), 1);
        assert_eq!(html.matches("<li").count(), 2);
        assert!(html.contains("First") && html.contains("Second"));
    }

    #[test]
    fn renders_explicit_items_grouped_into_containers() {
        let evaluation =
            evaluate("#item[One]#item[Two]#item(ordered=true)[Three]#item(ordered=true)[Four]");
        let html = render_element_tree(&evaluation.tree);
        assert!(html.contains("<ul data-notist-start="));
        assert!(html.contains("<ol data-notist-start="));
        assert_eq!(html.matches("<li").count(), 4);
    }

    #[test]
    fn renders_indented_mixed_nested_lists() {
        let evaluation = evaluate("- parent\n  + first child\n  + second child\n- sibling");
        let html = render_element_tree(&evaluation.tree);
        assert_eq!(html.matches("<ul").count(), 1);
        assert_eq!(html.matches("<ol").count(), 1);
        assert_eq!(html.matches("<li").count(), 4);
        assert!(html.contains("first child") && html.contains("sibling"));
    }

    #[test]
    fn renders_pipe_tables_as_semantic_html() {
        let evaluation = evaluate("| Name | Value |\n| :--- | ---: |\n| one | 1 |\n");
        let html = render_element_tree(&evaluation.tree);
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
        let evaluation = evaluate(
            "#figure(caption: [Cap], supplement: [Tab], kind: \"table\")[\n  #table(columns: 2)[#table-cell[A] #table-cell[B]]\n]",
        );
        let html = render_element_tree(&evaluation.tree);
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
        let evaluation = evaluate("#strong[bold] #emph[slanted] #strike[gone] #underline[under]");
        let html = render_element_tree(&evaluation.tree);
        assert!(html.contains("<strong"));
        assert!(html.contains("<em"));
        assert!(html.contains("<s"));
        assert!(html.contains("<u"));
    }

    #[test]
    fn renders_strike_content() {
        let evaluation = evaluate("~~obsolete~~");
        let html = render_element_tree(&evaluation.tree);
        assert!(html.contains("<s data-notist-start=\"0\""));
        assert!(html.contains("obsolete"));
        assert!(html.contains("</s>"));
    }

    #[test]
    fn renders_heading_levels() {
        let heading = render_element_tree(&evaluate("= Title").tree);
        let second_heading = render_element_tree(&evaluate("== Subtitle").tree);
        assert!(heading.contains("<h1"));
        assert!(second_heading.contains("<h2"));
    }

    #[test]
    fn renders_callouts_as_semantic_asides() {
        let evaluation = evaluate("#callout(kind=\"tip\", title=[Tip])[Use *small* steps]");
        let html = render_element_tree(&evaluation.tree);
        assert!(html.contains("<aside class=\"notist-callout\" data-notist-kind=\"tip\""));
        assert!(html.contains("<div class=\"notist-callout-title\">"));
        assert!(html.contains("<strong"));
        assert!(html.contains("</aside>"));
    }

    #[test]
    fn renders_details_disclosure() {
        let evaluation = evaluate("#details(summary=[More], open=true)[Hidden content]");
        let html = render_element_tree(&evaluation.tree);
        assert!(html.contains("<details class=\"notist-details\" open"));
        assert!(html.contains("<summary>"));
        assert!(html.contains("Hidden content"));
        assert!(html.contains("</details>"));

        let default_summary = evaluate("#details[Hidden content]");
        let html = render_element_tree(&default_summary.tree);
        assert!(html.contains("<summary>Details</summary>"));
    }

    #[test]
    fn renders_inline_content_inside_block_sugar() {
        let evaluation = evaluate("- *bold*\n- _slanted_");
        let html = render_element_tree(&evaluation.tree);
        assert!(html.contains("<li"));
        assert!(html.contains("<strong"));
        assert!(html.contains("<em"));
    }

    #[test]
    fn renders_rule_elements() {
        let evaluation = evaluate("#rule()");
        let html = render_element_tree(&evaluation.tree);
        assert!(html.contains("class=\"notist-rule\""));
    }

    #[test]
    fn preserves_unresolved_trailing_content_and_bodyless_calls() {
        let mut inline = Node::call("core::unresolved-call", TextRange::new(0, 18))
            .arg("name", "plugin::inline")
            .arg("arguments", "kind=\"tip\"");
        inline.children = vec![text("visible", 10, 17)];
        let bodyless = Node::block_call("core::unresolved-call", TextRange::new(19, 40))
            .arg("name", "plugin::bodyless");
        let document = tree(vec![paragraph(vec![inline], 0, 18), bodyless]);

        let html = render_element_tree(&document);

        assert!(html.contains("<span class=\"notist-unresolved-call\""));
        assert!(html.contains("data-notist-arguments=\"kind=&quot;tip&quot;\""));
        assert!(html.contains(">visible</span></span>"));
        assert!(html.contains("<div class=\"notist-unresolved-call\""));
        assert!(html.contains("data-notist-name=\"plugin::bodyless\""));
    }

    #[test]
    fn unsafe_url_references_render_without_executable_hrefs() {
        // External urls live on the String branch of `link`; the renderer
        // must never emit them as clickable hrefs.
        let evaluation =
            evaluate("#link(\"javascript:alert(1)\") #link(\"data:text/html,<script>alert(1)</script>\")");
        let html = render_element_tree(&evaluation.tree);
        assert!(html.contains("notist-reference-external"));
        assert!(!html.contains("href=\"javascript:"));
        assert!(!html.contains("href=\"data:text/html"));
        assert!(!html.contains("<script>"));
    }
}
