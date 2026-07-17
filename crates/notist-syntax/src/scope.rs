use notist_model::TextRange;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributeValue {
    pub raw: String,
    pub range: TextRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpannedName {
    pub value: String,
    pub range: TextRange,
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
