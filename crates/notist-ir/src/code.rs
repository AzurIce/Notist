//! `.notc` 前端：纯 code 模式。
//!
//! 模块体是语句序列；`let` 不产内容，值语句产内容。
//! 这里不做脱糖——code 模式与意图层同形，解析出来的就是意图层的语句。

use notist_model::TextRange;

use crate::ir::{Diagnostic, Value};
use crate::plan::{Callee, Expr, Param, Statement};

/// 解析一个 `.notc` 模块。
pub fn parse(source: &str) -> (Vec<Statement>, Vec<Diagnostic>) {
    let mut parser = Parser {
        source,
        cursor: 0,
    };
    let mut diagnostics = Vec::new();
    let statements = parser.parse_statements(None, &mut diagnostics);
    (statements, diagnostics)
}

struct Parser<'a> {
    source: &'a str,
    cursor: usize,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.source[self.cursor..]
    }

    fn done(&self) -> bool {
        self.cursor >= self.source.len()
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.cursor += character.len_utf8();
        Some(character)
    }

    fn eat(&mut self, text: &str) -> bool {
        if self.rest().starts_with(text) {
            self.cursor += text.len();
            true
        } else {
            false
        }
    }

    /// 跳过行内空白。
    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.cursor += 1;
        }
    }

    /// 跳过所有空白，含换行；括号内部使用。
    fn skip_trivia(&mut self) {
        while matches!(self.peek(), Some(character) if character.is_whitespace()) {
            self.bump();
        }
    }

    fn recover_line(&mut self) {
        while matches!(self.peek(), Some(character) if character != '\n') {
            self.bump();
        }
    }

    fn word(&mut self) -> Option<String> {
        let start = self.cursor;
        while matches!(self.peek(), Some(character)
            if character.is_ascii_alphanumeric() || character == '_' || character == '-')
        {
            self.cursor += 1;
        }
        (self.cursor > start).then(|| self.source[start..self.cursor].to_owned())
    }

    fn peek_word(&self) -> Option<&'a str> {
        let rest = self.rest();
        let end = rest
            .find(|character: char| {
                !(character.is_ascii_alphanumeric() || character == '_' || character == '-')
            })
            .unwrap_or(rest.len());
        (end > 0).then(|| &rest[..end])
    }

    /// 解析一段语句序列；`closer` 为 `Some` 时遇到该字符收尾。
    fn parse_statements(
        &mut self,
        closer: Option<char>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Vec<Statement> {
        let mut statements = Vec::new();
        let mut closed = closer.is_none();
        loop {
            self.skip_trivia();
            if self.done() {
                break;
            }
            if let Some(expected) = closer
                && self.peek() == Some(expected)
            {
                self.bump();
                closed = true;
                break;
            }
            let Some(statement) = self.parse_statement(diagnostics) else {
                self.recover_line();
                continue;
            };
            statements.push(statement);
            self.skip_spaces();
            if let Some(expected) = closer
                && self.peek() == Some(expected)
            {
                self.bump();
                closed = true;
                break;
            }
            if !self.done() && self.peek() != Some('\n') {
                let start = self.cursor;
                diagnostics.push(Diagnostic::warn(
                    "unexpected-token",
                    "unexpected token after statement",
                    TextRange::new(start, start + 1),
                ));
                self.recover_line();
            }
        }
        if !closed {
            diagnostics.push(Diagnostic::warn(
                "unclosed-content-block",
                "unclosed content block",
                TextRange::new(self.cursor, self.cursor),
            ));
        }
        statements
    }

    fn parse_statement(&mut self, diagnostics: &mut Vec<Diagnostic>) -> Option<Statement> {
        let start = self.cursor;
        match self.peek_word() {
            Some("let") => {
                self.word();
                self.parse_let(start, diagnostics)
            }
            Some("extern") => {
                self.word();
                self.parse_extern(start, diagnostics)
            }
            _ => {
                let expr = self.parse_expression(diagnostics)?;
                Some(Statement::Value {
                    range: expr.range(),
                    expr,
                })
            }
        }
    }

    fn parse_let(&mut self, start: usize, diagnostics: &mut Vec<Diagnostic>) -> Option<Statement> {
        self.skip_spaces();
        let Some(name) = self.word() else {
            diagnostics.push(Diagnostic::warn(
                "invalid-let",
                "expected a name after `let`",
                TextRange::new(start, self.cursor),
            ));
            return None;
        };
        self.skip_spaces();
        let mut params = Vec::new();
        let mut is_function = false;
        if self.peek() == Some('(') {
            // 空参数表也是函数定义：`let f() = ...` 与 `let x = ...` 不同。
            is_function = true;
            params = self.parse_params(diagnostics)?;
            self.skip_spaces();
        }
        if self.eat("->") {
            self.skip_spaces();
            self.word();
            self.skip_spaces();
        }
        if !self.eat("=") {
            diagnostics.push(Diagnostic::warn(
                "invalid-let",
                format!("`let {name}` expects `=`"),
                TextRange::new(start, self.cursor),
            ));
            return None;
        }
        self.skip_trivia();
        let value = self.parse_expression(diagnostics)?;
        let range = TextRange::new(start, self.cursor);
        if !is_function {
            return Some(Statement::Bind { name, value, range });
        }
        let body = match value {
            Expr::Content { statements, .. } => statements,
            other => vec![Statement::Value {
                range: other.range(),
                expr: other,
            }],
        };
        Some(Statement::Function {
            name,
            params,
            body: Some(body),
            range,
        })
    }

    /// `extern fn name(params) -> Type`：只有声明，没有体。
    fn parse_extern(
        &mut self,
        start: usize,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<Statement> {
        self.skip_spaces();
        let head = self.word().unwrap_or_default();
        if !matches!(head.as_str(), "fn" | "element" | "inline") {
            diagnostics.push(Diagnostic::warn(
                "invalid-declaration",
                format!("expected `fn`, `element` or `inline`, found `{head}`"),
                TextRange::new(start, self.cursor),
            ));
            return None;
        }
        self.skip_spaces();
        let Some(name) = self.word() else {
            diagnostics.push(Diagnostic::warn(
                "invalid-declaration",
                "expected a declaration name",
                TextRange::new(start, self.cursor),
            ));
            return None;
        };
        self.skip_spaces();
        let params = if self.peek() == Some('(') {
            self.parse_params(diagnostics)?
        } else {
            Vec::new()
        };
        self.skip_spaces();
        if !self.eat("->") {
            diagnostics.push(Diagnostic::warn(
                "invalid-declaration",
                format!("declaration `{name}` expects `->`"),
                TextRange::new(start, self.cursor),
            ));
            return None;
        }
        self.skip_spaces();
        self.word();
        Some(Statement::Function {
            name,
            params,
            body: None,
            range: TextRange::new(start, self.cursor),
        })
    }

    fn parse_params(&mut self, diagnostics: &mut Vec<Diagnostic>) -> Option<Vec<Param>> {
        let start = self.cursor;
        self.bump();
        let mut params = Vec::new();
        loop {
            self.skip_trivia();
            if self.peek() == Some(')') {
                self.bump();
                break;
            }
            if self.done() {
                diagnostics.push(Diagnostic::warn(
                    "unclosed-parameter-list",
                    "unclosed parameter list",
                    TextRange::new(start, self.cursor),
                ));
                break;
            }
            let Some(name) = self.word() else {
                diagnostics.push(Diagnostic::warn(
                    "invalid-parameter",
                    "expected a parameter name",
                    TextRange::new(self.cursor, self.cursor + 1),
                ));
                break;
            };
            self.skip_spaces();
            if !self.eat(":") {
                diagnostics.push(Diagnostic::warn(
                    "invalid-parameter",
                    format!("parameter `{name}` expects `name: Type`"),
                    TextRange::new(self.cursor, self.cursor + 1),
                ));
                break;
            }
            self.skip_spaces();
            let ty = self.word().unwrap_or_default();
            params.push(Param { name, ty });
            self.skip_spaces();
            if self.eat(",") {
                continue;
            }
            self.skip_trivia();
            if self.peek() == Some(')') {
                self.bump();
                break;
            }
            diagnostics.push(Diagnostic::warn(
                "invalid-parameter",
                "expected `,` or `)` in the parameter list",
                TextRange::new(self.cursor, self.cursor + 1),
            ));
            break;
        }
        Some(params)
    }

    fn parse_expression(&mut self, diagnostics: &mut Vec<Diagnostic>) -> Option<Expr> {
        self.skip_trivia();
        let start = self.cursor;
        let character = self.peek()?;
        match character {
            '"' => {
                self.bump();
                let mut value = String::new();
                loop {
                    match self.bump() {
                        Some('"') => break,
                        Some('\\') => {
                            if let Some(escaped) = self.bump() {
                                value.push(escaped);
                            }
                        }
                        Some(other) => value.push(other),
                        None => {
                            diagnostics.push(Diagnostic::warn(
                                "unclosed-string",
                                "unclosed string literal",
                                TextRange::new(start, self.cursor),
                            ));
                            break;
                        }
                    }
                }
                Some(Expr::Literal {
                    value: Value::String(value),
                    range: TextRange::new(start, self.cursor),
                })
            }
            '[' => {
                self.bump();
                let statements = self.parse_statements(Some(']'), diagnostics);
                Some(Expr::Content {
                    statements,
                    range: TextRange::new(start, self.cursor),
                })
            }
            '(' => {
                if self.rest().starts_with("()") {
                    self.cursor += 2;
                    return Some(Expr::Literal {
                        value: Value::Unit,
                        range: TextRange::new(start, self.cursor),
                    });
                }
                diagnostics.push(Diagnostic::warn(
                    "invalid-expression",
                    "expected an expression",
                    TextRange::new(start, self.cursor + 1),
                ));
                None
            }
            character if character.is_ascii_digit() => {
                let digits = self
                    .rest()
                    .bytes()
                    .take_while(u8::is_ascii_digit)
                    .count();
                let mut literal_end = start + digits;
                let mut float = false;
                if self.source[literal_end..].starts_with('.') {
                    let fraction = self.source[literal_end + 1..]
                        .bytes()
                        .take_while(u8::is_ascii_digit)
                        .count();
                    if fraction > 0 {
                        literal_end += 1 + fraction;
                        float = true;
                    }
                }
                self.cursor = literal_end;
                let text = &self.source[start..literal_end];
                let value = if float {
                    Value::Float(text.parse().ok()?)
                } else {
                    Value::Int(text.parse().ok()?)
                };
                Some(Expr::Literal {
                    value,
                    range: TextRange::new(start, literal_end),
                })
            }
            _ => {
                if let Some(word @ ("true" | "false")) = self.peek_word() {
                    self.word();
                    return Some(Expr::Literal {
                        value: Value::Bool(word == "true"),
                        range: TextRange::new(start, self.cursor),
                    });
                }
                let name = self.word()?;
                self.skip_spaces();
                if self.peek() == Some('(') {
                    let args = self.parse_arguments(diagnostics)?;
                    return Some(Expr::Call {
                        callee: Callee::Unresolved(name),
                        args,
                        range: TextRange::new(start, self.cursor),
                    });
                }
                Some(Expr::Local {
                    name,
                    range: TextRange::new(start, self.cursor),
                })
            }
        }
    }

    /// 实参一律具名：位置实参需要完整的绑定规则，本切片不做。
    fn parse_arguments(
        &mut self,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<Vec<(String, Expr)>> {
        let start = self.cursor;
        self.bump();
        let mut args = Vec::new();
        loop {
            self.skip_trivia();
            if self.peek() == Some(')') {
                self.bump();
                break;
            }
            if self.done() {
                diagnostics.push(Diagnostic::warn(
                    "unclosed-argument-list",
                    "unclosed argument list",
                    TextRange::new(start, self.cursor),
                ));
                break;
            }
            let Some(name) = self.word() else {
                diagnostics.push(Diagnostic::warn(
                    "invalid-argument",
                    "arguments are named: `name: value`",
                    TextRange::new(self.cursor, self.cursor + 1),
                ));
                break;
            };
            self.skip_spaces();
            if !self.eat(":") {
                diagnostics.push(Diagnostic::warn(
                    "invalid-argument",
                    "arguments are named: `name: value`",
                    TextRange::new(self.cursor, self.cursor + 1),
                ));
                break;
            }
            let value = self.parse_expression(diagnostics)?;
            args.push((name, value));
            self.skip_trivia();
            if self.eat(",") {
                continue;
            }
            if self.peek() == Some(')') {
                self.bump();
                break;
            }
            diagnostics.push(Diagnostic::warn(
                "invalid-argument",
                "expected `,` or `)` in the argument list",
                TextRange::new(self.cursor, self.cursor + 1),
            ));
            break;
        }
        Some(args)
    }
}
