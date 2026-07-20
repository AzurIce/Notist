use notist_model::{ModuleReference, TextRange, WikiReference};

mod argument;
mod parser;
mod raw;
mod scope;

pub use argument::{
    Argument, Expression, ExpressionKind, StringLiteral, StringLiteralForm, StringLiteralStyle,
};
pub use raw::{RawLiteral, RawLiteralForm, SpannedText};
pub use scope::{Attribute, AttributeValue, Attributes, BodyForm, SpannedName};

/// A parsed wiki-style module or label reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WikiLink {
    pub target: WikiReference,
    pub range: TextRange,
}

/// A recoverable syntax error with a precise source range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxError {
    pub message: String,
    pub range: TextRange,
}

/// A Markup sequence. Document roots and Content literals share this node.
#[derive(Clone, Debug, PartialEq)]
pub struct Markup {
    pub items: Vec<MarkupItem>,
    pub range: TextRange,
}

impl Default for Markup {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            range: TextRange::new(0, 0),
        }
    }
}

/// One source-ordered Markup item.
#[derive(Clone, Debug, PartialEq)]
pub enum MarkupItem {
    Text(SpannedText),
    Wiki(WikiLink),
    Raw(RawLiteral),
    Embedded(EmbeddedExpression),
}

/// A `#`-prefixed Code expression embedded into Markup.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddedExpression {
    pub expression: Expression,
    /// Source-only metadata. Type checking and evaluation ignore it.
    pub attributes: Attributes,
    /// The `#` plus expression, excluding postfix Attributes.
    pub scope_range: TextRange,
    /// The complete expression including postfix Attributes.
    pub range: TextRange,
}

/// A Code-mode Content literal whose body is parsed as Markup.
#[derive(Clone, Debug, PartialEq)]
pub struct ContentBlock {
    pub markup: Markup,
    pub payload_range: TextRange,
    pub form: BodyForm,
    pub range: TextRange,
}

/// A Function call expression.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub name: SpannedName,
    pub arguments_range: Option<TextRange>,
    pub arguments: Vec<Argument>,
    pub trailing: Vec<ContentBlock>,
    pub range: TextRange,
}

/// A complete mode-aware syntax tree.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parse {
    pub root: Markup,
    pub errors: Vec<SyntaxError>,
}

impl Parse {
    /// Collects wiki links in source order.
    pub fn links(&self) -> Vec<&WikiLink> {
        let mut output = Vec::new();
        visit_markup(&self.root, &mut |item| {
            if let MarkupItem::Wiki(link) = item {
                output.push(link);
            }
        });
        output
    }

    /// Collects Function calls in source order.
    pub fn calls(&self) -> Vec<&Call> {
        let mut output = Vec::new();
        visit_markup_expressions(&self.root, &mut |expression| {
            if let ExpressionKind::Call(call) = &expression.kind {
                output.push(call.as_ref());
            }
        });
        output
    }

    /// Collects host Raw literals in source order.
    pub fn raw_literals(&self) -> Vec<&RawLiteral> {
        let mut output = Vec::new();
        visit_markup(&self.root, &mut |item| {
            if let MarkupItem::Raw(raw) = item {
                output.push(raw);
            }
        });
        output
    }

    /// Collects annotated embedded expressions in source order.
    pub fn annotations(&self) -> Vec<&EmbeddedExpression> {
        let mut output = Vec::new();
        visit_markup(&self.root, &mut |item| {
            if let MarkupItem::Embedded(embedded) = item
                && embedded.attributes.range.is_some()
            {
                output.push(embedded);
            }
        });
        output
    }

    /// Finds the innermost embedded expression containing an offset.
    pub fn embedded_at(&self, offset: usize) -> Option<&EmbeddedExpression> {
        let mut found = None;
        visit_markup(&self.root, &mut |item| {
            if let MarkupItem::Embedded(embedded) = item
                && embedded.range.start <= offset
                && offset < embedded.range.end
            {
                found = Some(embedded);
            }
        });
        found
    }
}

fn visit_markup<'a>(markup: &'a Markup, visitor: &mut impl FnMut(&'a MarkupItem)) {
    for item in &markup.items {
        visitor(item);
        if let MarkupItem::Embedded(embedded) = item {
            visit_expression_markup(&embedded.expression, visitor);
        }
    }
}

