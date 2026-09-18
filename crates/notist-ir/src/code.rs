//! `.notc` 前端：纯 Code 模式的模块体。
//!
//! 模块体是语句序列；绑定与声明不产内容，值语句产内容。
//! 这里只做识别与结构，不做名字解析、绑定装配与类型检查——那些是意图层的事。

use notist_model::TextRange;

use crate::ast::{Arg, ArgLabel, BinaryOp, Callee, Expr, Param, Statement, UnaryOp};
use crate::ir::{Diagnostic, Value};
use crate::types::Type;

/// 解析一个 `.notc` 模块。
pub fn parse(source: &str) -> (Vec<Statement>, Vec<Diagnostic>) {
    let mut parser = Parser {
        source,
        cursor: 0,
        diagnostics: Vec::new(),
        trailing_allowed: true,
    };
    let statements = parser.parse_statements(None);
    (statements, parser.diagnostics)
}

struct Parser<'a> {
    source: &'a str,
    cursor: usize,
    diagnostics: Vec<Diagnostic>,
    /// `if` 的条件里禁用尾随体，避免条件吞掉分支块。
    trailing_allowed: bool,
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

    fn warn(&mut self, code: &'static str, message: impl Into<String>, start: usize, end: usize) {
        self.diagnostics
            .push(Diagnostic::warn(code, message, TextRange::new(start, end)));
    }

    /// 跳过行内空白。
    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.cursor += 1;
        }
    }

    /// 跳过空白与注释，含换行。
    fn skip_trivia(&mut self) {
        loop {
            let before = self.cursor;
            while matches!(self.peek(), Some(character) if character.is_whitespace()) {
                self.bump();
            }
            if self.rest().starts_with("//") {
                while matches!(self.peek(), Some(character) if character != '\n') {
                    self.bump();
                }
                continue;
            }
            if self.rest().starts_with("/*") {
                self.skip_block_comment();
                continue;
            }
            if self.cursor == before {
                break;
            }
        }
    }

    fn skip_block_comment(&mut self) {
        let start = self.cursor;
        self.cursor += 2;
        let mut depth = 1;
        while depth > 0 && !self.done() {
            if self.rest().starts_with("/*") {
                depth += 1;
                self.cursor += 2;
            } else if self.rest().starts_with("*/") {
                depth -= 1;
                self.cursor += 2;
            } else {
                self.bump();
            }
        }
        if depth > 0 {
            self.warn("unclosed-comment", "unclosed block comment", start, self.cursor);
        }
    }

    /// 读一个标识符：首字符为字母或下划线，后续可带数字、下划线与连字符。
    fn word(&mut self) -> Option<String> {
        let start = self.cursor;
        let rest = self.rest();
        let mut end = 0;
        for (index, character) in rest.char_indices() {
            let accept = if index == 0 {
                character.is_ascii_alphabetic() || character == '_'
            } else {
                character.is_ascii_alphanumeric() || character == '_' || character == '-'
            };
            if !accept {
                break;
            }
            end = index + character.len_utf8();
        }
        if end == 0 {
            return None;
        }
        self.cursor = start + end;
        Some(self.source[start..self.cursor].to_owned())
    }

    /// 前瞻一个标识符，不推进。
    fn peek_word(&self) -> Option<&'a str> {
        let rest = self.rest();
        let mut end = 0;
        for (index, character) in rest.char_indices() {
            let accept = if index == 0 {
                character.is_ascii_alphabetic() || character == '_'
            } else {
                character.is_ascii_alphanumeric() || character == '_' || character == '-'
            };
            if !accept {
                break;
            }
            end = index + character.len_utf8();
        }
        (end > 0).then(|| &rest[..end])
    }

    // ---- 语句 ----

    /// 解析一段语句序列；`closer` 为 `Some` 时遇到该字符收尾。
    fn parse_statements(&mut self, closer: Option<char>) -> Vec<Statement> {
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
            let Some(statement) = self.parse_statement() else {
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
                let rest = self.rest();
                let character = rest.chars().next().unwrap_or(' ');
                self.warn(
                    "unexpected-token",
                    format!("unexpected token `{character}` after statement"),
                    start,
                    start + character.len_utf8(),
                );
                self.recover_line();
            }
        }
        if !closed {
            self.warn(
                "unclosed-block",
                "unclosed block",
                self.cursor,
                self.cursor,
            );
        }
        statements
    }

    fn recover_line(&mut self) {
        while matches!(self.peek(), Some(character) if character != '\n') {
            self.bump();
        }
    }

    fn parse_statement(&mut self) -> Option<Statement> {
        let start = self.cursor;
        match self.peek_word() {
            Some("let") => {
                self.word();
                self.parse_let(start)
            }
            Some("extern") => {
                self.word();
                self.parse_extern(start)
            }
            _ => {
                let expr = self.parse_expression()?;
                Some(Statement::Value {
                    range: expr.range(),
                    expr,
                })
            }
        }
    }

    fn parse_let(&mut self, start: usize) -> Option<Statement> {
        self.skip_spaces();
        let recursive = if self.peek_word() == Some("rec") {
            self.word();
            self.skip_spaces();
            true
        } else {
            false
        };
        let Some(name) = self.word() else {
            self.warn("invalid-let", "expected a name after `let`", start, self.cursor);
            return None;
        };
        self.skip_spaces();
        // 名字后紧跟参数表即函数定义糖：`let f(x) = ...` 等价于 `let f = (x) => ...`。
        let value = if self.peek() == Some('(') {
            let params = self.parse_params()?;
            let returns = self.parse_return_type();
            self.skip_spaces();
            if !self.eat("=") {
                self.warn(
                    "invalid-let",
                    format!("`let {name}` expects `=`"),
                    start,
                    self.cursor,
                );
                return None;
            }
            self.skip_trivia();
            let body = self.parse_expression()?;
            let range = TextRange::new(start, body.range().end);
            Expr::Lambda {
                params,
                returns,
                body: Box::new(body),
                range,
            }
        } else {
            if self.rest().starts_with("->") {
                self.warn(
                    "invalid-let",
                    "a return type needs a parameter list",
                    start,
                    self.cursor + 2,
                );
            }
            self.skip_spaces();
            if !self.eat("=") {
                self.warn(
                    "invalid-let",
                    format!("`let {name}` expects `=`"),
                    start,
                    self.cursor,
                );
                return None;
            }
            self.skip_trivia();
            self.parse_expression()?
        };
        let range = TextRange::new(start, value.range().end);
        Some(Statement::Bind {
            name,
            recursive,
            value,
            range,
        })
    }

    /// `extern fn name(params) -> Type`：只有声明，没有体。
    fn parse_extern(&mut self, start: usize) -> Option<Statement> {
        self.skip_spaces();
        let head = self.word().unwrap_or_default();
        if !matches!(head.as_str(), "fn" | "element" | "inline") {
            self.warn(
                "invalid-declaration",
                format!("expected `fn`, `element` or `inline`, found `{head}`"),
                start,
                self.cursor,
            );
            return None;
        }
        self.skip_spaces();
        let Some(name) = self.word() else {
            self.warn(
                "invalid-declaration",
                "expected a declaration name",
                start,
                self.cursor,
            );
            return None;
        };
        self.skip_spaces();
        let params = if self.peek() == Some('(') {
            self.parse_params()?
        } else {
            Vec::new()
        };
        self.skip_spaces();
        if !self.eat("->") {
            self.warn(
                "invalid-declaration",
                format!("declaration `{name}` expects `->`"),
                start,
                self.cursor,
            );
            return None;
        }
        self.skip_spaces();
        let returns = self.parse_type();
        Some(Statement::Extern {
            name,
            params,
            returns,
            range: TextRange::new(start, self.cursor),
        })
    }

    // ---- 参数与类型 ----

    fn parse_params(&mut self) -> Option<Vec<Param>> {
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
                self.warn(
                    "unclosed-parameter-list",
                    "unclosed parameter list",
                    start,
                    self.cursor,
                );
                break;
            }
            let parameter_start = self.cursor;
            let Some(name) = self.word() else {
                self.warn(
                    "invalid-parameter",
                    "expected a parameter name",
                    self.cursor,
                    self.cursor + 1,
                );
                break;
            };
            self.skip_spaces();
            let ty = if self.eat(":") {
                self.skip_spaces();
                self.parse_type()
            } else {
                None
            };
            self.skip_spaces();
            let default = if self.eat("=") {
                self.skip_trivia();
                self.parse_expression()
            } else {
                None
            };
            params.push(Param {
                name,
                ty,
                default,
                range: TextRange::new(parameter_start, self.cursor),
            });
            self.skip_spaces();
            if self.eat(",") {
                continue;
            }
            self.skip_trivia();
            if self.peek() == Some(')') {
                self.bump();
                break;
            }
            self.warn(
                "invalid-parameter",
                "expected `,` or `)` in the parameter list",
                self.cursor,
                self.cursor + 1,
            );
            break;
        }
        Some(params)
    }

    /// `-> Type`；没有箭头时返回 `None`。
    fn parse_return_type(&mut self) -> Option<Type> {
        self.skip_spaces();
        if !self.eat("->") {
            return None;
        }
        self.skip_spaces();
        self.parse_type()
    }

    fn parse_type(&mut self) -> Option<Type> {
        let start = self.cursor;
        let Some(text) = self.word() else {
            self.warn("invalid-type", "expected a type", start, self.cursor + 1);
            return None;
        };
        match Type::parse(&text) {
            Some(ty) => Some(ty),
            None => {
                self.warn(
                    "unknown-type",
                    format!("unknown type `{text}`"),
                    start,
                    self.cursor,
                );
                None
            }
        }
    }

    // ---- 表达式 ----

    fn parse_expression(&mut self) -> Option<Expr> {
        self.skip_trivia();
        if self.peek() == Some('(')
            && let Some(lambda) = self.try_lambda()
        {
            return Some(lambda);
        }
        self.parse_or()
    }

    /// 尝试 `(params) => body`；不成立时回退，不留诊断。
    fn try_lambda(&mut self) -> Option<Expr> {
        let start = self.cursor;
        let diagnostics_mark = self.diagnostics.len();
        let params = self.parse_params()?;
        self.skip_spaces();
        if !self.eat("=>") {
            self.cursor = start;
            self.diagnostics.truncate(diagnostics_mark);
            return None;
        }
        self.skip_trivia();
        let body = self.parse_expression()?;
        let range = TextRange::new(start, body.range().end);
        Some(Expr::Lambda {
            params,
            returns: None,
            body: Box::new(body),
            range,
        })
    }

    fn parse_or(&mut self) -> Option<Expr> {
        let mut left = self.parse_and()?;
        loop {
            self.skip_spaces();
            if self.peek_word() != Some("or") {
                break;
            }
            self.word();
            self.skip_trivia();
            let right = self.parse_and()?;
            left = binary(BinaryOp::Or, left, right);
        }
        Some(left)
    }

    fn parse_and(&mut self) -> Option<Expr> {
        let mut left = self.parse_not()?;
        loop {
            self.skip_spaces();
            if self.peek_word() != Some("and") {
                break;
            }
            self.word();
            self.skip_trivia();
            let right = self.parse_not()?;
            left = binary(BinaryOp::And, left, right);
        }
        Some(left)
    }

    fn parse_not(&mut self) -> Option<Expr> {
        self.skip_trivia();
        if self.peek_word() == Some("not") {
            let start = self.cursor;
            self.word();
            self.skip_trivia();
            let operand = self.parse_not()?;
            let range = TextRange::new(start, operand.range().end);
            return Some(Expr::Unary {
                op: UnaryOp::Not,
                operand: Box::new(operand),
                range,
            });
        }
        self.parse_compare()
    }

    fn parse_compare(&mut self) -> Option<Expr> {
        let mut left = self.parse_sum()?;
        loop {
            self.skip_spaces();
            let op = if self.eat("==") {
                BinaryOp::Equal
            } else if self.eat("!=") {
                BinaryOp::NotEqual
            } else if self.eat("<=") {
                BinaryOp::LessEqual
            } else if self.eat(">=") {
                BinaryOp::GreaterEqual
            } else if self.eat("<") {
                BinaryOp::Less
            } else if self.eat(">") {
                BinaryOp::Greater
            } else {
                break;
            };
            self.skip_trivia();
            let right = self.parse_sum()?;
            left = binary(op, left, right);
        }
        Some(left)
    }

    fn parse_sum(&mut self) -> Option<Expr> {
        let mut left = self.parse_product()?;
        loop {
            self.skip_spaces();
            let op = if self.eat("+") {
                BinaryOp::Add
            } else if self.rest().starts_with('-') && !self.rest().starts_with("->") {
                self.bump();
                BinaryOp::Subtract
            } else {
                break;
            };
            self.skip_trivia();
            let right = self.parse_product()?;
            left = binary(op, left, right);
        }
        Some(left)
    }

    fn parse_product(&mut self) -> Option<Expr> {
        let mut left = self.parse_unary()?;
        loop {
            self.skip_spaces();
            let op = if self.eat("*") {
                BinaryOp::Multiply
            } else if self.rest().starts_with('/')
                && !self.rest().starts_with("//")
                && !self.rest().starts_with("/*")
            {
                self.bump();
                BinaryOp::Divide
            } else {
                break;
            };
            self.skip_trivia();
            let right = self.parse_unary()?;
            left = binary(op, left, right);
        }
        Some(left)
    }

    fn parse_unary(&mut self) -> Option<Expr> {
        self.skip_trivia();
        if self.peek() == Some('-') && !self.rest().starts_with("->") {
            let start = self.cursor;
            self.bump();
            self.skip_trivia();
            let operand = self.parse_unary()?;
            let range = TextRange::new(start, operand.range().end);
            return Some(Expr::Unary {
                op: UnaryOp::Negate,
                operand: Box::new(operand),
                range,
            });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Option<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            let mark = self.cursor;
            self.skip_spaces();
            if self.peek() == Some('(') {
                let args = self.parse_arguments()?;
                let start = expr.range().start;
                let range = TextRange::new(start, self.cursor);
                expr = Expr::Call {
                    callee: self.callee_of(expr),
                    args,
                    trailing: None,
                    range,
                };
                continue;
            }
            self.cursor = mark;
            // 尾随体必须紧邻被调者，避免与后续的内容块语句混淆。
            if self.trailing_allowed && self.peek() == Some('[') {
                let block = self.parse_content_block()?;
                let start = expr.range().start;
                let end = block.range().end;
                expr = match expr {
                    Expr::Call {
                        callee,
                        args,
                        trailing: None,
                        ..
                    } => Expr::Call {
                        callee,
                        args,
                        trailing: Some(Box::new(block)),
                        range: TextRange::new(start, end),
                    },
                    // 名字后直接跟尾随体：等价于不带其他实参的调用。
                    Expr::Local { name, .. } => Expr::Call {
                        callee: Callee::Named(name),
                        args: Vec::new(),
                        trailing: Some(Box::new(block)),
                        range: TextRange::new(start, end),
                    },
                    other => {
                        self.warn(
                            "unexpected-trailing-body",
                            "a trailing body needs a call",
                            start,
                            end,
                        );
                        other
                    }
                };
                continue;
            }
            break;
        }
        Some(expr)
    }

    /// 调用只以名字为被调者；其他形态给出诊断并保留原表达式。
    fn callee_of(&mut self, expr: Expr) -> Callee {
        match expr {
            Expr::Local { name, .. } => Callee::Named(name),
            other => {
                let range = other.range();
                self.warn(
                    "not-callable",
                    "only a name can be called",
                    range.start,
                    range.end,
                );
                Callee::Named(String::new())
            }
        }
    }

    fn parse_arguments(&mut self) -> Option<Vec<Arg>> {
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
                self.warn(
                    "unclosed-argument-list",
                    "unclosed argument list",
                    start,
                    self.cursor,
                );
                break;
            }
            let argument_start = self.cursor;
            let label = {
                let mark = self.cursor;
                match self.word() {
                    Some(name) => {
                        self.skip_spaces();
                        if self.peek() == Some(':') {
                            self.bump();
                            ArgLabel::Named(name)
                        } else {
                            self.cursor = mark;
                            ArgLabel::Positional
                        }
                    }
                    None => ArgLabel::Positional,
                }
            };
            self.skip_trivia();
            let value = self.parse_expression()?;
            args.push(Arg {
                label,
                range: TextRange::new(argument_start, value.range().end),
                value,
            });
            self.skip_spaces();
            if self.eat(",") {
                continue;
            }
            self.skip_trivia();
            if self.peek() == Some(')') {
                self.bump();
                break;
            }
            self.warn(
                "invalid-argument",
                "expected `,` or `)` in the argument list",
                self.cursor,
                self.cursor + 1,
            );
            break;
        }
        Some(args)
    }

    fn parse_primary(&mut self) -> Option<Expr> {
        self.skip_trivia();
        let start = self.cursor;
        let character = self.peek()?;
        match character {
            '"' => self.parse_string(),
            '[' => self.parse_content_block(),
            '{' => self.parse_code_block(),
            '(' => {
                if self.rest().starts_with("()") {
                    self.cursor += 2;
                    return Some(Expr::Literal {
                        value: Value::Unit,
                        range: TextRange::new(start, self.cursor),
                    });
                }
                self.bump();
                let inner = self.parse_expression()?;
                self.skip_trivia();
                if !self.eat(")") {
                    self.warn("unclosed-group", "unclosed group", start, self.cursor);
                }
                Some(inner)
            }
            character if character.is_ascii_digit() => self.parse_number(),
            _ => {
                let word = self.peek_word()?;
                match word {
                    "true" | "false" => {
                        self.word();
                        Some(Expr::Literal {
                            value: Value::Bool(word == "true"),
                            range: TextRange::new(start, self.cursor),
                        })
                    }
                    "if" => self.parse_if(),
                    "let" | "rec" | "extern" | "else" | "and" | "or" | "not" => {
                        self.warn(
                            "unexpected-keyword",
                            format!("unexpected keyword `{word}`"),
                            start,
                            start + word.len(),
                        );
                        None
                    }
                    _ => {
                        self.word();
                        Some(Expr::Local {
                            name: word.to_owned(),
                            range: TextRange::new(start, self.cursor),
                        })
                    }
                }
            }
        }
    }

    fn parse_if(&mut self) -> Option<Expr> {
        let start = self.cursor;
        self.word();
        self.skip_trivia();
        let saved = self.trailing_allowed;
        self.trailing_allowed = false;
        let condition = self.parse_expression();
        self.trailing_allowed = saved;
        let condition = condition?;
        self.skip_trivia();
        let then_branch = self.parse_block_expression()?;
        self.skip_trivia();
        let else_branch = if self.peek_word() == Some("else") {
            self.word();
            self.skip_trivia();
            Some(Box::new(self.parse_block_expression()?))
        } else {
            None
        };
        let end = else_branch
            .as_ref()
            .map(|branch| branch.range().end)
            .unwrap_or(then_branch.range().end);
        Some(Expr::If {
            condition: Box::new(condition),
            then_branch: Box::new(then_branch),
            else_branch,
            range: TextRange::new(start, end),
        })
    }

    /// `if` 的分支必须是块，消除 `else` 的分界歧义。
    fn parse_block_expression(&mut self) -> Option<Expr> {
        self.skip_trivia();
        match self.peek() {
            Some('[') => self.parse_content_block(),
            Some('{') => self.parse_code_block(),
            _ => {
                self.warn(
                    "expected-block",
                    "expected a block: `[ ... ]` or `{ ... }`",
                    self.cursor,
                    self.cursor + 1,
                );
                None
            }
        }
    }

    fn parse_content_block(&mut self) -> Option<Expr> {
        let start = self.cursor;
        if !self.eat("[") {
            return None;
        }
        let body = self.parse_statements(Some(']'));
        Some(Expr::Content {
            body,
            range: TextRange::new(start, self.cursor),
        })
    }

    fn parse_code_block(&mut self) -> Option<Expr> {
        let start = self.cursor;
        if !self.eat("{") {
            return None;
        }
        let body = self.parse_statements(Some('}'));
        Some(Expr::Block {
            body,
            range: TextRange::new(start, self.cursor),
        })
    }

    fn parse_string(&mut self) -> Option<Expr> {
        let start = self.cursor;
        self.bump();
        let mut value = String::new();
        loop {
            match self.bump() {
                Some('"') => break,
                Some('\\') => match self.bump() {
                    Some('n') => value.push('\n'),
                    Some('t') => value.push('\t'),
                    Some('r') => value.push('\r'),
                    Some('0') => value.push('\0'),
                    Some('\\') => value.push('\\'),
                    Some('"') => value.push('"'),
                    Some(other) => {
                        let position = self.cursor - other.len_utf8();
                        self.warn(
                            "unknown-escape",
                            format!("unknown escape `\\{other}`"),
                            position - 1,
                            self.cursor,
                        );
                        value.push(other);
                    }
                    None => break,
                },
                Some(other) => value.push(other),
                None => {
                    self.warn("unclosed-string", "unclosed string literal", start, self.cursor);
                    break;
                }
            }
        }
        Some(Expr::Literal {
            value: Value::String(value),
            range: TextRange::new(start, self.cursor),
        })
    }

    fn parse_number(&mut self) -> Option<Expr> {
        let start = self.cursor;
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
}

/// 由左右操作数拼一个二元表达式，区间覆盖两侧。
fn binary(op: BinaryOp, left: Expr, right: Expr) -> Expr {
    let range = TextRange::new(left.range().start, right.range().end);
    Expr::Binary {
        op,
        left: Box::new(left),
        right: Box::new(right),
        range,
    }
}
