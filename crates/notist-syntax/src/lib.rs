use notist_model::{ModuleReference, TextRange, WikiReference};

mod argument;
mod parser;
mod raw;
mod scope;

pub use argument::{
    Argument, BinaryOperator, Expression, ExpressionKind, ImportSelector, StringLiteral,
    StringLiteralForm, UnaryOperator,
    StringLiteralStyle, UserFunctionDefinition, UserParameter,
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
    /// A line-leading `= ...` heading sugar (D0003). The body is Markup up
    /// to the line end.
    Heading(HeadingSugar),
    /// A line-leading `---` rule sugar (D0003).
    Rule(TextRange),
    /// A contiguous run of `- ` / `+ ` list lines (D0003 item sugar); nesting
    /// is carried by row indentation.
    List(ListSugar),
    /// A standalone `@[...]` annotation bound to the next block-level node
    /// (D0006 block-prefix mount point).
    BlockAnnotation(BlockAnnotation),
    /// A file-leading `@![...]` annotation bound to the root scope as module
    /// metadata (D0006 module mount point).
    ModuleAnnotation(BlockAnnotation),
}

/// A line-leading heading sugar node: the `=` run length is the level, the
/// remainder of the line is the heading body (D0003).
#[derive(Clone, Debug, PartialEq)]
pub struct HeadingSugar {
    pub level: u32,
    pub body: Markup,
    pub range: TextRange,
}

/// A contiguous run of `- ` / `+ ` list lines (D0003 item sugar).
#[derive(Clone, Debug, PartialEq)]
pub struct ListSugar {
    pub rows: Vec<ListSugarRow>,
    pub range: TextRange,
}

/// One list line: indentation, marker kind, and the body Markup to the line
/// end (D0003).
#[derive(Clone, Debug, PartialEq)]
pub struct ListSugarRow {
    pub indent: usize,
    pub ordered: bool,
    pub marker_len: usize,
    pub body: Markup,
    pub range: TextRange,
}

/// A standalone bracket-delimited annotation (`@[...]` / `@![...]`, D0006).
#[derive(Clone, Debug, PartialEq)]
pub struct BlockAnnotation {
    pub attributes: Attributes,
    pub range: TextRange,
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

impl EmbeddedExpression {
    /// Returns the independent source range of the postfix `@...` metadata.
    pub fn attributes_range(&self) -> Option<TextRange> {
        self.attributes.range
    }
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

    /// Collects source-defined functions in source order.
    pub fn user_functions(&self) -> Vec<&UserFunctionDefinition> {
        let mut output = Vec::new();
        visit_markup_expressions(&self.root, &mut |expression| {
            if let ExpressionKind::LetFunction(definition) = &expression.kind {
                output.push(definition.as_ref());
            }
        });
        output
    }

