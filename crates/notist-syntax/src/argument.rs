use notist_model::{ModuleReference, TextRange, Type};

use crate::{Call, ContentBlock, SpannedName, SyntaxError};

/// A function argument expression together with an optional parameter name.
#[derive(Clone, Debug, PartialEq)]
pub struct Argument {
    /// The parameter name for a named argument, or `None` for a positional argument.
    pub name: Option<SpannedName>,
    /// The parsed argument expression.
    pub expression: Expression,
    /// The complete argument source range.
    pub range: TextRange,
}

/// A Code-mode expression.
#[derive(Clone, Debug, PartialEq)]
pub struct Expression {
    /// The expression value.
    pub kind: ExpressionKind,
    /// The complete expression source range.
    pub range: TextRange,
}

/// Code expression forms supported by the first Markup/Code implementation.
#[derive(Clone, Debug, PartialEq)]
pub enum ExpressionKind {
    /// The `()` literal: the Unit value.
    Unit,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(StringLiteral),
    Content(ContentBlock),
    Name(SpannedName),
    Call(Box<Call>),
    /// An Array literal: `(a, b)` / `(a,)` / `(,)`; entries may spread Arrays.
    Array(Vec<Expression>),
    /// `..expr` inside a collection literal: splice the evaluated value.
    Spread(Box<Expression>),
    /// A Dict literal: `(k: v)` / `(:)`; entries are `key: value` pairs or
    /// `..expr` spreads of another Dict.
    Dict(Vec<DictEntry>),
    Binary {
        operator: BinaryOperator,
        left: Box<Expression>,
        right: Box<Expression>,
    },
    Unary {
        operator: UnaryOperator,
        operand: Box<Expression>,
    },
    /// A Code block: multiple statements with join semantics (D0006).
    Block(Vec<Expression>),
    /// A general value binding `let name[: T] = expression` (D0007).
    Let {
        name: SpannedName,
        annotation: Option<Type>,
        value: Box<Expression>,
    },
    /// A conditional expression (D0007).
    If {
        condition: Box<Expression>,
        then_branch: Box<Expression>,
        else_branch: Option<Box<Expression>>,
    },
    /// An anonymous function `(params) => expression` (D0007).
    Lambda {
        parameters: Vec<UserParameter>,
        body: Box<Expression>,
    },
    /// A Code import with explicit selectors (D0004: no wildcard).
    Import {
        module: ModuleReference,
        selectors: Vec<ImportSelector>,
    },
    /// A static reference target literal `<path[/label]>`: a vault module
    /// path plus an optional module-local selector. External urls are
    /// rejected by the literal grammar and must use a `String`.
    Target(Box<notist_model::Target>),
    LetFunction(Box<UserFunctionDefinition>),
    Parenthesized(Box<Expression>),
    /// A recoverable invalid expression retained in the tree.
    Error,
}

