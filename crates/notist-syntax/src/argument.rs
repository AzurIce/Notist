use notist_model::TextRange;

use crate::{RawLiteral, SpannedName, SyntaxError};

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

/// An expression currently supported in function argument lists.
#[derive(Clone, Debug, PartialEq)]
pub struct Expression {
    /// The expression value.
    pub kind: ExpressionKind,
    /// The complete expression source range.
    pub range: TextRange,
}

/// The first-stage expression forms supported by the evaluator.
#[derive(Clone, Debug, PartialEq)]
pub enum ExpressionKind {
    /// The `none` literal.
    None,
    /// A boolean literal.
    Bool(bool),
    /// A signed integer literal.
    Int(i64),
    /// A floating-point literal.
    Float(f64),
    /// An escaped or raw quoted string literal.
    String(StringLiteral),
}

/// A quoted string literal with its lexical source metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StringLiteral {
    /// The literal value after escape processing when applicable.
    pub value: String,
    /// The source payload without prefixes or quote delimiters.
    pub payload_range: TextRange,
    /// Whether the literal uses one or three quote characters.
    pub form: StringLiteralForm,
    /// Whether escapes are processed or the payload is preserved verbatim.
    pub style: StringLiteralStyle,
}

/// The quote form of a string literal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StringLiteralForm {
    /// A single-line literal delimited by one quote on each side.
    Inline,
    /// A literal opened by three quotes followed immediately by a line break.
    Multiline,
}

/// The escape behavior and delimiter level of a string literal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StringLiteralStyle {
    /// Backslash escapes are processed.
    Escaped,
    /// The payload is preserved verbatim and delimited by matching hashes.
    Raw { hashes: usize },
}

pub(crate) fn parse_arguments(
    source: &str,
    range: TextRange,
    raw_literals: &[RawLiteral],
    errors: &mut Vec<SyntaxError>,
) -> Vec<Argument> {
    let mut parser = ArgumentParser {
        source,
        cursor: range.start,
        end: range.end,
        raw_literals,
        errors,
    };
    parser.parse()
}

struct ArgumentParser<'a> {
    source: &'a str,
    cursor: usize,
    end: usize,
    raw_literals: &'a [RawLiteral],
    errors: &'a mut Vec<SyntaxError>,
}