fn visit_expression_markup<'a>(
    expression: &'a Expression,
    visitor: &mut impl FnMut(&'a MarkupItem),
) {
    match &expression.kind {
        ExpressionKind::Content(block) => visit_markup(&block.markup, visitor),
        ExpressionKind::Call(call) => {
            for argument in &call.arguments {
                visit_expression_markup(&argument.expression, visitor);
            }
            for block in &call.trailing {
                visit_markup(&block.markup, visitor);
            }
        }
        ExpressionKind::Parenthesized(inner) => visit_expression_markup(inner, visitor),
        _ => {}
    }
}

fn visit_markup_expressions<'a>(markup: &'a Markup, visitor: &mut impl FnMut(&'a Expression)) {
    for item in &markup.items {
        if let MarkupItem::Embedded(embedded) = item {
            visit_expression(&embedded.expression, visitor);
        }
    }
}

fn visit_expression<'a>(expression: &'a Expression, visitor: &mut impl FnMut(&'a Expression)) {
    visitor(expression);
    match &expression.kind {
        ExpressionKind::Content(block) => visit_markup_expressions(&block.markup, visitor),
        ExpressionKind::Call(call) => {
            for argument in &call.arguments {
                visit_expression(&argument.expression, visitor);
            }
            for block in &call.trailing {
                visit_markup_expressions(&block.markup, visitor);
            }
        }
        ExpressionKind::Parenthesized(inner) => visit_expression(inner, visitor),
        _ => {}
    }
}

/// Parses a complete Notist source as top-level Markup.
pub fn parse(source: &str) -> Parse {
    parser::parse(source)
}

/// Parses a wiki reference body without the surrounding `[[` and `]]`.
pub fn parse_wiki_reference(source: &str) -> Result<WikiReference, String> {
    let source = source.trim();
    if source.is_empty() {
        return Err("wiki reference cannot be empty".into());
    }

    let mut parts = source.split('#');
    let module_part = parts.next().unwrap_or_default();
    let label = parts.next().map(str::trim).map(str::to_owned);
    if parts.next().is_some() {
        return Err("wiki reference contains more than one `#`".into());
    }
    if label.as_deref() == Some("") {
        return Err("wiki reference label cannot be empty".into());
    }

    let module = parse_module_reference(module_part.trim(), label.is_some())?;
    Ok(WikiReference { module, label })
}

fn parse_module_reference(source: &str, has_label: bool) -> Result<ModuleReference, String> {
    if source.is_empty() {
        if has_label {
            return Ok(ModuleReference::Relative(Vec::new()));
        }
        return Err("module path cannot be empty".into());
    }

    let segments: Vec<_> = source.split("::").map(str::trim).collect();
    if segments.iter().any(|segment| segment.is_empty()) {
        return Err("module path contains an empty segment".into());
    }
    if let Some(segment) = segments.iter().find(|segment| !is_module_segment(segment)) {
        return Err(format!("invalid module path segment `{segment}`"));
    }

    if segments[0] == "vault" {
        if segments[1..]
            .iter()
            .any(|segment| matches!(*segment, "vault" | "super" | "self"))
        {
            return Err(
                "`vault`, `super`, and `self` are only allowed at the start of a path".into(),
            );
        }
        return Ok(ModuleReference::Absolute(
            segments[1..]
                .iter()
                .map(|segment| (*segment).to_owned())
                .collect(),
        ));
    }

    if segments[0] == "super" {
        let levels = segments
            .iter()
            .take_while(|segment| **segment == "super")
            .count();
        if segments[levels..]
            .iter()
            .any(|segment| matches!(*segment, "vault" | "super" | "self"))
        {
            return Err(
                "`vault`, `super`, and `self` are only allowed at the start of a path".into(),
            );
        }
        return Ok(ModuleReference::Parent {
            levels,
            remainder: segments[levels..]
                .iter()
                .map(|segment| (*segment).to_owned())
                .collect(),
        });
    }

    if segments[0] == "self" {
        if segments[1..]
            .iter()
            .any(|segment| matches!(*segment, "vault" | "super" | "self"))
        {
            return Err(
                "`vault`, `super`, and `self` are only allowed at the start of a path".into(),
            );
        }
        return Ok(ModuleReference::Relative(
            segments[1..]
                .iter()
                .map(|segment| (*segment).to_owned())
                .collect(),
        ));
    }

    if segments
        .iter()
        .any(|segment| matches!(*segment, "vault" | "super" | "self"))
    {
        return Err("`vault`, `super`, and `self` are reserved path segments".into());
    }

    Ok(ModuleReference::Relative(
        segments
            .iter()
            .map(|segment| (*segment).to_owned())
            .collect(),
    ))
}

