use notist_model::TextRange;

use crate::SyntaxError;

/// A delimited source region classified by its visibility to the host parser.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Scope {
    /// A scope whose body remains visible to the host Notist parser.
    Transparent(TransparentScope),
    /// A scope whose raw body is owned by a processor and hidden from the host parser.
    Opaque(OpaqueScope),
}

impl Scope {
    /// Returns the full source range, including delimiters and postfix attributes.
    pub fn range(&self) -> TextRange {
        match self {
            Self::Transparent(scope) => scope.range,
            Self::Opaque(scope) => scope.range,
        }
    }

    /// Returns the source range of the scope body without delimiters.
    pub fn body_range(&self) -> TextRange {
        match self {
            Self::Transparent(scope) => scope.body_range,
            Self::Opaque(scope) => scope.body_range,
        }
    }

    /// Returns the postfix attributes attached to the scope.
    pub fn attributes(&self) -> &Attributes {
        match self {
            Self::Transparent(scope) => &scope.attributes,
            Self::Opaque(scope) => &scope.attributes,
        }
    }
}

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

/// A `#processor(...)[...]` scope whose body is opaque to the host parser.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpaqueScope {
    /// The qualified processor name following `#`.
    pub name: SpannedName,
    /// The argument contents without the surrounding parentheses.
    pub arguments_range: Option<TextRange>,
    /// The raw body range without the surrounding brackets.
    pub body_range: TextRange,
    /// The postfix attributes attached to the processor result.
    pub attributes: Attributes,
    /// The complete source range of the scope.
    pub range: TextRange,
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

pub(crate) fn parse_scopes(
    source: &str,
    raw_ranges: &[TextRange],
    errors: &mut Vec<SyntaxError>,
) -> Vec<Scope> {
    let mut scopes = Vec::new();
    parse_scopes_in(
        source,
        TextRange::new(0, source.len()),
        raw_ranges,
        errors,
        &mut scopes,
    );
    scopes
}

fn parse_scopes_in(
    source: &str,
    search_range: TextRange,
    raw_ranges: &[TextRange],
    errors: &mut Vec<SyntaxError>,
    scopes: &mut Vec<Scope>,
) {
    let bytes = source.as_bytes();
    let mut cursor = search_range.start;

    while cursor < search_range.end {
        if let Some(raw) = crate::raw::containing(raw_ranges, cursor) {
            cursor = raw.end;
            continue;
        }
        if bytes[cursor] != b'#' {
            cursor += 1;
            continue;
        }

        match parse_scope_at(source, cursor, raw_ranges, errors) {
            Some(Scope::Transparent(scope)) => {
                if scope.range.end > search_range.end {
                    cursor += 1;
                    continue;
                }
                let body_range = scope.body_range;
                let end = scope.range.end;
                scopes.push(Scope::Transparent(scope));
                parse_scopes_in(source, body_range, raw_ranges, errors, scopes);
                cursor = end;
            }
            Some(Scope::Opaque(scope)) => {
                if scope.range.end > search_range.end {
                    cursor += 1;
                    continue;
                }
                let end = scope.range.end;
                scopes.push(Scope::Opaque(scope));
                cursor = end;
            }
            None => cursor += 1,
        }
    }
}