impl ArgumentParser<'_> {
    fn parse(&mut self) -> Vec<Argument> {
        let mut arguments = Vec::new();
        self.skip_whitespace();
        while self.cursor < self.end {
            let start = self.cursor;
            let checkpoint = self.cursor;
            let possible_name = self.parse_identifier();
            self.skip_whitespace();
            let name = if possible_name.is_some() && self.peek() == Some('=') {
                self.cursor += 1;
                self.skip_whitespace();
                possible_name
            } else {
                self.cursor = checkpoint;
                None
            };

            let Some(expression) = self.parse_expression() else {
                self.recover_to_comma();
                if self.peek() == Some(',') {
                    self.cursor += 1;
                    self.skip_whitespace();
                    continue;
                }
                break;
            };
            let argument_end = expression.range.end;
            arguments.push(Argument {
                name,
                expression,
                range: TextRange::new(start, argument_end),
            });

            self.skip_whitespace();
            if self.cursor == self.end {
                break;
            }
            if self.peek() != Some(',') {
                self.errors.push(SyntaxError {
                    message: "expected `,` between function arguments".into(),
                    range: TextRange::new(self.cursor, self.next_char_end()),
                });
                self.recover_to_comma();
            }
            if self.peek() == Some(',') {
                self.cursor += 1;
                self.skip_whitespace();
                if self.cursor == self.end {
                    break;
                }
            }
        }
        arguments
    }

    fn parse_expression(&mut self) -> Option<Expression> {
        match self.peek()? {
            '"' => self.parse_string(),
            '`' => self.reject_raw_source(),
            '-' | '0'..='9' => self.parse_number(),
            'r' if string_delimiter_at(self.source, self.cursor, self.end).is_some() => {
                self.parse_string()
            }
            _ => self.parse_keyword(),
        }
    }

    fn parse_string(&mut self) -> Option<Expression> {
        let start = self.cursor;
        let delimiter = string_delimiter_at(self.source, start, self.end)?;
        let payload_start = delimiter.opening_end;
        let Some((payload_end, literal_end)) = find_string_close(self.source, delimiter, self.end)
        else {
            self.cursor = self.end;
            self.errors.push(SyntaxError {
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
                range: TextRange::new(start, self.end),
            });
            return None;
        };
        self.cursor = literal_end;
        let payload_range = trim_multiline_framing_newlines(
            self.source,
            TextRange::new(payload_start, payload_end),
            delimiter.form,
        );
        let value = match delimiter.style {
            StringLiteralStyle::Escaped => decode_escaped(self.source, payload_range, self.errors),
            StringLiteralStyle::Raw { .. } => {
                self.source[payload_range.start..payload_range.end].to_owned()
            }
        };
        Some(Expression {
            kind: ExpressionKind::String(StringLiteral {
                value,
                payload_range,
                form: delimiter.form,
                style: delimiter.style,
            }),
            range: TextRange::new(start, literal_end),
        })
    }

    fn reject_raw_source(&mut self) -> Option<Expression> {
        let start = self.cursor;
        let range = crate::raw::starting_at(self.raw_literals, start)
            .map_or(TextRange::new(start, self.next_char_end()), |literal| {
                literal.range
            });
        self.cursor = range.end.min(self.end);
        self.errors.push(SyntaxError {
            message: "backtick raw literals are not supported in argument expressions; use a string literal".into(),
            range,
        });
        None
    }

    fn parse_number(&mut self) -> Option<Expression> {
        let start = self.cursor;
        if self.peek() == Some('-') {
            self.cursor += 1;
        }
        let mut has_digit = false;
        let mut has_dot = false;
        while let Some(character) = self.peek() {
            if character.is_ascii_digit() {
                has_digit = true;
                self.cursor += 1;
            } else if character == '.' && !has_dot {
                has_dot = true;
                self.cursor += 1;
            } else {
                break;
            }
        }
        let range = TextRange::new(start, self.cursor);
        let raw = &self.source[start..self.cursor];
        let kind = if has_digit && has_dot {
            raw.parse().ok().map(ExpressionKind::Float)
        } else if has_digit {
            raw.parse().ok().map(ExpressionKind::Int)
        } else {
            None
        };
        match kind {
            Some(kind) => Some(Expression { kind, range }),
            None => {
                self.errors.push(SyntaxError {
                    message: format!("invalid numeric literal `{raw}`"),
                    range,
                });
                None
            }
        }
    }

    fn parse_keyword(&mut self) -> Option<Expression> {
        let start = self.cursor;
        let name = self.parse_identifier()?;
        let kind = match name.value.as_str() {
            "none" => ExpressionKind::None,
            "true" => ExpressionKind::Bool(true),
            "false" => ExpressionKind::Bool(false),
            _ => {
                self.errors.push(SyntaxError {
                    message: format!(
                        "unsupported argument expression `{}`; expected a literal",
                        name.value
                    ),
                    range: name.range,
                });
                return None;
            }
        };
        Some(Expression {
            kind,
            range: TextRange::new(start, self.cursor),
        })
    }

    fn parse_identifier(&mut self) -> Option<SpannedName> {
        let start = self.cursor;
        while let Some(character) = self.peek() {
            if character.is_alphanumeric() || matches!(character, '_' | '-') {
                self.cursor += character.len_utf8();
            } else {
                break;
            }
        }
        (self.cursor > start).then(|| SpannedName {
            value: self.source[start..self.cursor].to_owned(),
            range: TextRange::new(start, self.cursor),
        })
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.cursor += self.peek().unwrap().len_utf8();
        }
    }

    fn recover_to_comma(&mut self) {
        while self.cursor < self.end && self.peek() != Some(',') {
            self.cursor += self.peek().unwrap().len_utf8();
        }
    }

    fn peek(&self) -> Option<char> {
        self.source.get(self.cursor..self.end)?.chars().next()
    }

    fn next_char_end(&self) -> usize {
        self.peek()
            .map_or(self.cursor, |character| self.cursor + character.len_utf8())
    }
}

