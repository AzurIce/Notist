use notist_model::TextRange;

use crate::{Argument, RawLiteral, SyntaxError};

/// A `#[...]` scope whose body is parsed as ordinary Notist content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransparentScope {
    /// The body range without the `#[` and `]` delimiters.
    pub body_range: TextRange,
    /// The postfix attributes attached after the closing delimiter.
    pub attributes: Attributes,
    /// The complete source range of the scope.
    pub range: TextRange,
}

/// A function-style content-producing call.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    /// The qualified function name following `#`.
    pub name: SpannedName,
    /// The argument contents without the surrounding parentheses.
    pub arguments_range: Option<TextRange>,
    /// Parsed argument expressions in source order.
    pub arguments: Vec<Argument>,
    /// The optional trailing Content body.
    pub body: Option<ContentBody>,
    /// The postfix attributes attached to the call result.
    pub attributes: Attributes,
    /// The complete source range of the call.
    pub range: TextRange,
}

/// A trailing `[...]` body parsed recursively as Notist Content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContentBody {
    /// The body payload without the surrounding brackets or framing newlines.
    pub payload_range: TextRange,
    /// Whether the body opener is followed immediately by a newline.
    pub form: BodyForm,
}

/// The source form of a call body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodyForm {
    /// The opening bracket is followed directly by body content.
    Inline,
    /// The opening bracket is followed immediately by a newline.
    Block,
}

/// Postfix metadata introduced by `@` after a scope.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Attributes {
    /// The optional first bare identifier used as the scope ID.
    pub id: Option<SpannedName>,
    /// Ordered tag, class, and key-value attributes.
    pub items: Vec<Attribute>,
    /// The complete attribute range, including the leading `@`.
    pub range: Option<TextRange>,
}

/// A single non-ID postfix attribute.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Attribute {
    /// A semantic tag written as `#name`.
    Tag(SpannedName),
    /// A presentation class written as `.name`.
    Class(SpannedName),
    /// A structured property written as `key=value`.
    KeyValue {
        /// The property key.
        key: SpannedName,
        /// The raw property value.
        value: AttributeValue,
        /// The complete key-value source range.
        range: TextRange,
    },
}

impl Attribute {
    /// Returns the complete source range of the attribute.
    pub fn range(&self) -> TextRange {
        match self {
            Self::Tag(name) | Self::Class(name) => name.range,
            Self::KeyValue { range, .. } => *range,
        }
    }
}

/// A raw attribute value together with its source range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributeValue {
    /// The value exactly as written, including quotes when present.
    pub raw: String,
    /// The source range of the raw value.
    pub range: TextRange,
}

/// A parsed name that retains its original source range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpannedName {
    /// The normalized name text.
    pub value: String,
    /// The source range occupied by the name.
    pub range: TextRange,
}

pub(crate) fn parse_scopes_and_calls(
    source: &str,
    raw_literals: &[RawLiteral],
    errors: &mut Vec<SyntaxError>,
) -> (Vec<TransparentScope>, Vec<Call>) {
    let mut scopes = Vec::new();
    let mut calls = Vec::new();
    parse_regions_in(
        source,
        TextRange::new(0, source.len()),
        raw_literals,
        errors,
        &mut scopes,
        &mut calls,
    );
    (scopes, calls)
}

fn parse_regions_in(
    source: &str,
    search_range: TextRange,
    raw_literals: &[RawLiteral],
    errors: &mut Vec<SyntaxError>,
    scopes: &mut Vec<TransparentScope>,
    calls: &mut Vec<Call>,
) {
    let bytes = source.as_bytes();
    let mut cursor = search_range.start;

    while cursor < search_range.end {
        if let Some(raw) = crate::raw::containing(raw_literals, cursor) {
            cursor = raw.range.end;
            continue;
        }
        if bytes[cursor] != b'#' {
            cursor += 1;
            continue;
        }

        match parse_region_at(source, cursor, raw_literals, errors) {
            Some(Region::Transparent(scope)) => {
                if scope.range.end > search_range.end {
                    cursor += 1;
                    continue;
                }
                let body_range = scope.body_range;
                let end = scope.range.end;
                scopes.push(scope);
                parse_regions_in(source, body_range, raw_literals, errors, scopes, calls);
                cursor = end;
            }
            Some(Region::Call(call)) => {
                if call.range.end > search_range.end {
                    cursor += 1;
                    continue;
                }
                let body_range = call.body.map(|body| body.payload_range);
                let end = call.range.end;
                calls.push(call);
                if let Some(body_range) = body_range {
                    parse_regions_in(source, body_range, raw_literals, errors, scopes, calls);
                }
                cursor = end;
            }
            None => cursor += 1,
        }
    }
}