    /// Collects import statements in source order (D0004).
    pub fn imports(&self) -> Vec<&Expression> {
        let mut output = Vec::new();
        visit_markup_expressions(&self.root, &mut |expression| {
            if matches!(expression.kind, ExpressionKind::Import { .. }) {
                output.push(expression);
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

    /// Collects Code-mode String literal source ranges in source order.
    pub fn string_literal_ranges(&self) -> Vec<TextRange> {
        let mut output = Vec::new();
        visit_markup_expressions(&self.root, &mut |expression| {
            if matches!(expression.kind, ExpressionKind::String(_)) {
                output.push(expression.range);
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
        match item {
            MarkupItem::Embedded(embedded) => {
                visit_expression_markup(&embedded.expression, visitor)
            }
            MarkupItem::Heading(sugar) => visit_markup(&sugar.body, visitor),
            MarkupItem::List(sugar) => {
                for row in &sugar.rows {
                    visit_markup(&row.body, visitor);
                }
            }
            _ => {}
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
        ExpressionKind::Binary { left, right, .. } => {
            visit_expression_markup(left, visitor);
            visit_expression_markup(right, visitor);
        }
        ExpressionKind::LetFunction(definition) => {
            for parameter in &definition.parameters {
                if let Some(default) = &parameter.default {
                    visit_expression_markup(default, visitor);
                }
            }
            visit_expression_markup(&definition.body, visitor);
        }
        ExpressionKind::Parenthesized(inner) => visit_expression_markup(inner, visitor),
        _ => {}
    }
}

fn visit_markup_expressions<'a>(markup: &'a Markup, visitor: &mut impl FnMut(&'a Expression)) {
    for item in &markup.items {
        match item {
            MarkupItem::Embedded(embedded) => {
                visit_expression(&embedded.expression, visitor)
            }
            MarkupItem::Heading(sugar) => visit_markup_expressions(&sugar.body, visitor),
            MarkupItem::List(sugar) => {
                for row in &sugar.rows {
                    visit_markup_expressions(&row.body, visitor);
                }
            }
            _ => {}
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
        ExpressionKind::Binary { left, right, .. } => {
            visit_expression(left, visitor);
            visit_expression(right, visitor);
        }
        ExpressionKind::LetFunction(definition) => {
            for parameter in &definition.parameters {
                if let Some(default) = &parameter.default {
                    visit_expression(default, visitor);
                }
            }
            visit_expression(&definition.body, visitor);
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
    if looks_like_external(module_part) {
        // External targets are deferred (D0004) but syntactically legal (R10):
        // the literal url string parses and the resolver classifies it External.
        return Ok(WikiReference {
            module: ModuleReference::External(source.to_owned()),
            label: None,
        });
    }
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

/// Detects an external url scheme such as `https://` or `mailto:` while
/// leaving module paths (`vault::x`, which use `::`) alone.
fn looks_like_external(source: &str) -> bool {
    if source.contains("://") {
        return true;
    }
    let bytes = source.as_bytes();
    let Some(colon) = bytes.iter().position(|byte| *byte == b':') else {
        return false;
    };
    if bytes.get(colon + 1) == Some(&b':') {
        return false;
    }
    let scheme = &source[..colon];
    !scheme.is_empty()
        && scheme
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.'))
}

#[cfg(test)]
mod tests {
    use notist_model::Type;

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
    fn attributes_keep_independent_ranges_and_all_metadata_kinds() {
        let source = "#[outer #[inner]@inner,#tag,#tag,.class,.class,key=value,key=\"two\"]@outer,#top,owner=\"Alice\"";
        let parse = parse(source);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        let annotations = parse.annotations();
        assert_eq!(annotations.len(), 2);

        let inner = annotations
            .iter()
            .find(|annotation| annotation.attributes.id.as_ref().unwrap().value == "inner")
            .unwrap();
        assert_eq!(
            &source[inner.scope_range.start..inner.scope_range.end],
            "#[inner]"
        );
        let inner_attributes = inner.attributes_range().unwrap();
        assert_eq!(
            &source[inner_attributes.start..inner_attributes.end],
            "@inner,#tag,#tag,.class,.class,key=value,key=\"two\""
        );
        assert_eq!(inner.attributes.id.as_ref().unwrap().value, "inner");
        assert_eq!(inner.attributes.items.len(), 6);
        assert!(inner.scope_range.end <= inner_attributes.start);
        assert_eq!(inner.range.end, inner_attributes.end);

        let outer = annotations
            .iter()
            .find(|annotation| annotation.attributes.id.as_ref().unwrap().value == "outer")
            .unwrap();
        assert_eq!(outer.attributes.id.as_ref().unwrap().value, "outer");
        assert_eq!(
            &source[outer.attributes_range().unwrap().start..outer.range.end],
            "@outer,#top,owner=\"Alice\""
        );
        assert!(
            inner.range.start >= outer.scope_range.start
                && inner.range.end <= outer.scope_range.end
        );
    }

    #[test]
    fn attributes_may_omit_id_but_reject_a_second_bare_id() {
        let valid = parse("#[a]@#tag,.class,key=value");
        assert!(valid.errors.is_empty(), "{:?}", valid.errors);
        let attributes = &valid.annotations()[0].attributes;
        assert!(attributes.id.is_none());
        assert_eq!(attributes.items.len(), 3);

        let invalid = parse("#[a]@first,second");
        assert!(invalid.errors.iter().any(|error| {
            error.message == "expected a tag, class, or key-value attribute after `,`"
        }));
    }

    #[test]
    fn parser_keeps_multiple_trailing_content_blocks_for_binding_diagnostics() {
        let source = "#quote[first][second]";
        let parse = parse(source);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        assert_eq!(parse.calls()[0].trailing.len(), 2);
        assert_eq!(
            parse.calls()[0]
                .trailing
                .iter()
                .map(|block| &source[block.payload_range.start..block.payload_range.end])
                .collect::<Vec<_>>(),
            ["first", "second"]
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

    #[test]
    fn markup_keeps_comment_syntax_as_ordinary_text() {
        // E09: `//` and `/* ... */` are Code-context trivia only; the Markup
        // text stream keeps them verbatim (including url-like sequences).
        let source =
            "before // hidden\nafter /* outer /* nested */ hidden */ https://example.test/a/*path*/";
        let parse = parse(source);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        let visible = parse
            .root
            .items
            .iter()
            .filter_map(|item| match item {
                MarkupItem::Text(text) => Some(text.value.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(visible, source);
    }

    #[test]
    fn code_trivia_skips_line_and_nested_block_comments() {
        // D0007: comments inside Code contexts are lexical trivia. The
        // argument list is a Code context, so the comments between arguments
        // disappear and the call stays well-formed.
        let parsed = parse("#heading(level: /* outer /* nested */ inner */ 2)[Title]");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        assert!(parsed.calls()[0].arguments.iter().any(|argument| {
            matches!(&argument.name, Some(name) if name.value == "level")
        }));

        let block = parse("#{ // line comment\n 1 + 2 }");
        assert!(block.errors.is_empty(), "{:?}", block.errors);
    }

    #[test]
    fn reports_unclosed_block_comments_in_code_contexts() {
        let parse = parse("#{ /* hidden");
        assert!(
            parse
                .errors
                .iter()
                .any(|error| error.message == "unclosed block comment")
        );
    }

    #[test]
    fn parses_block_and_module_annotations() {
        // D0006: `@![...]` at the file start is the module mount point,
        // `@[...]` at line start is the block-prefix mount point.
        let parsed = parse("@![#design, status = \"draft\"]\n\n@[wip]\n= Title\n\n@[install]\nbody");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        assert!(matches!(
            &parsed.root.items[0],
            MarkupItem::ModuleAnnotation(_)
        ));
        assert_eq!(
            parsed
                .root
                .items
                .iter()
                .filter(|item| matches!(item, MarkupItem::BlockAnnotation(_)))
                .count(),
            2
        );
        let MarkupItem::ModuleAnnotation(module) = &parsed.root.items[0] else {
            panic!()
        };
        assert!(module.attributes.items.iter().any(|attribute| {
            matches!(attribute, Attribute::Tag(name) if name.value == "design")
        }));
        assert!(module.attributes.items.iter().any(|attribute| {
            matches!(attribute, Attribute::KeyValue { key, value, .. } if key.value == "status" && value.raw == "\"draft\"")
        }));
    }

    #[test]
    fn rejects_misplaced_module_annotations_and_dangling_at() {
        let parsed = parse("正文\n@![x]");
        assert!(parsed
            .errors
            .iter()
            .any(|error| error.message.contains("before any content")));
        let parsed = parse("@!missing");
        assert!(parsed
            .errors
            .iter()
            .any(|error| error.message == "expected `[` after `@!`"));
        // `@` followed by an identifier is ordinary text, never an annotation.
        let mention = parse("@user 提及");
        assert!(mention.errors.is_empty(), "{:?}", mention.errors);
        assert!(matches!(
            &mention.root.items[0],
            MarkupItem::Text(text) if text.value == "@user 提及"
        ));
    }

    #[test]
    fn parses_bare_code_blocks_in_markup() {
        // D0006: `{...}` is the Code block form usable bare in Markup.
        let parsed = parse("{ let x = 1; x + 1 }");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let MarkupItem::Embedded(embedded) = &parsed.root.items[0] else {
            panic!()
        };
        assert!(matches!(
            embedded.expression.kind,
            ExpressionKind::Block(_)
        ));
    }

    #[test]
    fn parses_heading_rule_and_list_sugar_as_frontend_items() {
        // D0003: sugar is a syntax-frontend node — the evaluator never
        // rescans source text.
        let parsed = parse("= Title\n\n---\n\n- one\n- two\n");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        assert!(matches!(
            &parsed.root.items[0],
            MarkupItem::Heading(sugar) if sugar.level == 1
        ));
        let Some((rule_index, _)) = parsed
            .root
            .items
            .iter()
            .enumerate()
            .find(|(_, item)| matches!(item, MarkupItem::Rule(_)))
        else {
            panic!()
        };
        assert!(matches!(
            &parsed.root.items[rule_index],
            MarkupItem::Rule(range) if range.start == 9 && range.end == 12
        ));
        let Some((list_index, _)) = parsed
            .root
            .items
            .iter()
            .enumerate()
            .find(|(_, item)| matches!(item, MarkupItem::List(_)))
        else {
            panic!()
        };
        let MarkupItem::List(sugar) = &parsed.root.items[list_index] else {
            panic!()
        };
        assert_eq!(sugar.rows.len(), 2);
        assert!(!sugar.rows[0].ordered);
        assert!(matches!(
            &sugar.rows[0].body.items[0],
            MarkupItem::Text(text) if text.value == "one"
        ));
    }

    #[test]
    fn parses_nested_list_indentation_and_setext_boundaries() {
        let parsed = parse("- parent\n  + child\n- sibling");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let MarkupItem::List(sugar) = &parsed.root.items[0] else {
            panic!()
        };
        assert_eq!(sugar.rows.len(), 3);
        assert_eq!(sugar.rows[0].indent, 0);
        assert!(sugar.rows[1].ordered && sugar.rows[1].indent == 2);
        assert!(!sugar.rows[2].ordered && sugar.rows[2].indent == 0);

        // `=foo` is ordinary text; `= foo` is heading sugar (D0003).
        let boundary = parse("=foo\n= bar");
        assert!(boundary.errors.is_empty(), "{:?}", boundary.errors);
        assert!(matches!(
            &boundary.root.items[0],
            MarkupItem::Text(text) if text.value == "=foo\n"
        ));
        assert!(matches!(
            &boundary.root.items[1],
            MarkupItem::Heading(sugar) if sugar.level == 1
        ));
    }

    #[test]
    fn parses_colon_named_arguments_and_operator_precedence() {
        let parsed = parse("#callout(kind: \"warning\", title: [注意], [内容。])");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let call = &parsed.calls()[0];
        assert_eq!(call.arguments.len(), 3);
        assert_eq!(call.arguments[0].name.as_ref().unwrap().value, "kind");
        assert_eq!(call.arguments[1].name.as_ref().unwrap().value, "title");
        assert!(call.arguments[2].name.is_none());

        let parsed = parse("#(1 + 2 * 3 == 7 and not false or 2 < 3)");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let MarkupItem::Embedded(embedded) = &parsed.root.items[0] else {
            panic!()
        };
        let ExpressionKind::Parenthesized(inner) = &embedded.expression.kind else {
            panic!("expected parenthesized expression")
        };
        let ExpressionKind::Binary { operator: BinaryOperator::Or, .. } = &inner.kind else {
            panic!("expected top-level `or`, got {:?}", inner.kind)
        };
    }

    #[test]
    fn embed_boundary_stops_at_whitespace_and_consumes_semicolon() {
        // R01: `#1 + 2` embeds `1` and keeps " + 2" as Markup text.
        let parsed = parse("#1 + 2");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        assert!(matches!(
            &parsed.root.items[0],
            MarkupItem::Embedded(EmbeddedExpression {
                expression: Expression {
                    kind: ExpressionKind::Int(1),
                    ..
                },
                ..
            })
        ));
        assert!(matches!(&parsed.root.items[1], MarkupItem::Text(text) if text.value == " + 2"));

        // D0001: `;` terminates an embed and produces no output.
        let parsed = parse("第一段#accent;文字");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let text = parsed
            .root
            .items
            .iter()
            .filter_map(|item| match item {
                MarkupItem::Text(text) => Some(text.value.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(text, "第一段文字");
    }

    #[test]
    fn parses_imports_with_explicit_selectors_and_aliases() {
        let parsed = parse("#import super::shared::{format as shared_format, warning}");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let imports = parsed.imports();
        assert_eq!(imports.len(), 1);
        let ExpressionKind::Import { module, selectors } = &imports[0].kind else {
            panic!("expected an import, got {:?}", imports[0].kind)
        };
        assert_eq!(
            module,
            &ModuleReference::Parent {
                levels: 1,
                remainder: vec!["shared".to_owned()],
            }
        );
        assert_eq!(selectors.len(), 2);
        assert_eq!(selectors[0].name, "format");
        assert_eq!(
            selectors[0].alias.as_ref().map(|alias| alias.value.as_str()),
            Some("shared_format")
        );
        assert_eq!(selectors[1].name, "warning");
        assert!(selectors[1].alias.is_none());
        // `path::{...}`: the brace separator does not extend the path.
        let parsed = parse("#import vault::theme::{accent}");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let ExpressionKind::Import { module, .. } = &parsed.imports()[0].kind else {
            panic!()
        };
        assert_eq!(
            module,
            &ModuleReference::Absolute(vec!["theme".to_owned()])
        );
    }

    #[test]
    fn heading_markup_is_not_consumed_by_equality_operators() {
        // R01: the equality operator must not swallow a following line's
        // heading marker — `== 标题` on its own line is heading sugar, never
        // an operand of the preceding embed.
        let parsed = parse("#callout[内容]\n\n== 标题");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        assert_eq!(parsed.root.items.len(), 3);
        assert!(matches!(
            &parsed.root.items[2],
            MarkupItem::Heading(sugar) if sugar.level == 2
        ));
    }

    #[test]
    fn parses_typed_user_functions_and_binary_precedence() {
        // D0007 type surface: scalars, T?, and fn(parameters) -> R. Array/
        // Dict/Union were removed (R07).
        let parse = parse(
            "#let combine(\
             values: fn(x: Int =, y: Int =) -> Int,\
             choice: String? = none,\
             callback: fn() -> Content,\
             ) -> Float? = 1 + 2 * 3",
        );
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        let functions = parse.user_functions();
        assert_eq!(functions.len(), 1);
        let function = functions[0];
        assert_eq!(function.parameters[0].ty, Type::Function);
        assert_eq!(
            function.parameters[1].ty,
            Type::Optional(Box::new(Type::String))
        );
        assert_eq!(function.parameters[2].ty, Type::Function);
        assert_eq!(function.result, Type::Optional(Box::new(Type::Float)));

        // Array/Dict/Union names are no longer types (R07).
        let legacy = crate::parse("#let f(values: Array<Int>) = 1");
        assert!(legacy.errors.iter().any(|error| error.message == "unknown type `Array`"));
        let ExpressionKind::Binary {
            operator: BinaryOperator::Add,
            right,
            ..
        } = &function.body.kind
        else {
            panic!("expected addition at the root")
        };
        assert!(matches!(
            right.kind,
            ExpressionKind::Binary {
                operator: BinaryOperator::Multiply,
                ..
            }
        ));
    }

    fn parse_source<'a>(_parse: &Parse, range: TextRange, source: &'a str) -> &'a str {
        &source[range.start..range.end]
    }
}