fn parse_scope_at(
    source: &str,
    start: usize,
    raw_ranges: &[TextRange],
    errors: &mut Vec<SyntaxError>,
) -> Option<Scope> {
    let bytes = source.as_bytes();
    let after_hash = start + 1;
    let next = *bytes.get(after_hash)?;

    if next == b'[' {
        let close = match find_matching(source, after_hash, b'[', b']', raw_ranges) {
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
        return Some(Scope::Transparent(TransparentScope {
            body_range,
            attributes,
            range: TextRange::new(start, end),
        }));
    }

    let (name, mut cursor) = parse_qualified_name(source, after_hash)?;
    let arguments_range = if bytes.get(cursor) == Some(&b'(') {
        let close = match find_matching(source, cursor, b'(', b')', raw_ranges) {
            Some(close) => close,
            None => {
                errors.push(SyntaxError {
                    message: format!("unclosed argument list for processor `{}`", name.value),
                    range: TextRange::new(start, source.len()),
                });
                return None;
            }
        };
        let range = TextRange::new(cursor + 1, close);
        cursor = close + 1;
        Some(range)
    } else {
        None
    };

    if bytes.get(cursor) != Some(&b'[') {
        return None;
    }

    let close = match find_matching(source, cursor, b'[', b']', raw_ranges) {
        Some(close) => close,
        None => {
            errors.push(SyntaxError {
                message: format!("unclosed body for processor `{}`", name.value),
                range: TextRange::new(start, source.len()),
            });
            return None;
        }
    };
    let body_range = TextRange::new(cursor + 1, close);
    let (attributes, end) = parse_attributes(source, close + 1, errors);

    Some(Scope::Opaque(OpaqueScope {
        name,
        arguments_range,
        body_range,
        attributes,
        range: TextRange::new(start, end),
    }))
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
    raw_ranges: &[TextRange],
) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut cursor = open;
    let mut in_string = false;
    let mut escaped = false;

    while let Some(&byte) = bytes.get(cursor) {
        if cursor != open
            && let Some(raw) = crate::raw::containing(raw_ranges, cursor)
        {
            cursor = raw.end;
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
        let Scope::Transparent(scope) = &parse.scopes[0] else {
            panic!("expected transparent scope");
        };

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
            assert!(parse.scopes[0].attributes().id.is_none());
            assert_eq!(parse.scopes[0].attributes().items.len(), 2);
        }
    }

    #[test]
    fn parses_nested_transparent_scopes_but_keeps_opaque_scopes_atomic() {
        let source = "#[outer #[inner]@inner #code[#[raw] [[ignored]]]]@outer";
        let parse = parse(source);
        assert!(parse.errors.is_empty());
        assert_eq!(parse.scopes.len(), 3);
        assert!(matches!(parse.scopes[0], Scope::Transparent(_)));
        assert!(matches!(parse.scopes[1], Scope::Transparent(_)));
        assert!(matches!(parse.scopes[2], Scope::Opaque(_)));
        assert!(parse.links.is_empty());
    }

    #[test]
    fn parses_opaque_scope_name_arguments_raw_body_and_attributes() {
        let source =
            "#plugin::code(lang=\"rust\")[[1, 2, 3]]@example,#snippet,.wide,status=checked";
        let parse = parse(source);
        assert!(parse.errors.is_empty());
        let Scope::Opaque(scope) = &parse.scopes[0] else {
            panic!("expected opaque scope");
        };

        assert_eq!(scope.name.value, "plugin::code");
        let arguments = scope.arguments_range.unwrap();
        assert_eq!(&source[arguments.start..arguments.end], "lang=\"rust\"");
        assert_eq!(
            &source[scope.body_range.start..scope.body_range.end],
            "[1, 2, 3]"
        );
        assert_eq!(scope.attributes.id.as_ref().unwrap().value, "example");
        assert_eq!(scope.attributes.items.len(), 3);
    }

    #[test]
    fn transparent_ranges_can_cross_host_markup_boundaries() {
        let source = "*前半段 #[后半段* 和普通文本]@selection,#concept 后续";
        let parse = parse(source);
        assert!(parse.errors.is_empty());
        let Scope::Transparent(scope) = &parse.scopes[0] else {
            panic!("expected transparent scope");
        };
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
    }

    #[test]
    fn raw_content_does_not_close_a_scope() {
        let source = "#[before `]` after]@example";
        let parse = parse(source);
        assert!(parse.errors.is_empty());
        let Scope::Transparent(scope) = &parse.scopes[0] else {
            panic!("expected transparent scope");
        };
        assert_eq!(
            &source[scope.body_range.start..scope.body_range.end],
            "before `]` after"
        );
    }
}
