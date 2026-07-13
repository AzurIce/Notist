//! Semantic HTML rendering for structured Notist documents.

use std::fmt::Write;

use notist_model::{
    Block, Content, Element, ElementNode, ModulePath, ModuleReference, StructuredDocument,
    UnresolvedCallBody, WikiReference,
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

/// Resolves an absolute module target and optional label to an HTML URL.
pub type ReferenceResolver<'a> = dyn Fn(&ModulePath, Option<&str>) -> Option<String> + 'a;

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

fn render_internal<'a>(
    document: &StructuredDocument,
    options: &'a RenderOptions<'a>,
    resolver: Option<&'a ReferenceResolver<'a>>,
) -> String {
    let mut renderer = Renderer {
        output: String::new(),
        options,
        reference_resolver: resolver,
    };
    renderer.document(document);
    renderer.output
}

struct Renderer<'a, 'options> {
    output: String,
    options: &'options RenderOptions<'a>,
    reference_resolver: Option<&'options ReferenceResolver<'options>>,
}

impl Renderer<'_, '_> {
    fn document(&mut self, document: &StructuredDocument) {
        for block in &document.blocks {
            self.block(block);
        }
    }

    fn block(&mut self, block: &Block) {
        match block {
            Block::Paragraph(content) => {
                self.output.push_str("<p>");
                self.inline_content(content);
                self.output.push_str("</p>");
            }
            Block::List(items) => {
                self.output.push_str("<ul>");
                for item in items {
                    self.list_item(item);
                }
                self.output.push_str("</ul>");
            }
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

    fn list_item(&mut self, node: &ElementNode) {
        self.output.push_str("<li");
        self.range_attributes(node);
        self.output.push('>');
        match &node.element {
            Element::ListItem(body) => self.flow_content(body),
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
            Element::Parbreak => self.output.push_str("<br><br>"),
            Element::Strong(body) => {
                self.output.push_str("<strong");
                self.range_attributes(node);
                self.output.push('>');
                self.inline_content(body);
                self.output.push_str("</strong>");
            }
            Element::Heading { level, body } => {
                let level = (*level).clamp(1, 6);
                write!(self.output, "<h{level}").unwrap();
                self.range_attributes(node);
                self.output.push('>');
                self.inline_content(body);
                write!(self.output, "</h{level}>").unwrap();
            }
            Element::ListItem(body) => {
                self.output.push_str("<ul><li");
                self.range_attributes(node);
                self.output.push('>');
                self.flow_content(body);
                self.output.push_str("</li></ul>");
            }
            Element::Quote(body) => {
                self.output.push_str("<blockquote");
                self.range_attributes(node);
                self.output.push('>');
                self.flow_content(body);
                self.output.push_str("</blockquote>");
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
                body,
                block,
            } => self.unresolved_call(name, arguments.as_deref(), body, *block, node, position),
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
        body: &UnresolvedCallBody,
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
        match body {
            UnresolvedCallBody::Content(body) if tag == "div" => self.flow_content(body),
            UnresolvedCallBody::Content(body) => self.inline_content(body),
            UnresolvedCallBody::Raw(body) => {
                self.output.push_str("<code>");
                escape_text(&mut self.output, body);
                self.output.push_str("</code>");
            }
        }
        write!(self.output, "</{tag}>").unwrap();
    }

    fn range_attributes(&mut self, node: &ElementNode) {
        write!(
            self.output,
            " data-notist-start=\"{}\" data-notist-end=\"{}\"",
            node.range.start, node.range.end
        )
        .unwrap();
    }
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
        UnresolvedCallBody,
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
            "#heading(level=2)[Title]\n\nBefore after\n\n#quote[First\n\nSecond]\n\n#raw(lang=\"rust\")![\nfn main() {}\n]!",
        );
        let structured = structure(evaluation);

        let html = render(&structured.document);

        assert!(html.starts_with("<h2 data-notist-start=\"0\""));
        assert!(html.contains("<p><span class=\"notist-text\""));
        assert!(html.contains("<blockquote"));
        assert!(html.contains(">First</span></p><p>"));
        assert!(html.contains("<pre"));
        assert!(html.contains("<code class=\"language-rust\">\nfn main() {}\n</code></pre>"));
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
            annotations: Vec::new(),
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
                Block::Paragraph(Content::single(
                    Element::Strong(Content::single(
                        Element::Text("important".into()),
                        TextRange::new(0, 9),
                    )),
                    TextRange::new(0, 9),
                )),
                Block::Element(node(
                    Element::Quote(Content {
                        elements: vec![item("one", 10), item("two", 14)],
                    }),
                    10,
                    17,
                )),
            ],
            annotations: Vec::new(),
        };

        let html = render(&document);

        assert!(html.contains("<strong data-notist-start=\"0\" data-notist-end=\"9\">"));
        assert_eq!(html.matches("<ul>").count(), 1);
        assert_eq!(html.matches("<li").count(), 2);
    }

    #[test]
    fn preserves_unresolved_content_and_raw_bodies() {
        let document = StructuredDocument {
            blocks: vec![
                Block::Paragraph(Content {
                    elements: vec![node(
                        Element::UnresolvedCall {
                            name: "plugin::inline".into(),
                            arguments: Some("kind=\"tip\"".into()),
                            body: UnresolvedCallBody::Content(Content::single(
                                Element::Text("visible".into()),
                                TextRange::new(10, 17),
                            )),
                            block: false,
                        },
                        0,
                        18,
                    )],
                }),
                Block::Element(node(
                    Element::UnresolvedCall {
                        name: "plugin::raw".into(),
                        arguments: None,
                        body: UnresolvedCallBody::Raw("<unsafe>".into()),
                        block: true,
                    },
                    19,
                    40,
                )),
            ],
            annotations: Vec::new(),
        };

        let html = render(&document);

        assert!(html.contains("<span class=\"notist-unresolved-call\""));
        assert!(html.contains("data-notist-arguments=\"kind=&quot;tip&quot;\""));
        assert!(html.contains(">visible</span></span>"));
        assert!(html.contains("<div class=\"notist-unresolved-call\""));
        assert!(html.contains("<code>&lt;unsafe&gt;</code>"));
    }
}