enum Region {
    Transparent(TransparentScope),
    Call(Call),
}

fn parse_region_at(
    source: &str,
    start: usize,
    raw_literals: &[RawLiteral],
    errors: &mut Vec<SyntaxError>,
) -> Option<Region> {
    let bytes = source.as_bytes();
    let after_hash = start + 1;
    let next = *bytes.get(after_hash)?;

    if next == b'[' {
        let close = match find_matching(source, after_hash, b'[', b']', raw_literals) {
            Some(close) => close,
            None => {
                errors.push(SyntaxError {
                    message: "unclosed transparent scope".into(),
                    range: TextRange::new(start, source.len()),
                });
                return None;
            }
        };
        let body_range = TextRange::new(after_hash + 1, close);
        let (attributes, end) = parse_attributes(source, close + 1, errors);
        return Some(Region::Transparent(TransparentScope {
            body_range,
            attributes,
            range: TextRange::new(start, end),
        }));
    }

    let (name, mut cursor) = parse_qualified_name(source, after_hash)?;
    let mut has_call_syntax = false;
    let (arguments_range, arguments) = if bytes.get(cursor) == Some(&b'(') {
        has_call_syntax = true;
        let close = match find_matching(source, cursor, b'(', b')', raw_literals) {
            Some(close) => close,
            None => {
                errors.push(SyntaxError {
                    message: format!("unclosed argument list for call `{}`", name.value),
                    range: TextRange::new(start, source.len()),
                });
                return None;
            }
        };
        let range = TextRange::new(cursor + 1, close);
        cursor = close + 1;
        let arguments = crate::argument::parse_arguments(source, range, raw_literals, errors);
        (Some(range), arguments)
    } else {
        (None, Vec::new())
    };

    let body = if bytes.get(cursor) == Some(&b'[') {
        has_call_syntax = true;
        let body_form = if bytes.get(cursor + 1) == Some(&b'\n')
            || bytes.get(cursor + 1..cursor + 3) == Some(b"\r\n")
        {
            BodyForm::Block
        } else {
            BodyForm::Inline
        };
        let close = match find_matching(source, cursor, b'[', b']', raw_literals) {
            Some(close) => close,
            None => {
                errors.push(SyntaxError {
                    message: format!("unclosed body for call `{}`", name.value),
                    range: TextRange::new(start, source.len()),
                });
                return None;
            }
        };
        let payload_range = match body_form {
            BodyForm::Inline => TextRange::new(cursor + 1, close),
            BodyForm::Block => block_body_range(source, cursor + 1, close),
        };
        cursor = close + 1;
        Some(ContentBody {
            payload_range,
            form: body_form,
        })
    } else {
        None
    };

    if !has_call_syntax {
        return None;
    }

    let (attributes, end) = parse_attributes(source, cursor, errors);

    Some(Region::Call(Call {
        name,
        arguments_range,
        arguments,
        body,
        attributes,
        range: TextRange::new(start, end),
    }))
}

fn block_body_range(source: &str, start: usize, end: usize) -> TextRange {
    let bytes = source.as_bytes();
    let content_start = if bytes.get(start..start + 2) == Some(b"\r\n") {
        start + 2
    } else if bytes.get(start) == Some(&b'\n') {
        start + 1
    } else {
        start
    };
    let content_end = if end >= content_start + 2 && bytes.get(end - 2..end) == Some(b"\r\n") {
        end - 2
    } else if end > content_start && bytes.get(end - 1) == Some(&b'\n') {
        end - 1
    } else {
        end
    };
    TextRange::new(content_start, content_end)
}

