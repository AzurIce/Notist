use notist_model::TextRange;

use crate::argument::{ExpressionKind, StringLiteral, parse_string_at};
use crate::SyntaxError;

/// The source form of a Content block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodyForm {
    Inline,
    Block,
}

/// Postfix source metadata introduced by `@`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Attributes {
    pub id: Option<SpannedName>,
    pub items: Vec<Attribute>,
    pub range: Option<TextRange>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Attribute {
    Tag(SpannedName),
    Class(SpannedName),
    KeyValue {
        key: SpannedName,
        value: AttributeValue,
        range: TextRange,
    },
}

impl Attribute {
    pub fn range(&self) -> TextRange {
        match self {
            Self::Tag(name) | Self::Class(name) => name.range,
            Self::KeyValue { range, .. } => *range,
        }
    }
}

/// A `key = value` attribute value: a bare identifier, or a string literal
/// sharing the Code string grammar (inline `"..."` cannot span a line break;
/// multiline uses `"""`; raw forms use `r#"..."#`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributeValue {
    /// The value source text, including string delimiters when quoted.
    pub raw: String,
    /// The decoded literal when the value is a string; `None` for bare
    /// identifiers and for retained-but-invalid literals.
    pub string: Option<StringLiteral>,
    pub range: TextRange,
}

impl AttributeValue {
    /// The user-facing value: decoded string content for quoted literals,
    /// the bare token source text otherwise.
    pub fn text(&self) -> &str {
        match &self.string {
            Some(literal) => literal.value.as_str(),
            None => &self.raw,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpannedName {
    pub value: String,
    pub range: TextRange,
}

/// Parses a bracket-delimited attribute list starting at the opening `[`
/// (`@[...]` block-prefix and `@![...]` module annotations, D0006). The
/// attribute list is the same grammar as the postfix form — an optional id,
/// `#tag`, `.class`, and `key = value` entries separated by `,` — except
/// that whitespace around entries (including around `=`) may span lines,
/// which the line-bound postfix form cannot. Returns the attributes, the
/// cursor after the closing `]`, and whether the list closed.
pub(crate) fn parse_annotation_block(
    source: &str,
    start: usize,
    errors: &mut Vec<SyntaxError>,
) -> (Attributes, TextRange, bool) {
    let mut attributes = Attributes {
        range: Some(TextRange::new(start, start + 1)),
        ..Attributes::default()
    };
    let mut cursor = start + 1;
    let mut first = true;
    let mut closed = false;

    loop {
        while matches!(
            source.as_bytes().get(cursor),
            Some(b' ' | b'\t' | b'\r' | b'\n')
        ) {
            cursor += 1;
        }
        if source.as_bytes().get(cursor) == Some(&b']') {
            cursor += 1;
            closed = true;
            break;
        }
        let item_start = cursor;
        let parsed = match source.as_bytes().get(cursor) {
            Some(b'#') => parse_prefixed_name(source, cursor, AttributeKind::Tag),
            Some(b'.') => parse_prefixed_name(source, cursor, AttributeKind::Class),
            Some(_) => parse_bare_attribute(source, cursor, first, errors, true),
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
                        "expected an ID, tag, class, or key-value attribute in `@[...]`".into()
                    } else {
                        "expected a tag, class, or key-value attribute after `,`".into()
                    },
                    range: TextRange::new(item_start, next_char_end(source, item_start)),
                });
                cursor = next_char_end(source, item_start);
                break;
            }
        }
        first = false;
        while matches!(
            source.as_bytes().get(cursor),
            Some(b' ' | b'\t' | b'\r' | b'\n')
        ) {
            cursor += 1;
        }
        match source.as_bytes().get(cursor) {
            Some(b']') => {
                cursor += 1;
                closed = true;
                break;
            }
            Some(b',') => cursor += 1,
            _ => {
                errors.push(SyntaxError {
                    message: "expected `,` or `]` in annotation block".into(),
                    range: TextRange::new(cursor, next_char_end(source, cursor)),
                });
                break;
            }
        }
    }

    let range = TextRange::new(start, cursor);
    attributes.range = Some(range);
    (attributes, range, closed)
}

pub(crate) fn parse_attributes(
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
            // The postfix form is line-bound: reaching across a line end
            // would read the next line's heading `=` as a key-value
            // separator (e.g. `#[x]@anchor` above `== 标题`).
            Some(_) => parse_bare_attribute(source, cursor, first, errors, false),
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

/// Parses one bare attribute entry: a leading id or `key = value`. Inside
/// `]`-delimited annotation blocks whitespace around `=` may span lines
/// (D0006); the postfix form passes `allow_line_break = false` so the scan
/// can never reach past the line end.
fn parse_bare_attribute(
    source: &str,
    start: usize,
    first: bool,
    errors: &mut Vec<SyntaxError>,
    allow_line_break: bool,
) -> Option<ParsedAttribute> {
    let (value, name_end) = parse_identifier(source, start)?;
    let name = SpannedName {
        value,
        range: TextRange::new(start, name_end),
    };

    // `key = value`: whitespace is allowed around the `=` (D0006).
    let cursor = skip_attribute_whitespace(source, name_end, allow_line_break);
    if source.as_bytes().get(cursor) == Some(&b'=') {
        let value_start = skip_attribute_whitespace(source, cursor + 1, allow_line_break);
        let (value, end) = parse_attribute_value(source, value_start, errors)?;
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

/// Skips whitespace around `=` in an attribute entry; line breaks only when
/// the enclosing form allows them (see `parse_bare_attribute`).
fn skip_attribute_whitespace(source: &str, mut cursor: usize, allow_line_break: bool) -> usize {
    while matches!(source.as_bytes().get(cursor), Some(b' ' | b'\t'))
        || (allow_line_break && matches!(source.as_bytes().get(cursor), Some(b'\r' | b'\n')))
    {
        cursor += 1;
    }
    cursor
}

fn parse_attribute_value(
    source: &str,
    start: usize,
    errors: &mut Vec<SyntaxError>,
) -> Option<(AttributeValue, usize)> {
    if let Some((expression, end)) = parse_string_at(source, start, source.len(), errors) {
        let string = match expression.kind {
            ExpressionKind::String(literal) => Some(literal),
            // Unclosed literal: the shared lexer already reported the error
            // and the value is retained as recoverable source text.
            _ => None,
        };
        let range = TextRange::new(start, end);
        let raw = source[start..end].to_owned();
        return Some((AttributeValue { raw, string, range }, end));
    }

    let (_, end) = parse_identifier(source, start)?;
    Some((
        AttributeValue {
            raw: source[start..end].to_owned(),
            string: None,
            range: TextRange::new(start, end),
        },
        end,
    ))
}

pub(crate) fn parse_qualified_name(source: &str, start: usize) -> Option<(SpannedName, usize)> {
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

pub(crate) fn parse_identifier(source: &str, start: usize) -> Option<(String, usize)> {
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

fn next_char_end(source: &str, start: usize) -> usize {
    source
        .get(start..)
        .and_then(|tail| tail.chars().next())
        .map_or(start, |character| start + character.len_utf8())
}