fn is_module_segment(source: &str) -> bool {
    !source.chars().any(|character| {
        character.is_control() || matches!(character, '/' | '\\' | '#' | '[' | ']')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_markup_and_code_modes_into_one_tree() {
        let parse = parse("a[plain]#\"text\"#[content]z");
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        assert_eq!(parse.root.items.len(), 4);
        assert!(matches!(&parse.root.items[0], MarkupItem::Text(text) if text.value == "a[plain]"));
        assert!(matches!(
            &parse.root.items[1],
            MarkupItem::Embedded(EmbeddedExpression {
                expression: Expression {
                    kind: ExpressionKind::String(_),
                    ..
                },
                ..
            })
        ));
        assert!(matches!(
            &parse.root.items[2],
            MarkupItem::Embedded(EmbeddedExpression {
                expression: Expression {
                    kind: ExpressionKind::Content(_),
                    ..
                },
                ..
            })
        ));
        assert!(matches!(&parse.root.items[3], MarkupItem::Text(text) if text.value == "z"));
    }

    #[test]
    fn parses_content_literals_as_ordinary_and_trailing_arguments() {
        let parse = parse("#quote(body=[ordinary]) #quote[trailing]");
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        let calls = parse.calls();
        assert_eq!(calls.len(), 2);
        assert!(matches!(
            calls[0].arguments[0].expression.kind,
            ExpressionKind::Content(_)
        ));
        assert!(calls[0].trailing.is_empty());
        assert_eq!(calls[1].trailing.len(), 1);
    }

    #[test]
    fn nests_annotated_content_without_confusing_later_raw_markup() {
        let parse = parse("#[outer #[inner]@inner]@outer `real`");
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        assert_eq!(parse.annotations().len(), 2);
        let raw = parse.raw_literals();
        assert_eq!(raw.len(), 1);
        assert_eq!(
            parse_source(
                &parse,
                raw[0].payload_range,
                "#[outer #[inner]@inner]@outer `real`"
            ),
            "real"
        );
    }

    #[test]
    fn reports_missing_code_expressions_and_accepts_leading_dot_floats() {
        let parse = parse("#heading(level=)[x] #heading(.5)[x]");
        assert!(
            parse
                .errors
                .iter()
                .any(|error| error.message.contains("expected Code expression"))
        );
        let calls = parse.calls();
        assert!(matches!(
            calls[1].arguments[0].expression.kind,
            ExpressionKind::Float(value) if value == 0.5
        ));
    }

    #[test]
    fn parses_module_references_and_hides_markup_inside_raw() {
        let parse = parse("[[intro]] `[[hidden]]` #[[[self::target]]]");
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        let links = parse.links();
        assert_eq!(links.len(), 2);
        assert_eq!(parse.raw_literals().len(), 1);
    }

    #[test]
    fn parses_nested_brackets_as_markup_text_inside_content() {
        let parse = parse("#[outer [literal] brackets]");
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        let calls = parse.calls();
        assert!(calls.is_empty());
        let MarkupItem::Embedded(embedded) = &parse.root.items[0] else {
            panic!()
        };
        let ExpressionKind::Content(block) = &embedded.expression.kind else {
            panic!()
        };
        assert!(
            matches!(&block.markup.items[0], MarkupItem::Text(text) if text.value == "outer [literal] brackets")
        );
    }

    fn parse_source<'a>(_parse: &Parse, range: TextRange, source: &'a str) -> &'a str {
        &source[range.start..range.end]
    }
}