fn parse_attributes(
    source: &str,
    start: usize,
    errors: &mut Vec<SyntaxError>,
) -> (Attributes, usize) {
    if source.as_bytes().get(start) != Some(&b'@') {
        return (Attributes::default(), start);
    }

    let mut attributes = Attributes {
        range: Some(TextRange::new(start, start + 1)),
        ..Attributes::default()
    };
    let mut cursor = start + 1;
    let mut first = true;

    loop {
        let item_start = cursor;
        let parsed = match source.as_bytes().get(cursor) {
            Some(b'#') => parse_prefixed_name(source, cursor, AttributeKind::Tag),
            Some(b'.') => parse_prefixed_name(source, cursor, AttributeKind::Class),
            Some(_) => parse_bare_attribute(source, cursor, first),
            None => None,
        };

        match parsed {
            Some(ParsedAttribute::Id(id, end)) => {
                attributes.id = Some(id);
                cursor = end;
            }
            Some(ParsedAttribute::Item(item, end)) => {
                attributes.items.push(item);
                cursor = end;
            }
            None => {
                errors.push(SyntaxError {
                    message: if first {
                        "expected an ID, tag, class, or key-value attribute after `@`".into()
                    } else {
                        "expected a tag, class, or key-value attribute after `,`".into()
                    },
                    range: TextRange::new(item_start, next_char_end(source, item_start)),
                });
                break;
            }
        }

        first = false;
        if source.as_bytes().get(cursor) != Some(&b',') {
            break;
        }
        cursor += 1;
    }

    attributes.range = Some(TextRange::new(start, cursor));
    (attributes, cursor)
}

enum AttributeKind {
    Tag,
    Class,
}

enum ParsedAttribute {
    Id(SpannedName, usize),
    Item(Attribute, usize),
}

fn parse_prefixed_name(source: &str, start: usize, kind: AttributeKind) -> Option<ParsedAttribute> {
    let name_start = start + 1;
    let (value, end) = parse_identifier(source, name_start)?;
    let name = SpannedName {
        value,
        range: TextRange::new(start, end),
    };
    let item = match kind {
        AttributeKind::Tag => Attribute::Tag(name),
        AttributeKind::Class => Attribute::Class(name),
    };
    Some(ParsedAttribute::Item(item, end))
}

fn parse_bare_attribute(source: &str, start: usize, first: bool) -> Option<ParsedAttribute> {
    let (value, name_end) = parse_identifier(source, start)?;
    let name = SpannedName {
        value,
        range: TextRange::new(start, name_end),
    };

    if source.as_bytes().get(name_end) == Some(&b'=') {
        let (value, end) = parse_attribute_value(source, name_end + 1)?;
        let range = TextRange::new(start, end);
        return Some(ParsedAttribute::Item(
            Attribute::KeyValue {
                key: name,
                value,
                range,
            },
            end,
        ));
    }

    first.then_some(ParsedAttribute::Id(name, name_end))
}

fn parse_attribute_value(source: &str, start: usize) -> Option<(AttributeValue, usize)> {
    if source.as_bytes().get(start) == Some(&b'"') {
        let mut cursor = start + 1;
        let mut escaped = false;
        while let Some(&byte) = source.as_bytes().get(cursor) {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                let end = cursor + 1;
                return Some((
                    AttributeValue {
                        raw: source[start..end].to_owned(),
                        range: TextRange::new(start, end),
                    },
                    end,
                ));
            }
            cursor += 1;
        }
        return None;
    }

    let (_, end) = parse_identifier(source, start)?;
    Some((
        AttributeValue {
            raw: source[start..end].to_owned(),
            range: TextRange::new(start, end),
        },
        end,
    ))
}

fn parse_qualified_name(source: &str, start: usize) -> Option<(SpannedName, usize)> {
    let (_, mut end) = parse_identifier(source, start)?;
    loop {
        if source.as_bytes().get(end..end + 2) != Some(b"::") {
            break;
        }
        let (_, segment_end) = parse_identifier(source, end + 2)?;
        end = segment_end;
    }

    Some((
        SpannedName {
            value: source[start..end].to_owned(),
            range: TextRange::new(start, end),
        },
        end,
    ))
}

