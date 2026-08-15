use notist_model::{ModuleReference, TextRange, Type};

use crate::argument::parse_string_at;
use crate::scope::{
    parse_annotation_block, parse_attributes, parse_identifier, parse_qualified_name,
};
use crate::{
    Argument, Attributes, BinaryOperator, BlockAnnotation, BodyForm, Call, ContentBlock,
    EmbeddedExpression, Expression, ExpressionKind, HeadingSugar, ListSugar, ListSugarRow, Markup,
    MarkupItem, Parse, SpannedName, SpannedText, SyntaxError, UnaryOperator,
    UserFunctionDefinition, UserParameter, WikiLink, parse_wiki_reference,
};

/// Precedence of unary `not`: tighter than `and`/`or`, looser than comparison,
/// so `not a == b` parses as `not (a == b)` (D0007).
const NOT_PRECEDENCE: u8 = 3;

pub(crate) fn parse(source: &str) -> Parse {
    let mut parser = Parser {
        source,
        cursor: 0,
        end: source.len(),
        errors: Vec::new(),
    };
    let root = parser.parse_markup(false, true).0;
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
    fn parse_markup(&mut self, stop_at_bracket: bool, at_line_start: bool) -> (Markup, bool) {
        let start = self.cursor;
        let mut items = Vec::new();
        let mut text_start = self.cursor;
        let mut bracket_depth = 0usize;
        let mut at_line_start = at_line_start;

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
                at_line_start = false;
                continue;
            }

            if self.byte() == Some(b'\\') && self.source.as_bytes().get(self.cursor + 1).is_some() {
                self.cursor = self.next_char_end(self.cursor + 1);
                at_line_start = false;
                continue;
            }

            if self.source.as_bytes().get(self.cursor..self.cursor + 2) == Some(b"[[") {
                self.push_text(&mut items, text_start, self.cursor);
                self.parse_wiki_link(&mut items);
                text_start = self.cursor;
                at_line_start = false;
                continue;
            }

            // D0003: heading, rule, and list sugar are syntax-frontend nodes
            // recognized at line starts; the evaluator never rescans source.
            if at_line_start {
                let mut consumed = false;
                if self.byte() == Some(b'=') {
                    if let Some((sugar, end)) = self.parse_heading_sugar(stop_at_bracket) {
                        self.push_text(&mut items, text_start, self.cursor);
                        items.push(MarkupItem::Heading(sugar));
                        self.cursor = end;
                        text_start = self.cursor;
                        consumed = true;
                    }
                } else if self.byte() == Some(b'-') {
                    if let Some((range, cursor)) = self.parse_rule_sugar() {
                        self.push_text(&mut items, text_start, self.cursor);
                        items.push(MarkupItem::Rule(range));
                        self.cursor = cursor;
                        text_start = self.cursor;
                        consumed = true;
                    } else if let Some((sugar, end)) = self.parse_list_sugar(stop_at_bracket) {
                        self.push_text(&mut items, text_start, self.cursor);
                        items.push(MarkupItem::List(sugar));
                        self.cursor = end;
                        text_start = self.cursor;
                        consumed = true;
                    }
                } else if self.byte() == Some(b'+')
                    && let Some((sugar, end)) = self.parse_list_sugar(stop_at_bracket)
                {
                    self.push_text(&mut items, text_start, self.cursor);
                    items.push(MarkupItem::List(sugar));
                    self.cursor = end;
                    text_start = self.cursor;
                    consumed = true;
                }
                if consumed {
                    // Sugar consumed its line including the trailing newline,
                    // so the cursor resumes at the start of the next line.
                    at_line_start =
                        self.cursor > 0 && self.source.as_bytes()[self.cursor - 1] == b'\n';
                    continue;
                }
            }

            match self.byte() {
                Some(b'`') => {
                    self.push_text(&mut items, text_start, self.cursor);
                    let (literal, error) = crate::raw::parse_at(self.source, self.cursor);
                    self.cursor = literal.range.end.max(self.cursor + 1);
                    self.errors.extend(error);
                    items.push(MarkupItem::Raw(literal));
                    text_start = self.cursor;
                    at_line_start = false;
                }
                Some(b'#') => {
                    self.push_text(&mut items, text_start, self.cursor);
                    let embedded = self.parse_embedded_expression();
                    items.push(MarkupItem::Embedded(embedded));
                    text_start = self.cursor;
                    at_line_start = false;
                }
                // D0006: `@[...]` block-prefix and `@![...]` module annotations
                // are distinct token sequences at line start; `@` elsewhere is
                // ordinary text (inline postfix is parsed after expressions).
                Some(b'@')
                    if self.annotation_at_line_start()
                        && matches!(
                            self.source.as_bytes().get(self.cursor + 1),
                            Some(b'[' | b'!')
                        ) =>
                {
                    self.push_text(&mut items, text_start, self.cursor);
                    let annotation_start = self.cursor;
                    let module = self.source.as_bytes().get(self.cursor + 1) == Some(&b'!');
                    if module {
                        self.cursor += 2;
                        if !self.only_whitespace_before(annotation_start) {
                            self.errors.push(SyntaxError {
                                message: "module annotation `@![...]` must appear before any content"
                                    .into(),
                                range: TextRange::new(annotation_start, annotation_start + 2),
                            });
                        }
                    } else {
                        self.cursor += 1;
                    }
                    if self.byte() == Some(b'[') {
                        let (attributes, block_range, closed) =
                            parse_annotation_block(self.source, self.cursor, &mut self.errors);
                        self.cursor = block_range.end;
                        if !closed {
                            self.errors.push(SyntaxError {
                                message: "unclosed annotation block".into(),
                                range: block_range,
                            });
                        }
                        let annotation = BlockAnnotation {
                            attributes,
                            range: TextRange::new(annotation_start, self.cursor),
                        };
                        if module {
                            items.push(MarkupItem::ModuleAnnotation(annotation));
                        } else {
                            items.push(MarkupItem::BlockAnnotation(annotation));
                        }
                    } else {
                        self.errors.push(SyntaxError {
                            message: if module {
                                "expected `[` after `@!`".into()
                            } else {
                                "expected `[` after `@`".into()
                            },
                            range: TextRange::new(annotation_start, self.cursor),
                        });
                    }
                    text_start = self.cursor;
                    at_line_start = false;
                }
                // D0006: `{...}` is the Code block form usable bare in Markup;
                // its join value enters the surrounding content.
                Some(b'{') => {
                    self.push_text(&mut items, text_start, self.cursor);
                    let block_start = self.cursor;
                    let expression = self.parse_code_block();
                    items.push(MarkupItem::Embedded(EmbeddedExpression {
                        expression,
                        attributes: Attributes::default(),
                        scope_range: TextRange::new(block_start, self.cursor),
                        range: TextRange::new(block_start, self.cursor),
                    }));
                    text_start = self.cursor;
                    at_line_start = false;
                }
                Some(b'[') if stop_at_bracket => {
                    bracket_depth += 1;
                    self.cursor += 1;
                    at_line_start = false;
                }
                Some(byte) => {
                    at_line_start = match byte {
                        b' ' | b'\t' => at_line_start,
                        b'\n' => true,
                        _ => false,
                    };
                    self.cursor = self.next_char_end(self.cursor);
                }
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

    /// Parses Markup inside `[start, end)` with a nested parser over the
    /// same source; the nested parser's errors merge into this parser's list.
    fn parse_markup_slice(&mut self, start: usize, end: usize, stop_at_bracket: bool) -> Markup {
        let mut nested = Parser {
            source: self.source,
            cursor: start,
            end: end.min(self.end),
            errors: Vec::new(),
        };
        let (markup, _) = nested.parse_markup(stop_at_bracket, false);
        self.errors.append(&mut nested.errors);
        markup
    }

    /// Parses a line-leading `= ...` heading sugar (D0003). Returns the sugar
    /// and the cursor after the heading (the body may stop early at `]`).
    fn parse_heading_sugar(&mut self, stop_at_bracket: bool) -> Option<(HeadingSugar, usize)> {
        let start = self.cursor;
        let rest = &self.source[start..self.end];
        let mut line_end = rest.find('\n').map_or(start + rest.len(), |index| start + index);
        let line = &self.source[start..line_end];
        let line = line.strip_suffix('\r').unwrap_or(line);
        line_end = start + line.len();
        let level = line.bytes().take_while(|byte| *byte == b'=').count();
        if level == 0 {
            return None;
        }
        let after = &line[level..];
        if !after.is_empty() && !matches!(after.as_bytes()[0], b' ' | b'\t') {
            return None; // `=foo` is ordinary text.
        }
        let body_start = start + level + usize::from(!after.is_empty());
        let body = self.parse_markup_slice(body_start, line_end, stop_at_bracket);
        let range = TextRange::new(start, body.range.end.max(start + level));
        let mut end = range.end;
        // The heading consumes its line, including the trailing newline: a
        // lone newline between blocks never becomes paragraph content.
        if self.source.as_bytes().get(end..end + 2) == Some(b"\r\n") {
            end += 2;
        } else if self.source.as_bytes().get(end) == Some(&b'\n') {
            end += 1;
        }
        Some((
            HeadingSugar {
                level: level as u32,
                body,
                range,
            },
            end,
        ))
    }

    /// Parses a line-leading `---` rule sugar (D0003): three or more dashes
    /// followed by nothing but line-end whitespace. Returns the rule range
    /// and the cursor after the line (trailing newline consumed).
    fn parse_rule_sugar(&mut self) -> Option<(TextRange, usize)> {
        let start = self.cursor;
        let rest = &self.source[start..self.end];
        let line_len = rest.find('\n').map_or(rest.len(), |index| index);
        let line = &rest[..line_len];
        let line = line.strip_suffix('\r').unwrap_or(line);
        let run = line.bytes().take_while(|byte| *byte == b'-').count();
        if run < 3 || !line[run..].trim().is_empty() {
            return None;
        }
        let mut cursor = start + line_len;
        if rest.as_bytes().get(line_len) == Some(&b'\n') {
            cursor += 1;
        }
        Some((TextRange::new(start, start + line.len()), cursor))
    }

    /// Parses a contiguous run of `- ` / `+ ` list lines (D0003). Returns the
    /// sugar and the cursor after the run; the trailing newline of the run is
    /// consumed, and a closing `]` ends the run early.
    fn parse_list_sugar(&mut self, stop_at_bracket: bool) -> Option<(ListSugar, usize)> {
        let start = self.cursor;
        let mut rows = Vec::new();
        let mut cursor = start;
        loop {
            let rest = &self.source[cursor..self.end];
            let line_len = rest.find('\n').map_or(rest.len(), |index| index);
            let line = &rest[..line_len];
            let line_trim_cr = line.strip_suffix('\r').unwrap_or(line);
            let indent = line_trim_cr
                .bytes()
                .take_while(|byte| matches!(byte, b' ' | b'\t'))
                .count();
            let trimmed = &line_trim_cr[indent..];
            let (ordered, marker_len) = if let Some(_) = trimmed.strip_prefix("- ") {
                (false, 2)
            } else if let Some(_) = trimmed.strip_prefix("+ ") {
                (true, 2)
            } else {
                break;
            };
            let body_start = cursor + indent + marker_len;
            let row_line_end = cursor + line_trim_cr.len();
            let mut body_end = row_line_end;
            while body_end > body_start
                && matches!(self.source.as_bytes().get(body_end - 1), Some(b' ' | b'\t'))
            {
                body_end -= 1;
            }
            if body_end == body_start {
                break; // `- ` with no content stays text (legacy scan parity).
            }
            let body = self.parse_markup_slice(body_start, body_end, stop_at_bracket);
            let stopped_at_bracket = body.range.end < body_end;
            let body_end_reached = body.range.end;
            rows.push(ListSugarRow {
                indent,
                ordered,
                marker_len,
                body,
                range: TextRange::new(cursor, row_line_end),
            });
            if stopped_at_bracket {
                cursor = body_end_reached;
                break;
            }
            cursor = row_line_end;
            if rest.as_bytes().get(line_len) == Some(&b'\n') {
                cursor += 1;
            } else {
                break;
            }
        }
        if rows.is_empty() {
            return None;
        }
        let end = rows.last().map(|row| row.range.end).unwrap();
        Some((
            ListSugar {
                rows,
                range: TextRange::new(start, end),
            },
            cursor,
        ))
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
        // R01: at the bare embed top level, whitespace terminates the
        // expression and the remainder stays Markup text; binary operators
        // must be adjacent, and complete expressions use parentheses.
        let expression = self.parse_code_expression_top_level();
        let expression_end = expression.range.end.max(self.cursor);
        self.cursor = expression_end;
        // D0001: a `;` terminator is consumed and produces no output.
        if self.byte() == Some(b';') {
            self.cursor += 1;
        }
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
        self.parse_binary_expression(0, false)
    }

    fn parse_code_expression_top_level(&mut self) -> Expression {
        self.parse_binary_expression(0, true)
    }

    fn parse_binary_expression(
        &mut self,
        minimum_precedence: u8,
        top_level: bool,
    ) -> Expression {
        let mut left = if minimum_precedence <= NOT_PRECEDENCE && self.peek_keyword("not") {
            self.parse_not_expression()
        } else {
            self.parse_atomic_expression()
        };
        loop {
            // R01: at the embed top level an operator must directly follow the
            // operand; whitespace ends the expression and stays Markup text.
            if top_level && self.byte().is_some_and(|byte| byte.is_ascii_whitespace()) {
                break;
            }
            let before_whitespace = self.cursor;
            if !top_level {
                self.skip_trivia();
            }
            if minimum_precedence <= NOT_PRECEDENCE && self.peek_keyword("not") {
                let operand = self.parse_not_expression();
                let range = TextRange::new(left.range.start, operand.range.end);
                left = Expression {
                    kind: ExpressionKind::Unary {
                        operator: UnaryOperator::Not,
                        operand: Box::new(operand),
                    },
                    range,
                };
                continue;
            }
            let Some((operator, precedence, length)) = self.binary_operator() else {
                self.cursor = before_whitespace;
                break;
            };
            if precedence < minimum_precedence {
                self.cursor = before_whitespace;
                break;
            }
            self.cursor += length;
            self.skip_trivia();
            let right = self.parse_binary_expression(precedence + 1, false);
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

    /// Parses `not <operand>`; the operand binds at comparison precedence so
    /// that `not a == b` is `not (a == b)` (D0007).
    fn parse_not_expression(&mut self) -> Expression {
        let start = self.cursor;
        self.cursor += 3;
        self.skip_trivia();
        let operand = self.parse_binary_expression(NOT_PRECEDENCE, false);
        Expression {
            kind: ExpressionKind::Unary {
                operator: UnaryOperator::Not,
                operand: Box::new(operand),
            },
            range: TextRange::new(start, self.cursor),
        }
    }

    /// Returns whether `word` starts at the cursor and is followed by an
    /// identifier boundary (so `notable` never reads as `not`).
    fn peek_keyword(&self, word: &str) -> bool {
        let Some(rest) = self.source.get(self.cursor..) else {
            return false;
        };
        let Some(after) = rest.as_bytes().get(word.len()) else {
            return false;
        };
        let boundary = |byte: &u8| {
            byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'-')
        };
        rest.starts_with(word) && !boundary(after)
    }

    fn binary_operator(&self) -> Option<(BinaryOperator, u8, usize)> {
        let rest = self.source.get(self.cursor..)?;
        let after = |length: usize| {
            rest.as_bytes()
                .get(length)
                .is_none_or(|byte| {
                    !(byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'-'))
                })
        };
        if rest.starts_with("or") && after(2) {
            return Some((BinaryOperator::Or, 1, 2));
        }
        if rest.starts_with("and") && after(3) {
            return Some((BinaryOperator::And, 2, 3));
        }
        match self.byte()? {
            b'<' => Some((
                if rest.as_bytes().get(1) == Some(&b'=') {
                    BinaryOperator::LessEqual
                } else {
                    BinaryOperator::Less
                },
                3,
                if rest.as_bytes().get(1) == Some(&b'=') { 2 } else { 1 },
            )),
            b'>' => Some((
                if rest.as_bytes().get(1) == Some(&b'=') {
                    BinaryOperator::GreaterEqual
                } else {
                    BinaryOperator::Greater
                },
                3,
                if rest.as_bytes().get(1) == Some(&b'=') { 2 } else { 1 },
            )),
            b'=' if rest.as_bytes().get(1) == Some(&b'=') => {
                Some((BinaryOperator::Equal, 3, 2))
            }
            b'!' if rest.as_bytes().get(1) == Some(&b'=') => {
                Some((BinaryOperator::NotEqual, 3, 2))
            }
            b'+' => Some((BinaryOperator::Add, 4, 1)),
            b'-' => Some((BinaryOperator::Subtract, 4, 1)),
            b'*' => Some((BinaryOperator::Multiply, 5, 1)),
            b'/' => Some((BinaryOperator::Divide, 5, 1)),
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
            Some(b'{') => self.parse_code_block(),
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
        self.skip_trivia();
        // Lambda lookahead: `(name: Type, ...) => expression` (D0007).
        let checkpoint = self.cursor;
        if let Some(parameters) = self.parse_parameter_list() {
            self.skip_trivia();
            if self.byte() == Some(b'=')
                && self.source.as_bytes().get(self.cursor + 1) == Some(&b'>')
            {
                self.cursor += 2;
                self.skip_trivia();
                let body = self.parse_code_expression();
                let range = TextRange::new(start, body.range.end.max(self.cursor));
                return Expression {
                    kind: ExpressionKind::Lambda {
                        parameters,
                        body: Box::new(body),
                    },
                    range,
                };
            }
            self.cursor = checkpoint;
        }
        let inner = self.parse_code_expression();
        self.skip_trivia();
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

    /// Parses `name: Type [= default], ...` and the closing `)`, returning
    /// `None` (with the cursor rewound) when the parenthesized content is not
    /// a parameter list.
    fn parse_parameter_list(&mut self) -> Option<Vec<UserParameter>> {
        let start = self.cursor;
        let mut parameters = Vec::new();
        loop {
            self.skip_trivia();
            let (parameter_name, parameter_end) =
                parse_identifier(self.source, self.cursor)?;
            let name = crate::SpannedName {
                value: parameter_name,
                range: TextRange::new(self.cursor, parameter_end),
            };
            self.cursor = parameter_end;
            self.skip_trivia();
            if self.byte() != Some(b':') {
                self.cursor = start;
                return None;
            }
            self.cursor += 1;
            self.skip_trivia();
            let ty = self.parse_type();
            self.skip_trivia();
            let mut default = None;
            if self.byte() == Some(b'=') {
                self.cursor += 1;
                self.skip_trivia();
                default = Some(self.parse_code_expression());
                self.skip_trivia();
            }
            let parameter = UserParameter {
                name,
                ty,
                default,
                range: TextRange::new(start, self.cursor),
            };
            parameters.push(parameter);
            self.skip_trivia();
            match self.byte() {
                Some(b',') => {
                    self.cursor += 1;
                }
                Some(b')') => {
                    self.cursor += 1;
                    return Some(parameters);
                }
                _ => {
                    self.cursor = start;
                    return None;
                }
            }
        }
    }

    /// Parses `{ statement; statement }`; statements separate at `;` or
    /// newlines (R06), with greedy expression parsing per statement.
    fn parse_code_block(&mut self) -> Expression {
        let start = self.cursor;
        self.cursor += 1;
        let mut statements = Vec::new();
        loop {
            self.skip_trivia();
            if self.byte() == Some(b'}') {
                self.cursor += 1;
                break;
            }
            if self.byte() == Some(b';') {
                self.cursor += 1;
                continue;
            }
            if self.cursor >= self.end {
                self.errors.push(SyntaxError {
                    message: "unclosed code block".into(),
                    range: TextRange::new(start, self.end),
                });
                break;
            }
            let statement_start = self.cursor;
            let statement = self.parse_code_expression();
            if statement.range.start == statement.range.end && self.cursor == statement_start {
                // No progress: consume one character to avoid an infinite loop.
                self.cursor = self.next_char_end(self.cursor);
            }
            statements.push(statement);
            self.skip_trivia();
            if self.byte() == Some(b';') {
                self.cursor += 1;
            }
        }
        Expression {
            kind: ExpressionKind::Block(statements),
            range: TextRange::new(start, self.cursor),
        }
    }

    /// Parses `let name = expr` / `let name: T = expr`, delegating to the
    /// function-definition sugar when `(` follows the name (D0007).
    fn parse_let(&mut self, start: usize) -> Expression {
        self.skip_trivia();
        let checkpoint = self.cursor;
        let Some((name, name_end)) = parse_qualified_name(self.source, self.cursor) else {
            return self.invalid_expression(start);
        };
        self.cursor = name_end;
        self.skip_trivia();
        if self.byte() == Some(b'(') {
            self.cursor = checkpoint;
            return self.parse_user_function(start);
        }
        let binding = crate::SpannedName {
            value: name.value.clone(),
            range: TextRange::new(checkpoint, name_end),
        };
        let mut annotation = None;
        if self.byte() == Some(b':') {
            self.cursor += 1;
            self.skip_trivia();
            annotation = Some(self.parse_type());
            self.skip_trivia();
        }
        if self.byte() == Some(b'=') {
            self.cursor += 1;
            self.skip_trivia();
        } else {
            self.errors.push(SyntaxError {
                message: "expected `=` in let binding".into(),
                range: TextRange::new(self.cursor, self.next_char_end(self.cursor)),
            });
            return Expression {
                kind: ExpressionKind::Error,
                range: TextRange::new(start, self.cursor),
            };
        }
        let value = self.parse_code_expression();
        let range = TextRange::new(start, value.range.end.max(self.cursor));
        Expression {
            kind: ExpressionKind::Let {
                name: binding,
                annotation,
                value: Box::new(value),
            },
            range,
        }
    }

    /// Parses `if condition { ... } else { ... }` (D0007). Branches are Code
    /// blocks or Content blocks; `else` may be omitted or chain another `if`.
    fn parse_if(&mut self, start: usize) -> Expression {
        self.skip_trivia();
        let condition = self.parse_code_expression();
        self.skip_trivia();
        let then_branch = self.parse_if_branch();
        self.skip_trivia();
        let mut else_branch = None;
        if self.peek_keyword("else") {
            self.cursor += 4;
            self.skip_trivia();
            if self.peek_keyword("if") {
                else_branch = Some(Box::new(self.parse_if(self.cursor)));
            } else {
                else_branch = Some(Box::new(self.parse_if_branch()));
            }
        }
        let range = TextRange::new(start, self.cursor);
        Expression {
            kind: ExpressionKind::If {
                condition: Box::new(condition),
                then_branch: Box::new(then_branch),
                else_branch,
            },
            range,
        }
    }

    fn parse_if_branch(&mut self) -> Expression {
        match self.byte() {
            Some(b'{') => self.parse_code_block(),
            Some(b'[') => {
                let block = self.parse_content_block();
                Expression {
                    range: block.range,
                    kind: ExpressionKind::Content(block),
                }
            }
            _ => self.missing_expression(self.cursor),
        }
    }

    /// Parses `import path::{name, name as alias}` (D0004): an explicit
    /// ModulePath with an explicit selector list, no wildcard.
    fn parse_import(&mut self, start: usize) -> Expression {
        self.skip_trivia();
        let mut segments: Vec<String> = Vec::new();
        let mut segment_ranges = Vec::new();
        loop {
            let segment_start = self.cursor;
            let Some((segment, end)) = parse_identifier(self.source, self.cursor) else {
                return self.invalid_expression(start);
            };
            segments.push(segment);
            segment_ranges.push(TextRange::new(segment_start, end));
            self.cursor = end;
            self.skip_trivia();
            if self.source.get(self.cursor..self.cursor + 2) == Some("::") {
                // `path::{selectors}`: the `::` directly before the selector
                // brace ends the path instead of introducing another segment.
                let mut probe = self.cursor + 2;
                while probe < self.end && self.source.as_bytes()[probe].is_ascii_whitespace() {
                    probe += 1;
                }
                if self.source.as_bytes().get(probe) == Some(&b'{') {
                    self.cursor += 2;
                    break;
                }
                self.cursor += 2;
                self.skip_trivia();
                continue;
            }
            break;
        }
        let module = match segments.first().map(String::as_str) {
            Some("vault") => ModuleReference::Absolute(segments[1..].to_vec()),
            Some("self") => ModuleReference::Relative(segments[1..].to_vec()),
            Some("super") => {
                let levels = segments
                    .iter()
                    .take_while(|segment| segment.as_str() == "super")
                    .count();
                ModuleReference::Parent {
                    levels,
                    remainder: segments[levels..].to_vec(),
                }
            }
            _ => ModuleReference::Relative(segments),
        };
        self.skip_trivia();
        if self.byte() != Some(b'{') {
            self.errors.push(SyntaxError {
                message: "expected `{` selector list after import path".into(),
                range: TextRange::new(self.cursor, self.next_char_end(self.cursor)),
            });
            return Expression {
                kind: ExpressionKind::Error,
                range: TextRange::new(start, self.cursor),
            };
        }
        self.cursor += 1;
        let mut selectors = Vec::new();
        loop {
            self.skip_trivia();
            if self.byte() == Some(b'}') {
                self.cursor += 1;
                break;
            }
            let selector_start = self.cursor;
            let Some((name, name_end)) = parse_identifier(self.source, self.cursor) else {
                self.errors.push(SyntaxError {
                    message: "expected binding name in import selector".into(),
                    range: TextRange::new(self.cursor, self.next_char_end(self.cursor)),
                });
                self.recover_argument();
                if self.byte() == Some(b',') {
                    self.cursor += 1;
                }
                continue;
            };
            self.cursor = name_end;
            self.skip_trivia();
            let mut alias = None;
            if self.peek_keyword("as") {
                self.cursor += 2;
                self.skip_trivia();
                let alias_start = self.cursor;
                let (alias_name, alias_end) = match parse_identifier(self.source, self.cursor)
                {
                    Some(parsed) => parsed,
                    None => {
                        self.errors.push(SyntaxError {
                            message: "expected alias name after `as`".into(),
                            range: TextRange::new(self.cursor, self.next_char_end(self.cursor)),
                        });
                        (String::new(), alias_start)
                    }
                };
                alias = Some(SpannedName {
                    value: alias_name,
                    range: TextRange::new(alias_start, alias_end),
                });
                self.cursor = alias_end;
                self.skip_trivia();
            }
            selectors.push(crate::ImportSelector {
                name,
                alias,
                range: TextRange::new(selector_start, self.cursor),
            });
            self.skip_trivia();
            match self.byte() {
                Some(b',') => {
                    self.cursor += 1;
                }
                Some(b'}') => {
                    self.cursor += 1;
                    break;
                }
                _ => {
                    self.errors.push(SyntaxError {
                        message: "expected `,` or `}` in import selector list".into(),
                        range: TextRange::new(self.cursor, self.next_char_end(self.cursor)),
                    });
                    self.recover_argument();
                    if self.byte() == Some(b',') {
                        self.cursor += 1;
                    }
                }
            }
        }
        Expression {
            kind: ExpressionKind::Import { module, selectors },
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
            return self.parse_let(start);
        }
        if name.value == "if" {
            return self.parse_if(start);
        }
        if name.value == "import" {
            return self.parse_import(start);
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
        self.skip_trivia();
        let Some((name, end)) = parse_qualified_name(self.source, self.cursor) else {
            return self.invalid_expression(start);
        };
        self.cursor = end;
        self.skip_trivia();
        let mut parameters = Vec::new();
        if self.byte() != Some(b'(') {
            self.errors.push(SyntaxError {
                message: "expected `(` after user function name".into(),
                range: TextRange::new(self.cursor, self.next_char_end(self.cursor)),
            });
        } else {
            self.cursor += 1;
            loop {
                self.skip_trivia();
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
                self.skip_trivia();
                if self.byte() != Some(b':') {
                    self.errors.push(SyntaxError {
                        message: "expected `:` and an explicit parameter type".into(),
                        range: TextRange::new(self.cursor, self.next_char_end(self.cursor)),
                    });
                    break;
                }
                self.cursor += 1;
                self.skip_trivia();
                let ty = self.parse_type();
                self.skip_trivia();
                let default = if self.byte() == Some(b'=') {
                    self.cursor += 1;
                    self.skip_trivia();
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
                self.skip_trivia();
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

        self.skip_trivia();
        if self.source.as_bytes().get(self.cursor..self.cursor + 2) == Some(b"->") {
            self.cursor += 2;
        } else {
            self.errors.push(SyntaxError {
                message: "expected `->` and an explicit result type".into(),
                range: TextRange::new(self.cursor, self.next_char_end(self.cursor)),
            });
        }
        self.skip_trivia();
        let result = self.parse_type();
        self.skip_trivia();
        if self.byte() == Some(b'=') {
            self.cursor += 1;
        } else {
            self.errors.push(SyntaxError {
                message: "expected `=` before user function body".into(),
                range: TextRange::new(self.cursor, self.next_char_end(self.cursor)),
            });
        }
        self.skip_trivia();
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
        let mut ty = self.parse_primary_type();
        if self.byte() == Some(b'?') {
            self.cursor += 1;
            // D0002: `(T?)?` folds to `T?`.
            ty = match ty {
                Type::Optional(_) => ty,
                other => Type::Optional(Box::new(other)),
            };
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
            return Type::Inferred;
        };
        self.cursor = end;
        match name.as_str() {
            "None" => Type::None,
            "Bool" => Type::Bool,
            "Int" => Type::Int,
            "Float" => Type::Float,
            "String" => Type::String,
            "Content" => Type::Content,
            "fn" => self.parse_function_type(start),
            _ => {
                self.errors.push(SyntaxError {
                    message: format!("unknown type `{name}`"),
                    range: TextRange::new(start, end),
                });
                Type::Inferred
            }
        }
    }

    /// Parses `fn(parameters) -> R` (D0007). The declared parameter details
    /// are validated and discarded: the concrete signature is carried by
    /// symbol metadata (D0002), so the type value is the bare `Function`
    /// marker.
    fn parse_function_type(&mut self, start: usize) -> Type {
        self.skip_trivia();
        if self.byte() == Some(b'(') {
            self.cursor += 1;
            self.skip_trivia();
            if self.byte() != Some(b')') {
                loop {
                    // An optional `trailing` keyword marks the parameter that
                    // binds trailing Content (D0002).
                    if self.peek_keyword("trailing") {
                        self.cursor = self
                            .next_char_end(self.cursor + "trailing".len());
                        self.skip_trivia();
                    }
                    let Some((_name, name_end)) =
                        parse_identifier(self.source, self.cursor)
                    else {
                        self.errors.push(SyntaxError {
                            message: "expected parameter name in function type".into(),
                            range: TextRange::new(
                                self.cursor,
                                self.next_char_end(self.cursor),
                            ),
                        });
                        self.recover_type_parameter();
                        continue;
                    };
                    self.cursor = name_end;
                    self.skip_trivia();
                    if self.byte() == Some(b':') {
                        self.cursor += 1;
                    } else {
                        self.errors.push(SyntaxError {
                            message: "expected `:` after parameter name".into(),
                            range: TextRange::new(start, self.cursor),
                        });
                    }
                    self.skip_trivia();
                    self.parse_type();
                    self.skip_trivia();
                    // `=` marks the parameter optional; no default value is
                    // written in a function type (D0002).
                    if self.byte() == Some(b'=') {
                        self.cursor += 1;
                        self.skip_trivia();
                    }
                    match self.byte() {
                        Some(b',') => {
                            self.cursor += 1;
                            self.skip_trivia();
                        }
                        Some(b')') => {
                            self.cursor += 1;
                            break;
                        }
                        _ => {
                            self.errors.push(SyntaxError {
                                message: "expected `,` or `)` after function type parameter"
                                    .into(),
                                range: TextRange::new(
                                    self.cursor,
                                    self.next_char_end(self.cursor),
                                ),
                            });
                            self.recover_type_parameter();
                        }
                    }
                }
            } else {
                self.cursor += 1;
            }
        } else {
            self.errors.push(SyntaxError {
                message: "expected `(parameters)` after `fn`".into(),
                range: TextRange::new(self.cursor, self.next_char_end(self.cursor)),
            });
        }
        self.skip_trivia();
        if self.source.as_bytes().get(self.cursor..self.cursor + 2) == Some(b"->") {
            self.cursor += 2;
            self.skip_trivia();
            self.parse_type();
            self.skip_trivia();
        } else {
            self.errors.push(SyntaxError {
                message: "expected `->` and a result type in function type".into(),
                range: TextRange::new(self.cursor, self.next_char_end(self.cursor)),
            });
        }
        Type::Function
    }

    /// Recovers a malformed function-type parameter by scanning to the next
    /// `,` or `)`, consuming the `,` when found.
    fn recover_type_parameter(&mut self) {
        while self.cursor < self.end && !matches!(self.byte(), Some(b',' | b')')) {
            self.cursor = self.next_char_end(self.cursor);
        }
        if self.byte() == Some(b',') {
            self.cursor += 1;
        }
    }

    fn parse_arguments(&mut self) -> (TextRange, Vec<Argument>) {
        let open = self.cursor;
        self.cursor += 1;
        let content_start = self.cursor;
        let mut arguments = Vec::new();
        self.skip_trivia();

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
            self.skip_trivia();
            let name = if possible_name.is_some() && matches!(self.byte(), Some(b'=') | Some(b':')) {
                self.cursor += 1;
                self.skip_trivia();
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
            self.skip_trivia();

            if self.byte() == Some(b',') {
                self.cursor += 1;
                self.skip_trivia();
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
                    self.skip_trivia();
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
        let (mut markup, closed) = self.parse_markup(true, form == BodyForm::Block);
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

    /// Returns whether the cursor sits at a line start: only spaces and
    /// tabs since the previous newline (or since the document start).
    fn annotation_at_line_start(&self) -> bool {
        let prefix = &self.source[..self.cursor];
        match prefix.rfind('\n') {
            Some(index) => prefix[index + 1..].bytes().all(|byte| matches!(byte, b' ' | b'\t')),
            None => prefix.bytes().all(|byte| matches!(byte, b' ' | b'\t')),
        }
    }

    /// Returns whether everything before `position` is whitespace (the
    /// `@![...]` module annotation must precede the first meaningful token).
    fn only_whitespace_before(&self, position: usize) -> bool {
        self.source[..position].bytes().all(|byte| byte.is_ascii_whitespace())
    }

    /// Skips whitespace and comments. D0007 lexical trivia (`//` line
    /// comments and nested `/* ... */` blocks) exists only in Code contexts;
    /// Markup text never strips comments (E09).
    fn skip_trivia(&mut self) {
        loop {
            while self
                .source
                .get(self.cursor..self.end)
                .and_then(|tail| tail.chars().next())
                .is_some_and(char::is_whitespace)
            {
                self.cursor = self.next_char_end(self.cursor);
            }
            if self.source.as_bytes().get(self.cursor..self.cursor + 2) == Some(b"//") {
                self.cursor += 2;
                while self.cursor < self.end && !matches!(self.byte(), Some(b'\r' | b'\n')) {
                    self.cursor = self.next_char_end(self.cursor);
                }
                continue;
            }
            if self.source.as_bytes().get(self.cursor..self.cursor + 2) == Some(b"/*") {
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
                continue;
            }
            break;
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
