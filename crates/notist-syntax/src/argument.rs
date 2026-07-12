use notist_model::TextRange;

use crate::{SpannedName, SyntaxError};

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
    /// A quoted string literal after escape processing.
    String(String),
}

pub(crate) fn parse_arguments(
    source: &str,
    range: TextRange,
    errors: &mut Vec<SyntaxError>,
) -> Vec<Argument> {
    let mut parser = ArgumentParser {
        source,
        cursor: range.start,
        end: range.end,
        errors,
    };
    parser.parse()
}

struct ArgumentParser<'a> {
    source: &'a str,
    cursor: usize,
    end: usize,
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
                    self.errors.push(SyntaxError {
                        message: "expected an argument after `,`".into(),
                        range: TextRange::new(self.cursor.saturating_sub(1), self.cursor),
                    });
                }
            }
        }
        arguments
    }

    fn parse_expression(&mut self) -> Option<Expression> {
        match self.peek()? {
            '"' => self.parse_string(),
            '-' | '0'..='9' => self.parse_number(),
            _ => self.parse_keyword(),
        }
    }

    fn parse_string(&mut self) -> Option<Expression> {
        let start = self.cursor;
        self.cursor += 1;
        let mut value = String::new();
        let mut escaped = false;
        while self.cursor < self.end {
            let character = self.peek()?;
            self.cursor += character.len_utf8();
            if escaped {
                match character {
                    '"' => value.push('"'),
                    '\\' => value.push('\\'),
                    'n' => value.push('\n'),
                    'r' => value.push('\r'),
                    't' => value.push('\t'),
                    _ => {
                        self.errors.push(SyntaxError {
                            message: format!("unsupported string escape `\\{character}`"),
                            range: TextRange::new(
                                self.cursor - character.len_utf8() - 1,
                                self.cursor,
                            ),
                        });
                        value.push(character);
                    }
                }
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                return Some(Expression {
                    kind: ExpressionKind::String(value),
                    range: TextRange::new(start, self.cursor),
                });
            } else {
                value.push(character);
            }
        }
        self.errors.push(SyntaxError {
            message: "unclosed string literal".into(),
            range: TextRange::new(start, self.end),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_and_positional_literals() {
        let source = "level=2, true, lang=\"rust\", ratio=-1.5, missing=none";
        let mut errors = Vec::new();
        let arguments = parse_arguments(source, TextRange::new(0, source.len()), &mut errors);
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
            ExpressionKind::String(value) if value == "rust"
        ));
        assert!(matches!(
            arguments[3].expression.kind,
            ExpressionKind::Float(value) if value == -1.5
        ));
        assert!(matches!(arguments[4].expression.kind, ExpressionKind::None));
    }
}