fn parse_identifier(source: &str, start: usize) -> Option<(String, usize)> {
    let tail = source.get(start..)?;
    let mut end = start;
    for (offset, character) in tail.char_indices() {
        if character.is_alphanumeric() || matches!(character, '_' | '-') {
            end = start + offset + character.len_utf8();
        } else {
            break;
        }
    }
    (end > start).then(|| (source[start..end].to_owned(), end))
}

fn find_matching(
    source: &str,
    open: usize,
    opening: u8,
    closing: u8,
    raw_literals: &[RawLiteral],
) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut cursor = open;
    let mut in_string = false;
    let mut escaped = false;

    while let Some(&byte) = bytes.get(cursor) {
        if cursor != open
            && let Some(raw) = crate::raw::containing(raw_literals, cursor)
        {
            cursor = raw.range.end;
            continue;
        }
        if cursor != open
            && matches!(byte, b'"' | b'r')
            && let Some(string) = crate::argument::string_literal_range_at(source, cursor)
        {
            cursor = string.end;
            continue;
        }
        if escaped {
            escaped = false;
            cursor += 1;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
            cursor += 1;
            continue;
        }
        if byte == b'"' {
            in_string = !in_string;
            cursor += 1;
            continue;
        }
        if !in_string {
            if byte == opening {
                depth += 1;
            } else if byte == closing {
                depth -= 1;
                if depth == 0 {
                    return Some(cursor);
                }
            }
        }
        cursor += 1;
    }
    None
}

