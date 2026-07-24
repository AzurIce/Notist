use notist_model::{TextRange, Type};

use crate::argument::parse_string_at;
use crate::scope::{parse_attributes, parse_identifier, parse_qualified_name};
use crate::{
    Argument, BinaryOperator, BodyForm, Call, ContentBlock, EmbeddedExpression, Expression,
    ExpressionKind, Markup, MarkupItem, Parse, SpannedText, SyntaxError, UserFunctionDefinition,
    UserParameter, WikiLink, parse_wiki_reference,
};

pub(crate) fn parse(source: &str) -> Parse {
    let mut parser = Parser {
        source,
        cursor: 0,
        end: source.len(),
        errors: Vec::new(),
    };
    let root = parser.parse_markup(false).0;
    Parse {
        root,
        errors: parser.errors,
    }
}

struct Parser<'a> {
    source: &'a str,
    cursor: usize,
    end: usize,
    errors: Vec<SyntaxError>,
}

impl Parser<'_> {
    fn parse_markup(&mut self, stop_at_bracket: bool) -> (Markup, bool) {
        let start = self.cursor;
        let mut items = Vec::new();
        let mut text_start = self.cursor;
        let mut bracket_depth = 0usize;

        while self.cursor < self.end {
            if stop_at_bracket && self.byte() == Some(b']') {
                if bracket_depth == 0 {
                    self.push_text(&mut items, text_start, self.cursor);
                    return (
                        Markup {
                            items,
                            range: TextRange::new(start, self.cursor),
                        },
                        true,
                    );
                }
                bracket_depth -= 1;
                self.cursor += 1;
                continue;
            }

            if self.byte() == Some(b'\\') && self.source.as_bytes().get(self.cursor + 1).is_some() {
                self.cursor = self.next_char_end(self.cursor + 1);
                continue;
            }

            if self.source.as_bytes().get(self.cursor..self.cursor + 2) == Some(b"//")
                && !self.inside_http_url()
            {
                self.push_text(&mut items, text_start, self.cursor);
                self.cursor += 2;
                while self.cursor < self.end && !matches!(self.byte(), Some(b'\r' | b'\n')) {
                    self.cursor = self.next_char_end(self.cursor);
                }
                text_start = self.cursor;
                continue;
            }

            if self.source.as_bytes().get(self.cursor..self.cursor + 2) == Some(b"/*")
                && !self.inside_http_url()
            {
                self.push_text(&mut items, text_start, self.cursor);
                let comment_start = self.cursor;
                self.cursor += 2;
                let mut depth = 1usize;
                while self.cursor < self.end && depth > 0 {
                    match self.source.as_bytes().get(self.cursor..self.cursor + 2) {
                        Some(b"/*") => {
                            depth += 1;
                            self.cursor += 2;
                        }
                        Some(b"*/") => {
                            depth -= 1;
                            self.cursor += 2;
                        }
                        _ => self.cursor = self.next_char_end(self.cursor),
                    }
                }
                if depth > 0 {
                    self.errors.push(SyntaxError {
                        message: "unclosed block comment".into(),
                        range: TextRange::new(comment_start, self.end),
                    });
                }
                text_start = self.cursor;
                continue;
            }

            if self.source.as_bytes().get(self.cursor..self.cursor + 2) == Some(b"[[") {
                self.push_text(&mut items, text_start, self.cursor);
                self.parse_wiki_link(&mut items);
                text_start = self.cursor;
                continue;
            }

            match self.byte() {
                Some(b'`') => {
                    self.push_text(&mut items, text_start, self.cursor);
                    let (literal, error) = crate::raw::parse_at(self.source, self.cursor);
                    self.cursor = literal.range.end.max(self.cursor + 1);
                    self.errors.extend(error);
                    items.push(MarkupItem::Raw(literal));
                    text_start = self.cursor;
                }
                Some(b'#') => {
                    self.push_text(&mut items, text_start, self.cursor);
                    let embedded = self.parse_embedded_expression();
                    items.push(MarkupItem::Embedded(embedded));
                    text_start = self.cursor;
                }
                Some(b'[') if stop_at_bracket => {
                    bracket_depth += 1;
                    self.cursor += 1;
                }
                Some(_) => self.cursor = self.next_char_end(self.cursor),
                None => break,
            }
        }

        self.push_text(&mut items, text_start, self.cursor);
        (
            Markup {
                items,
                range: TextRange::new(start, self.cursor),
            },
            !stop_at_bracket,
        )
    }

    fn inside_http_url(&self) -> bool {
        let bytes = self.source.as_bytes();
        let mut start = self.cursor;
        while start > 0 && !bytes[start - 1].is_ascii_whitespace() {
            start -= 1;
        }
        let prefix = &self.source[start..self.cursor];
        matches!(prefix, "http:" | "http:/" | "https:" | "https:/")
            || prefix.starts_with("http://")
            || prefix.starts_with("https://")
    }

    fn parse_wiki_link(&mut self, items: &mut Vec<MarkupItem>) {
        let start = self.cursor;
        let content_start = start + 2;
        let Some(relative_end) = self.source[content_start..self.end].find("]]") else {
            self.errors.push(SyntaxError {
                message: "unclosed wiki reference".into(),
                range: TextRange::new(start, self.end),
            });
            self.cursor = self.end;
            self.push_text(items, start, self.end);
            return;
        };
        let content_end = content_start + relative_end;
        let end = content_end + 2;
        let range = TextRange::new(start, end);
        match parse_wiki_reference(&self.source[content_start..content_end]) {
            Ok(target) => items.push(MarkupItem::Wiki(WikiLink { target, range })),
            Err(message) => {
                self.errors.push(SyntaxError { message, range });
                self.push_text(items, start, end);
            }
        }
        self.cursor = end;
    }

    fn parse_embedded_expression(&mut self) -> EmbeddedExpression {
        let start = self.cursor;
        self.cursor += 1;
        let expression = self.parse_code_expression();
        let expression_end = expression.range.end.max(self.cursor);
        self.cursor = expression_end;
        let (attributes, end) = parse_attributes(self.source, self.cursor, &mut self.errors);
        self.cursor = end;
        EmbeddedExpression {
            expression,
            attributes,
            scope_range: TextRange::new(start, expression_end),
            range: TextRange::new(start, end),
        }
    }

    fn parse_code_expression(&mut self) -> Expression {
        self.parse_binary_expression(0)
    }

    fn parse_binary_expression(&mut self, minimum_precedence: u8) -> Expression {
        let mut left = self.parse_atomic_expression();
        loop {
            let before_whitespace = self.cursor;
            self.skip_whitespace();
            let Some((operator, precedence)) = self.binary_operator() else {
                self.cursor = before_whitespace;
                break;
            };
            if precedence < minimum_precedence {
                self.cursor = before_whitespace;
                break;
            }
            self.cursor += 1;
            self.skip_whitespace();
            let right = self.parse_binary_expression(precedence + 1);
            let range = TextRange::new(left.range.start, right.range.end);
            left = Expression {
                kind: ExpressionKind::Binary {
                    operator,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                range,
            };
        }
        left
    }

    fn binary_operator(&self) -> Option<(BinaryOperator, u8)> {
        match self.byte()? {
            b'+' => Some((BinaryOperator::Add, 1)),
            b'-' => Some((BinaryOperator::Subtract, 1)),
            b'*' => Some((BinaryOperator::Multiply, 2)),
            b'/' => Some((BinaryOperator::Divide, 2)),
            _ => None,
        }
    }

    fn parse_atomic_expression(&mut self) -> Expression {
        let start = self.cursor;
        match self.byte() {
            Some(b'[') => {
                let block = self.parse_content_block();
                Expression {
                    range: block.range,
                    kind: ExpressionKind::Content(block),
                }
            }
            Some(b'"') => self.parse_string_expression(),
            Some(b'r')
                if crate::argument::string_literal_range_at(self.source, start).is_some() =>
            {
                self.parse_string_expression()
            }
            Some(b'-' | b'.' | b'0'..=b'9') => self.parse_number_expression(),
            Some(b'(') => self.parse_parenthesized_expression(),
            Some(b'`') => {
                let (raw, _) = crate::raw::parse_at(self.source, start);
                self.cursor = raw.range.end.min(self.end);
                self.errors.push(SyntaxError {
                    message: "Raw markup is not a Code expression; use a String literal".into(),
                    range: raw.range,
                });
                Expression {
                    kind: ExpressionKind::Error,
                    range: raw.range,
                }
            }
            Some(_) => self.parse_keyword_or_call_expression(),
            None => self.missing_expression(start),
        }
    }

    fn parse_string_expression(&mut self) -> Expression {
        let start = self.cursor;
        match parse_string_at(self.source, start, self.end, &mut self.errors) {
            Some((expression, end)) => {
                self.cursor = end;
                expression
            }
            None => self.invalid_expression(start),
        }
    }

    fn parse_number_expression(&mut self) -> Expression {
        let start = self.cursor;
        if self.byte() == Some(b'-') {
            self.cursor += 1;
        }
        let before = self.consume_ascii_digits();
        let has_dot = self.byte() == Some(b'.');
        if has_dot {
            self.cursor += 1;
        }
        let after = if has_dot {
            self.consume_ascii_digits()
        } else {
            0
        };
        let range = TextRange::new(start, self.cursor);
        let raw = &self.source[start..self.cursor];

        let kind = if has_dot && before + after > 0 {
            raw.parse().ok().map(ExpressionKind::Float)
        } else if !has_dot && before > 0 {
            raw.parse().ok().map(ExpressionKind::Int)
        } else {
            None
        };
        match kind {
            Some(kind) => Expression { kind, range },
            None => {
                self.errors.push(SyntaxError {
                    message: format!("invalid numeric literal `{raw}`"),
                    range,
                });
                Expression {
                    kind: ExpressionKind::Error,
                    range,
                }
            }
        }
    }

    fn parse_parenthesized_expression(&mut self) -> Expression {
        let start = self.cursor;
        self.cursor += 1;
        self.skip_whitespace();
        let inner = self.parse_code_expression();
        self.skip_whitespace();
        if self.byte() == Some(b')') {
            self.cursor += 1;
        } else {
            self.errors.push(SyntaxError {
                message: "unclosed parenthesized expression".into(),
                range: TextRange::new(start, self.cursor),
            });
        }
        Expression {
            kind: ExpressionKind::Parenthesized(Box::new(inner)),
            range: TextRange::new(start, self.cursor),
        }
    }

    fn parse_keyword_or_call_expression(&mut self) -> Expression {
        let start = self.cursor;
        let Some((name, name_end)) = parse_qualified_name(self.source, start) else {
            return self.invalid_expression(start);
        };
        self.cursor = name_end;
        if name.value == "let" {
            return self.parse_user_function(start);
        }
        if !name.value.contains("::") {
            match name.value.as_str() {
                "none" => {
                    return Expression {
                        kind: ExpressionKind::None,
                        range: name.range,
                    };
                }
                "true" => {
                    return Expression {
                        kind: ExpressionKind::Bool(true),
                        range: name.range,
                    };
                }
                "false" => {
                    return Expression {
                        kind: ExpressionKind::Bool(false),
                        range: name.range,
                    };
                }
                _ => {}
            }
        }

        let mut arguments = Vec::new();
        let mut arguments_range = None;
        if self.byte() == Some(b'(') {
            let (range, parsed) = self.parse_arguments();
            arguments_range = Some(range);
            arguments = parsed;
        }
        let mut trailing = Vec::new();
        while self.byte() == Some(b'[') {
            trailing.push(self.parse_content_block());
        }

        if arguments_range.is_none() && trailing.is_empty() {
            return Expression {
                kind: ExpressionKind::Name(name.clone()),
                range: name.range,
            };
        }

        let range = TextRange::new(start, self.cursor);
        Expression {
            kind: ExpressionKind::Call(Box::new(Call {
                name,
                arguments_range,
                arguments,
                trailing,
                range,
            })),
            range,
        }
    }

    fn parse_user_function(&mut self, start: usize) -> Expression {
        self.skip_whitespace();
        let Some((name, end)) = parse_qualified_name(self.source, self.cursor) else {
            return self.invalid_expression(start);
        };
        self.cursor = end;
        self.skip_whitespace();
        let mut parameters = Vec::new();
        if self.byte() != Some(b'(') {
            self.errors.push(SyntaxError {
                message: "expected `(` after user function name".into(),
                range: TextRange::new(self.cursor, self.next_char_end(self.cursor)),
            });
        } else {
            self.cursor += 1;
            loop {
                self.skip_whitespace();
                if self.byte() == Some(b')') {
                    self.cursor += 1;
                    break;
                }
                let parameter_start = self.cursor;
                let Some((parameter_name, parameter_end)) =
                    parse_identifier(self.source, self.cursor)
                else {
                    self.errors.push(SyntaxError {
                        message: "expected parameter name".into(),
                        range: TextRange::new(self.cursor, self.next_char_end(self.cursor)),
                    });
                    break;
                };
                let parameter_name = crate::SpannedName {
                    value: parameter_name,
                    range: TextRange::new(self.cursor, parameter_end),
                };
                self.cursor = parameter_end;
                self.skip_whitespace();
                if self.byte() != Some(b':') {
                    self.errors.push(SyntaxError {
                        message: "expected `:` and an explicit parameter type".into(),
                        range: TextRange::new(self.cursor, self.next_char_end(self.cursor)),
                    });
                    break;
                }
                self.cursor += 1;
                self.skip_whitespace();
                let ty = self.parse_type();
                self.skip_whitespace();
                let default = if self.byte() == Some(b'=') {
                    self.cursor += 1;
                    self.skip_whitespace();
                    Some(self.parse_code_expression())
                } else {
                    None
                };
                let parameter_range = TextRange::new(parameter_start, self.cursor);
                parameters.push(UserParameter {
                    name: parameter_name,
                    ty,
                    default,
                    range: parameter_range,
                });
                self.skip_whitespace();
                match self.byte() {
                    Some(b',') => self.cursor += 1,
                    Some(b')') => {
                        self.cursor += 1;
                        break;
                    }
                    _ => {
                        self.errors.push(SyntaxError {
                            message: "expected `,` or `)` after parameter".into(),
                            range: TextRange::new(self.cursor, self.next_char_end(self.cursor)),
                        });
                        break;
                    }
                }
            }
        }

        self.skip_whitespace();
        if self.source.as_bytes().get(self.cursor..self.cursor + 2) == Some(b"->") {
            self.cursor += 2;
        } else {
            self.errors.push(SyntaxError {
                message: "expected `->` and an explicit result type".into(),
                range: TextRange::new(self.cursor, self.next_char_end(self.cursor)),
            });
        }
        self.skip_whitespace();
        let result = self.parse_type();
        self.skip_whitespace();
        if self.byte() == Some(b'=') {
            self.cursor += 1;
        } else {
            self.errors.push(SyntaxError {
                message: "expected `=` before user function body".into(),
                range: TextRange::new(self.cursor, self.next_char_end(self.cursor)),
            });
        }
        self.skip_whitespace();
        if self.byte() == Some(b'#') {
            self.cursor += 1;
        }
        let body = self.parse_code_expression();
        let range = TextRange::new(start, body.range.end);
        Expression {
            kind: ExpressionKind::LetFunction(Box::new(UserFunctionDefinition {
                name,
                parameters,
                result,
                body,
                range,
            })),
            range,
        }
    }

    fn parse_type(&mut self) -> Type {
        let mut members = vec![self.parse_primary_type()];
        loop {
            self.skip_whitespace();
            if self.byte() != Some(b'|') {
                break;
            }
            self.cursor += 1;
            self.skip_whitespace();
            members.push(self.parse_primary_type());
        }
        let mut ty = if members.len() == 1 {
            members.pop().unwrap()
        } else {
            Type::Union(members)
        };
        if self.byte() == Some(b'?') {
            self.cursor += 1;
            ty = Type::Optional(Box::new(ty));
        }
        ty
    }

    fn parse_primary_type(&mut self) -> Type {
        let start = self.cursor;
        let Some((name, end)) = parse_identifier(self.source, start) else {
            self.errors.push(SyntaxError {
                message: "expected type name".into(),
                range: TextRange::new(start, self.next_char_end(start)),
            });
            return Type::Union(Vec::new());
        };
        self.cursor = end;
        match name.as_str() {
            "None" => Type::None,
            "Bool" => Type::Bool,
            "Int" => Type::Int,
            "Float" => Type::Float,
            "String" => Type::String,
            "Content" => Type::Content,
            "Function" => Type::Function,
            "Array" => {
                let item = self.parse_single_type_argument("Array");
                Type::Array(Box::new(item))
            }
            "Dict" => {
                self.skip_whitespace();
                if self.byte() == Some(b'<') {
                    self.cursor += 1;
                } else {
                    self.errors.push(SyntaxError {
                        message: "expected `<K, V>` after `Dict`".into(),
                        range: TextRange::new(start, self.cursor),
                    });
                }
                self.skip_whitespace();
                let key = self.parse_type();
                self.skip_whitespace();
                if self.byte() == Some(b',') {
                    self.cursor += 1;
                } else {
                    self.errors.push(SyntaxError {
                        message: "expected `,` between Dict key and value types".into(),
                        range: TextRange::new(start, self.cursor),
                    });
                }
                self.skip_whitespace();
                let value = self.parse_type();
                self.skip_whitespace();
                if self.byte() == Some(b'>') {
                    self.cursor += 1;
                } else {
                    self.errors.push(SyntaxError {
                        message: "expected `>` after Dict types".into(),
                        range: TextRange::new(start, self.cursor),
                    });
                }
                Type::Dict(Box::new(key), Box::new(value))
            }
            _ => {
                self.errors.push(SyntaxError {
                    message: format!("unknown type `{name}`"),
                    range: TextRange::new(start, end),
                });
                Type::Union(Vec::new())
            }
        }
    }

    fn parse_single_type_argument(&mut self, owner: &str) -> Type {
        self.skip_whitespace();
        if self.byte() == Some(b'<') {
            self.cursor += 1;
        } else {
            self.errors.push(SyntaxError {
                message: format!("expected `<T>` after `{owner}`"),
                range: TextRange::new(self.cursor, self.next_char_end(self.cursor)),
            });
        }
        self.skip_whitespace();
        let ty = self.parse_type();
        self.skip_whitespace();
        if self.byte() == Some(b'>') {
            self.cursor += 1;
        } else {
            self.errors.push(SyntaxError {
                message: format!("expected `>` after `{owner}` item type"),
                range: TextRange::new(self.cursor, self.next_char_end(self.cursor)),
            });
        }
        ty
    }

    fn parse_arguments(&mut self) -> (TextRange, Vec<Argument>) {
        let open = self.cursor;
        self.cursor += 1;
        let content_start = self.cursor;
        let mut arguments = Vec::new();
        self.skip_whitespace();

        while self.cursor < self.end && self.byte() != Some(b')') {
            let start = self.cursor;
            let checkpoint = self.cursor;
            let possible_name = parse_identifier(self.source, self.cursor).map(|(value, end)| {
                self.cursor = end;
                crate::SpannedName {
                    value,
                    range: TextRange::new(checkpoint, end),
                }
            });
            self.skip_whitespace();
            let name = if possible_name.is_some() && self.byte() == Some(b'=') {
                self.cursor += 1;
                self.skip_whitespace();
                possible_name
            } else {
                self.cursor = checkpoint;
                None
            };

            let expression = if self.byte() == Some(b')') {
                self.missing_expression(self.cursor)
            } else {
                self.parse_code_expression()
            };
            let argument_end = expression.range.end.max(self.cursor);
            arguments.push(Argument {
                name,
                expression,
                range: TextRange::new(start, argument_end),
            });
            self.cursor = argument_end;
            self.skip_whitespace();

            if self.byte() == Some(b',') {
                self.cursor += 1;
                self.skip_whitespace();
                continue;
            }
            if self.byte() != Some(b')') {
                self.errors.push(SyntaxError {
                    message: "expected `,` between function arguments".into(),
                    range: TextRange::new(self.cursor, self.next_char_end(self.cursor)),
                });
                self.recover_argument();
                if self.byte() == Some(b',') {
                    self.cursor += 1;
                    self.skip_whitespace();
                }
            }
        }

        let close = self.cursor;
        if self.byte() == Some(b')') {
            self.cursor += 1;
        } else {
            self.errors.push(SyntaxError {
                message: "unclosed argument list".into(),
                range: TextRange::new(open, self.cursor),
            });
        }
        (TextRange::new(content_start, close), arguments)
    }

    fn parse_content_block(&mut self) -> ContentBlock {
        let start = self.cursor;
        self.cursor += 1;
        let form = if self.source.as_bytes().get(self.cursor..self.cursor + 2) == Some(b"\r\n") {
            self.cursor += 2;
            BodyForm::Block
        } else if self.byte() == Some(b'\n') {
            self.cursor += 1;
            BodyForm::Block
        } else {
            BodyForm::Inline
        };
        let payload_start = self.cursor;
        let (mut markup, closed) = self.parse_markup(true);
        let close = self.cursor;
        let payload_end = if form == BodyForm::Block {
            trim_trailing_framing_newline(self.source, payload_start, close, &mut markup)
        } else {
            close
        };
        markup.range = TextRange::new(payload_start, payload_end);
        if closed {
            self.cursor += 1;
        } else {
            self.errors.push(SyntaxError {
                message: "unclosed Content block".into(),
                range: TextRange::new(start, self.end),
            });
        }
        ContentBlock {
            markup,
            payload_range: TextRange::new(payload_start, payload_end),
            form,
            range: TextRange::new(start, self.cursor),
        }
    }

    fn missing_expression(&mut self, at: usize) -> Expression {
        self.errors.push(SyntaxError {
            message: "expected Code expression".into(),
            range: TextRange::new(at, self.next_char_end(at)),
        });
        Expression {
            kind: ExpressionKind::Error,
            range: TextRange::new(at, at),
        }
    }

    fn invalid_expression(&mut self, start: usize) -> Expression {
        let end = self.next_char_end(start);
        self.errors.push(SyntaxError {
            message: "expected Code expression".into(),
            range: TextRange::new(start, end),
        });
        self.cursor = end;
        Expression {
            kind: ExpressionKind::Error,
            range: TextRange::new(start, end),
        }
    }

    fn push_text(&self, items: &mut Vec<MarkupItem>, start: usize, end: usize) {
        if start < end {
            items.push(MarkupItem::Text(SpannedText {
                value: self.source[start..end].to_owned(),
                range: TextRange::new(start, end),
            }));
        }
    }

    fn recover_argument(&mut self) {
        while self.cursor < self.end && !matches!(self.byte(), Some(b',' | b')')) {
            self.cursor = self.next_char_end(self.cursor);
        }
    }

    fn skip_whitespace(&mut self) {
        while self
            .source
            .get(self.cursor..self.end)
            .and_then(|tail| tail.chars().next())
            .is_some_and(char::is_whitespace)
        {
            self.cursor = self.next_char_end(self.cursor);
        }
    }

    fn consume_ascii_digits(&mut self) -> usize {
        let start = self.cursor;
        while self.byte().is_some_and(|byte| byte.is_ascii_digit()) {
            self.cursor += 1;
        }
        self.cursor - start
    }

    fn byte(&self) -> Option<u8> {
        self.source.as_bytes().get(self.cursor).copied()
    }

    fn next_char_end(&self, start: usize) -> usize {
        self.source
            .get(start..self.end)
            .and_then(|tail| tail.chars().next())
            .map_or(start, |character| start + character.len_utf8())
    }
}

fn trim_trailing_framing_newline(
    source: &str,
    start: usize,
    end: usize,
    markup: &mut Markup,
) -> usize {
    let bytes = source.as_bytes();
    let trimmed = if end >= start + 2 && bytes.get(end - 2..end) == Some(b"\r\n") {
        end - 2
    } else if end > start && bytes.get(end - 1) == Some(&b'\n') {
        end - 1
    } else {
        end
    };
    if trimmed != end
        && let Some(MarkupItem::Text(text)) = markup.items.last_mut()
        && text.range.end == end
    {
        text.range.end = trimmed;
        text.value.truncate(trimmed - text.range.start);
        if text.value.is_empty() {
            markup.items.pop();
        }
    }
    trimmed
}