#[derive(Clone, Copy)]
struct StringDelimiter {
    opening_end: usize,
    form: StringLiteralForm,
    style: StringLiteralStyle,
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
    let opening_end = cursor + quotes;
    (opening_end <= end).then_some(StringDelimiter {
        opening_end,
        form,
        style: StringLiteralStyle::Raw { hashes },
    })
}

fn line_break_at(bytes: &[u8], cursor: usize, end: usize) -> bool {
    (cursor < end && bytes.get(cursor) == Some(&b'\n'))
        || (cursor + 2 <= end && bytes.get(cursor..cursor + 2) == Some(b"\r\n"))
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
                cursor += source[cursor..end].chars().next()?.len_utf8();
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
                cursor += source[cursor..end].chars().next()?.len_utf8();
            }
        }
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

pub(crate) fn string_literal_range_at(source: &str, start: usize) -> Option<TextRange> {
    let delimiter = string_delimiter_at(source, start, source.len())?;
    let (_, end) = find_string_close(source, delimiter, source.len())?;
    Some(TextRange::new(start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_and_positional_literals() {
        let source = "level=2, true, lang=\"rust\", ratio=-1.5, missing=none";
        let mut errors = Vec::new();
        let arguments = parse_arguments(source, TextRange::new(0, source.len()), &[], &mut errors);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(arguments.len(), 5);
        assert_eq!(arguments[0].name.as_ref().unwrap().value, "level");
        assert!(matches!(
            arguments[0].expression.kind,
            ExpressionKind::Int(2)
        ));
        assert!(matches!(
            arguments[1].expression.kind,
            ExpressionKind::Bool(true)
        ));
        assert!(matches!(
            &arguments[2].expression.kind,
            ExpressionKind::String(value) if value.value == "rust"
        ));
        assert!(matches!(
            arguments[3].expression.kind,
            ExpressionKind::Float(value) if value == -1.5
        ));
        assert!(matches!(arguments[4].expression.kind, ExpressionKind::None));
    }

    #[test]
    fn allows_trailing_comma_after_whitespace() {
        let source = "value=1,   ";
        let mut errors = Vec::new();
        let arguments = parse_arguments(source, TextRange::new(0, source.len()), &[], &mut errors);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(arguments.len(), 1);
    }

    #[test]
    fn parses_all_string_literal_forms_with_payload_metadata() {
        let source = concat!(
            "escaped=\"line\\n\", ",
            "multiline=\"\"\"\nfirst\nsecond\n\"\"\", ",
            "raw=r#\"line\\n\"#, ",
            "raw_multiline=r##\"\"\"\nfirst \"#\nsecond\n\"\"\"##"
        );
        let mut errors = Vec::new();
        let arguments = parse_arguments(source, TextRange::new(0, source.len()), &[], &mut errors);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(arguments.len(), 4);

        let strings = arguments
            .iter()
            .map(|argument| match &argument.expression.kind {
                ExpressionKind::String(literal) => literal,
                other => panic!("expected string literal, found {other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(strings[0].value, "line\n");
        assert_eq!(strings[0].form, StringLiteralForm::Inline);
        assert_eq!(strings[0].style, StringLiteralStyle::Escaped);
        assert_eq!(
            &source[strings[0].payload_range.start..strings[0].payload_range.end],
            "line\\n"
        );

        assert_eq!(strings[1].value, "first\nsecond");
        assert_eq!(strings[1].form, StringLiteralForm::Multiline);
        assert_eq!(strings[1].style, StringLiteralStyle::Escaped);

        assert_eq!(strings[2].value, "line\\n");
        assert_eq!(strings[2].form, StringLiteralForm::Inline);
        assert_eq!(strings[2].style, StringLiteralStyle::Raw { hashes: 1 });

        assert_eq!(strings[3].value, "first \"#\nsecond");
        assert_eq!(strings[3].form, StringLiteralForm::Multiline);
        assert_eq!(strings[3].style, StringLiteralStyle::Raw { hashes: 2 });
    }

    #[test]
    fn raw_hash_levels_ignore_shorter_closing_sequences() {
        let source = "value=r##\"one \"# two\"##";
        let mut errors = Vec::new();
        let arguments = parse_arguments(source, TextRange::new(0, source.len()), &[], &mut errors);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(matches!(
            &arguments[0].expression.kind,
            ExpressionKind::String(StringLiteral { value, style: StringLiteralStyle::Raw { hashes: 2 }, .. })
                if value == "one \"# two"
        ));
    }

    #[test]
    fn raw_inline_strings_can_start_and_end_with_quotes() {
        let source = concat!(
            "single=r#\"\"\"#, ",
            "pair=r#\"\"\"\"#, ",
            "text=r#\"\"\"abc\"\"\"#"
        );
        let mut errors = Vec::new();
        let arguments = parse_arguments(source, TextRange::new(0, source.len()), &[], &mut errors);
        assert!(errors.is_empty(), "{errors:?}");

        let values = arguments
            .iter()
            .map(|argument| match &argument.expression.kind {
                ExpressionKind::String(literal) => {
                    assert_eq!(literal.form, StringLiteralForm::Inline);
                    assert_eq!(literal.style, StringLiteralStyle::Raw { hashes: 1 });
                    literal.value.as_str()
                }
                other => panic!("expected string literal, found {other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(values, ["\"", "\"\"", "\"\"abc\"\""]);
    }

    #[test]
    fn trims_multiline_framing_newlines_without_dedenting() {
        let source = concat!(
            "escaped=\"\"\"\r\n  first\r\nsecond\r\n\"\"\", ",
            "raw=r#\"\"\"\n  raw\n\"\"\"#,"
        );
        let mut errors = Vec::new();
        let arguments = parse_arguments(source, TextRange::new(0, source.len()), &[], &mut errors);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(arguments.len(), 2);

        let escaped = match &arguments[0].expression.kind {
            ExpressionKind::String(literal) => literal,
            other => panic!("expected string, found {other:?}"),
        };
        assert_eq!(escaped.value, "  first\r\nsecond");
        assert_eq!(
            &source[escaped.payload_range.start..escaped.payload_range.end],
            "  first\r\nsecond"
        );

        let raw = match &arguments[1].expression.kind {
            ExpressionKind::String(literal) => literal,
            other => panic!("expected string, found {other:?}"),
        };
        assert_eq!(raw.value, "  raw");
        assert_eq!(
            &source[raw.payload_range.start..raw.payload_range.end],
            "  raw"
        );
    }

    #[test]
    fn reports_incomplete_escape_after_closing_framing_newline_is_trimmed() {
        let source = "value=\"\"\"\nabc\\\n\"\"\"";
        let mut errors = Vec::new();
        let arguments = parse_arguments(source, TextRange::new(0, source.len()), &[], &mut errors);

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, "incomplete string escape");
        assert!(matches!(
            &arguments[0].expression.kind,
            ExpressionKind::String(literal) if literal.value == "abc\\"
        ));
    }

    #[test]
    fn locates_raw_strings_that_contain_backticks() {
        let source = r###"r#"`"#"###;
        assert_eq!(
            string_literal_range_at(source, 0),
            Some(TextRange::new(0, source.len()))
        );
    }
}