fn next_char_end(source: &str, start: usize) -> usize {
    source
        .get(start..)
        .and_then(|tail| tail.chars().next())
        .map_or(start, |character| start + character.len_utf8())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    #[test]
    fn parses_transparent_scope_and_all_attribute_forms() {
        let source = "#[重要概念]@intro,#concept,#review,.highlight,priority=high,owner=\"Alice\"";
        let parse = parse(source);
        assert!(parse.errors.is_empty());
        let scope = &parse.scopes[0];

        assert_eq!(
            &source[scope.body_range.start..scope.body_range.end],
            "重要概念"
        );
        assert_eq!(scope.attributes.id.as_ref().unwrap().value, "intro");
        assert_eq!(scope.attributes.items.len(), 5);
        assert!(
            matches!(&scope.attributes.items[0], Attribute::Tag(tag) if tag.value == "concept")
        );
        assert!(
            matches!(&scope.attributes.items[2], Attribute::Class(class) if class.value == "highlight")
        );
        assert!(matches!(
            &scope.attributes.items[4],
            Attribute::KeyValue { key, value, .. }
                if key.value == "owner" && value.raw == "\"Alice\""
        ));
        assert_eq!(scope.range.end, source.len());
    }

    #[test]
    fn allows_attributes_without_an_id() {
        for source in [
            "#[content]@#draft,#concept",
            "#[content]@.highlight,.lead",
            "#[content]@status=draft,priority=high",
        ] {
            let parse = parse(source);
            assert!(parse.errors.is_empty(), "{source}: {:?}", parse.errors);
            assert!(parse.scopes[0].attributes.id.is_none());
            assert_eq!(parse.scopes[0].attributes.items.len(), 2);
        }
    }

    #[test]
    fn recursively_parses_content_calls_but_keeps_raw_literals_atomic() {
        let source =
            "#[outer #[inner]@inner #quote[#[nested] [[visible]]] `#[raw] [[ignored]]`]@outer";
        let parse = parse(source);
        assert!(parse.errors.is_empty());
        assert_eq!(parse.scopes.len(), 3);
        assert_eq!(parse.calls.len(), 1);
        assert_eq!(parse.raw_literals.len(), 1);
        assert_eq!(parse.links.len(), 1);
    }

    #[test]
    fn parses_argument_only_calls_and_attributes() {
        let source = "#plugin::code(lang=\"rust\")@example,#snippet,.wide,status=checked";
        let parse = parse(source);
        assert!(parse.errors.is_empty());
        let call = &parse.calls[0];

        assert_eq!(call.name.value, "plugin::code");
        assert!(call.body.is_none());
        let arguments = call.arguments_range.unwrap();
        assert_eq!(&source[arguments.start..arguments.end], "lang=\"rust\"");
        assert_eq!(call.attributes.id.as_ref().unwrap().value, "example");
        assert_eq!(call.attributes.items.len(), 3);
        assert_eq!(call.range.end, source.len());
    }

    #[test]
    fn supports_each_content_call_shape_but_not_bare_names() {
        let source = "#ping() #quote[text] #render(mode=\"full\")[body] #bare";
        let parse = parse(source);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        assert_eq!(
            parse
                .calls
                .iter()
                .map(|call| call.name.value.as_str())
                .collect::<Vec<_>>(),
            ["ping", "quote", "render"]
        );
        assert!(parse.calls[0].body.is_none());
        assert!(parse.calls[1].arguments_range.is_none());
        assert!(parse.calls[1].body.is_some());
        assert!(parse.calls[2].arguments_range.is_some());
        assert!(parse.calls[2].body.is_some());
    }

    #[test]
    fn distinguishes_inline_and_block_call_forms() {
        let inline = parse("#quote[content]");
        let block = parse("#quote[\r\ncontent\r\n]");
        assert_eq!(inline.calls[0].body.unwrap().form, BodyForm::Inline);
        assert_eq!(block.calls[0].body.unwrap().form, BodyForm::Block);
        let body = block.calls[0].body.unwrap();
        assert_eq!(
            &"#quote[\r\ncontent\r\n]"[body.payload_range.start..body.payload_range.end],
            "content"
        );
    }

    #[test]
    fn excludes_block_content_framing_newlines_from_an_empty_body() {
        let source = "#quote[\n]";
        let parse = parse(source);
        assert!(parse.errors.is_empty());
        assert!(parse.calls[0].body.unwrap().payload_range.is_empty());
    }

    #[test]
    fn string_literals_do_not_close_argument_lists_or_content_bodies() {
        let source = "#render(source=r#\") ] [\"#)[before \" ] \" after]";
        let parse = parse(source);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        let call = &parse.calls[0];
        let body = call.body.unwrap();
        assert_eq!(
            &source[body.payload_range.start..body.payload_range.end],
            "before \" ] \" after"
        );
    }

    #[test]
    fn raw_strings_with_backticks_do_not_confuse_argument_matching() {
        let source = r###"#code(value=r#"`"#)"###;
        let raw = crate::raw::parse_raw_literals(source);
        assert!(raw.literals.is_empty());
        assert_eq!(
            crate::argument::string_literal_range_at(source, 12),
            Some(TextRange::new(12, 18))
        );
        assert_eq!(
            find_matching(source, 5, b'(', b')', &raw.literals),
            Some(18)
        );
    }

    #[test]
    fn transparent_ranges_can_cross_host_markup_boundaries() {
        let source = "*前半段 #[后半段* 和普通文本]@selection,#concept 后续";
        let parse = parse(source);
        assert!(parse.errors.is_empty());
        let scope = &parse.scopes[0];
        assert_eq!(
            &source[scope.body_range.start..scope.body_range.end],
            "后半段* 和普通文本"
        );
    }

    #[test]
    fn reports_invalid_attributes_and_unclosed_scopes() {
        let invalid = parse("#[content]@id,bare");
        assert_eq!(invalid.errors.len(), 1);
        assert!(invalid.errors[0].message.contains("after `,`"));

        let unclosed = parse("before #[content");
        assert_eq!(unclosed.errors.len(), 1);
        assert_eq!(unclosed.errors[0].message, "unclosed transparent scope");

        let unclosed_call = parse("#quote[body");
        assert_eq!(unclosed_call.errors.len(), 1);
        assert!(unclosed_call.errors[0].message.contains("unclosed body"));
    }

    #[test]
    fn raw_content_does_not_close_a_scope() {
        let source = "#[before `]` after]@example";
        let parse = parse(source);
        assert!(parse.errors.is_empty());
        let scope = &parse.scopes[0];
        assert_eq!(
            &source[scope.body_range.start..scope.body_range.end],
            "before `]` after"
        );
    }
}