/// One entry of a Dict literal: a `key: value` pair or a `..expr` spread.
#[derive(Clone, Debug, PartialEq)]
pub enum DictEntry {
    /// `..expr`: splice another Dict into the literal.
    Spread(Box<Expression>),
    /// `key: value`; the key expression evaluates to the dict key.
    Entry {
        key: Box<Expression>,
        value: Box<Expression>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOperator {
    Not,
}

/// One explicitly selected root binding of an import (D0004).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportSelector {
    /// The Code name in the target module.
    pub name: String,
    /// The `as` alias that binds the value locally, when renamed.
    pub alias: Option<SpannedName>,
    pub range: TextRange,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UserFunctionDefinition {
    pub name: SpannedName,
    pub parameters: Vec<UserParameter>,
    pub result: Type,
    pub body: Expression,
    pub range: TextRange,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UserParameter {
    pub name: SpannedName,
    pub ty: Type,
    pub default: Option<Expression>,
    pub range: TextRange,
}

/// A quoted string literal with lexical source metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StringLiteral {
    pub value: String,
    pub payload_range: TextRange,
    pub form: StringLiteralForm,
    pub style: StringLiteralStyle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StringLiteralForm {
    Inline,
    Multiline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StringLiteralStyle {
    Escaped,
    Raw { hashes: usize },
}

#[derive(Clone, Copy)]
struct StringDelimiter {
    opening_end: usize,
    form: StringLiteralForm,
    style: StringLiteralStyle,
}

pub(crate) fn parse_string_at(
    source: &str,
    start: usize,
    end: usize,
    errors: &mut Vec<SyntaxError>,
) -> Option<(Expression, usize)> {
    let delimiter = string_delimiter_at(source, start, end)?;
    // An inline string cannot span a line break, so both the scan and the
    // unclosed diagnostic stop at the end of the opening line.
    let scan_end = match delimiter.form {
        StringLiteralForm::Inline => inline_scan_end(source, delimiter.opening_end, end),
        StringLiteralForm::Multiline => end,
    };
    let Some((payload_end, literal_end)) = find_string_close(source, delimiter, scan_end) else {
        errors.push(SyntaxError {
            message: format!(
                "unclosed {} string literal",
                match (delimiter.style, delimiter.form) {
                    (StringLiteralStyle::Escaped, StringLiteralForm::Inline) => "escaped",
                    (StringLiteralStyle::Escaped, StringLiteralForm::Multiline) => {
                        "escaped multiline"
                    }
                    (StringLiteralStyle::Raw { .. }, StringLiteralForm::Inline) => "raw",
                    (StringLiteralStyle::Raw { .. }, StringLiteralForm::Multiline) => {
                        "raw multiline"
                    }
                }
            ),
            range: TextRange::new(start, scan_end),
        });
        return Some((
            Expression {
                kind: ExpressionKind::Error,
                range: TextRange::new(start, end),
            },
            end,
        ));
    };
    let payload_range = trim_multiline_framing_newlines(
        source,
        TextRange::new(delimiter.opening_end, payload_end),
        delimiter.form,
    );
    let value = match delimiter.style {
        StringLiteralStyle::Escaped => decode_escaped(source, payload_range, errors),
        StringLiteralStyle::Raw { .. } => source[payload_range.start..payload_range.end].to_owned(),
    };
    Some((
        Expression {
            kind: ExpressionKind::String(StringLiteral {
                value,
                payload_range,
                form: delimiter.form,
                style: delimiter.style,
            }),
            range: TextRange::new(start, literal_end),
        },
        literal_end,
    ))
}

pub(crate) fn string_literal_range_at(source: &str, start: usize) -> Option<TextRange> {
    let delimiter = string_delimiter_at(source, start, source.len())?;
    let (_, end) = find_string_close(source, delimiter, source.len())?;
    Some(TextRange::new(start, end))
}

fn string_delimiter_at(source: &str, start: usize, end: usize) -> Option<StringDelimiter> {
    let bytes = source.as_bytes();
    if start + 3 <= end
        && bytes.get(start..start + 3) == Some(b"\"\"\"")
        && line_break_at(bytes, start + 3, end)
    {
        return Some(StringDelimiter {
            opening_end: start + 3,
            form: StringLiteralForm::Multiline,
            style: StringLiteralStyle::Escaped,
        });
    }
    if bytes.get(start) == Some(&b'"') {
        return Some(StringDelimiter {
            opening_end: start + 1,
            form: StringLiteralForm::Inline,
            style: StringLiteralStyle::Escaped,
        });
    }
    if bytes.get(start) != Some(&b'r') {
        return None;
    }

    let mut cursor = start + 1;
    while cursor < end && bytes.get(cursor) == Some(&b'#') {
        cursor += 1;
    }
    let hashes = cursor - start - 1;
    if hashes == 0 {
        return None;
    }
    let (form, quotes) = if cursor + 3 <= end
        && bytes.get(cursor..cursor + 3) == Some(b"\"\"\"")
        && line_break_at(bytes, cursor + 3, end)
    {
        (StringLiteralForm::Multiline, 3)
    } else if bytes.get(cursor) == Some(&b'"') {
        (StringLiteralForm::Inline, 1)
    } else {
        return None;
    };
    Some(StringDelimiter {
        opening_end: cursor + quotes,
        form,
        style: StringLiteralStyle::Raw { hashes },
    })
}

fn line_break_at(bytes: &[u8], cursor: usize, end: usize) -> bool {
    (cursor < end && bytes.get(cursor) == Some(&b'\n'))
        || (cursor + 2 <= end && bytes.get(cursor..cursor + 2) == Some(b"\r\n"))
}

fn inline_scan_end(source: &str, from: usize, end: usize) -> usize {
    let bytes = source.as_bytes();
    let mut cursor = from;
    while cursor < end {
        match bytes[cursor] {
            b'\r' | b'\n' => return cursor,
            _ => cursor += 1,
        }
    }
    end
}

fn find_string_close(
    source: &str,
    delimiter: StringDelimiter,
    end: usize,
) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    let quote_count = match delimiter.form {
        StringLiteralForm::Inline => 1,
        StringLiteralForm::Multiline => 3,
    };
    let mut cursor = delimiter.opening_end;

    while cursor < end {
        if delimiter.form == StringLiteralForm::Inline
            && matches!(bytes.get(cursor), Some(b'\r' | b'\n'))
        {
            return None;
        }
        match delimiter.style {
            StringLiteralStyle::Escaped => {
                if bytes.get(cursor) == Some(&b'\\') {
                    cursor += 1;
                    if cursor < end {
                        cursor += source[cursor..end].chars().next()?.len_utf8();
                    }
                    continue;
                }
                if bytes
                    .get(cursor..cursor + quote_count)
                    .is_some_and(|quotes| quotes.iter().all(|byte| *byte == b'"'))
                {
                    return Some((cursor, cursor + quote_count));
                }
            }
            StringLiteralStyle::Raw { hashes } => {
                if bytes
                    .get(cursor..cursor + quote_count)
                    .is_some_and(|quotes| quotes.iter().all(|byte| *byte == b'"'))
                {
                    let hashes_start = cursor + quote_count;
                    let hashes_end = hashes_start + hashes;
                    if bytes
                        .get(hashes_start..hashes_end)
                        .is_some_and(|suffix| suffix.iter().all(|byte| *byte == b'#'))
                        && bytes.get(hashes_end) != Some(&b'#')
                    {
                        return Some((cursor, hashes_end));
                    }
                }
            }
        }
        cursor += source[cursor..end].chars().next()?.len_utf8();
    }
    None
}

fn decode_escaped(source: &str, range: TextRange, errors: &mut Vec<SyntaxError>) -> String {
    let mut value = String::new();
    let mut cursor = range.start;
    while cursor < range.end {
        let character = source[cursor..range.end]
            .chars()
            .next()
            .expect("cursor is within a UTF-8 boundary");
        cursor += character.len_utf8();
        if character != '\\' {
            value.push(character);
            continue;
        }
        let escape_start = cursor - 1;
        let Some(escaped) = source[cursor..range.end].chars().next() else {
            errors.push(SyntaxError {
                message: "incomplete string escape".into(),
                range: TextRange::new(escape_start, cursor),
            });
            value.push('\\');
            break;
        };
        cursor += escaped.len_utf8();
        match escaped {
            '"' => value.push('"'),
            '\\' => value.push('\\'),
            'n' => value.push('\n'),
            'r' => value.push('\r'),
            't' => value.push('\t'),
            _ => {
                errors.push(SyntaxError {
                    message: format!("unsupported string escape `\\{escaped}`"),
                    range: TextRange::new(escape_start, cursor),
                });
                value.push(escaped);
            }
        }
    }
    value
}

fn trim_multiline_framing_newlines(
    source: &str,
    range: TextRange,
    form: StringLiteralForm,
) -> TextRange {
    if form == StringLiteralForm::Inline {
        return range;
    }
    let bytes = source.as_bytes();
    let start = if bytes.get(range.start..range.start + 2) == Some(b"\r\n") {
        range.start + 2
    } else if bytes.get(range.start) == Some(&b'\n') {
        range.start + 1
    } else {
        range.start
    };
    let end = if range.end >= start + 2 && bytes.get(range.end - 2..range.end) == Some(b"\r\n") {
        range.end - 2
    } else if range.end > start && bytes.get(range.end - 1) == Some(&b'\n') {
        range.end - 1
    } else {
        range.end
    };
    TextRange::new(start, end)
}
