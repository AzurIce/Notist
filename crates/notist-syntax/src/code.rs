use std::ops::{Deref, DerefMut};

use crate::markup::MarkupParser;
use crate::state::ParseState;
use crate::{Arg, Expr, ExprKind, Param, Statement, Type, Use, valid_binding};

pub(super) struct Token<'a> {
    text: &'a str,
    pub(super) offset: usize,
    end: usize,
    string: bool,
}

fn token_kind(token: &Token) -> &'static str {
    if token.string {
        return "string";
    }
    let first = token.text.chars().next().unwrap_or('\0');
    if first.is_ascii_digit() {
        "int"
    } else if first.is_alphabetic() || first == '_' {
        match token.text {
            "use" | "wasm" | "as" | "let" | "if" | "else" | "true" | "false" => "keyword",
            _ => "name",
        }
    } else {
        "punct"
    }
}

/// Parses Code tokens on demand so a nested Markup parser sees untouched text.
pub(super) struct CodeParser<'s, 'a> {
    state: &'s mut ParseState<'a>,
}

impl<'s, 'a> CodeParser<'s, 'a> {
    pub(super) fn new(state: &'s mut ParseState<'a>) -> Self {
        Self { state }
    }

    // Lex code on demand: markup must see its original whitespace and punctuation.
    pub(super) fn token(&self) -> Token<'a> {
        let mut start = self.pos;
        loop {
            while let Some(c) = self.source[start..]
                .chars()
                .next()
                .filter(|c| c.is_whitespace())
            {
                start += c.len_utf8();
            }
            if self.source[start..].starts_with("//") {
                start += self.source[start..]
                    .find('\n')
                    .unwrap_or(self.source.len() - start);
            } else {
                break;
            }
        }
        let mut chars = self.source[start..].char_indices();
        let Some((_, first)) = chars.next() else {
            return Token {
                text: "",
                offset: start,
                end: start,
                string: false,
            };
        };
        let mut end = start + first.len_utf8();
        if first == '"' {
            let mut escaped = false;
            for (i, c) in chars {
                end = start + i + c.len_utf8();
                if c == '"' && !escaped {
                    break;
                }
                escaped = c == '\\' && !escaped;
            }
        } else if first.is_alphanumeric() || first == '_' {
            for (i, c) in chars {
                if !c.is_alphanumeric() && c != '_' {
                    break;
                }
                end = start + i + c.len_utf8();
            }
        } else if let Some((_, next)) = chars.next()
            && matches!(
                (first, next),
                ('=', '>')
                    | ('-', '>')
                    | (':', ':')
                    | ('=', '=')
                    | ('!', '=')
                    | ('<', '=')
                    | ('>', '=')
            )
        {
            end += next.len_utf8();
        }
        Token {
            text: &self.source[start..end],
            offset: start,
            end,
            string: first == '"',
        }
    }
    pub(super) fn bump(&mut self) {
        let token = self.token();
        self.record_code_trivia(token.offset);
        self.pos = token.end;
        self.state
            .record_span(token.offset, token.end, token_kind(&token));
    }
    pub(super) fn record_code_trivia(&mut self, end: usize) {
        let mut at = self.pos;
        while at < end {
            if self.source[at..].starts_with("//") {
                let stop = self.source[at..end].find('\n').map_or(end, |n| at + n);
                self.state.record_span(at, stop, "comment");
                at = stop;
            } else {
                at += self.source[at..].chars().next().unwrap().len_utf8();
            }
        }
    }
    pub(super) fn at(&self, text: &str) -> bool {
        let t = self.token();
        !t.string && t.text == text
    }
    pub(super) fn eat(&mut self, text: &str) -> bool {
        if self.at(text) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, text: &str) -> Result<(), String> {
        if self.eat(text) {
            Ok(())
        } else {
            Err(format!("expected `{text}`, found `{}`", self.token().text))
        }
    }
    fn name(&mut self) -> Result<String, String> {
        let token = self.token();
        if token.string
            || !token
                .text
                .starts_with(|c: char| c.is_alphabetic() || c == '_')
        {
            return Err("expected a name".into());
        }
        self.pos = token.end;
        self.state
            .record_span(token.offset, token.end, token_kind(&token));
        Ok(token.text.into())
    }
    fn string(&mut self) -> Result<String, String> {
        let token = self.token();
        if !token.string {
            return Err("expected a string".into());
        }
        let value = serde_json::from_str(token.text).map_err(|e| format!("invalid string: {e}"))?;
        self.pos = token.end;
        self.state.record_span(token.offset, token.end, "string");
        Ok(value)
    }
    fn ty(&mut self) -> Result<Type, String> {
        let base = self.base_type()?;
        Ok(if self.eat("?") {
            Type::Optional(Box::new(base))
        } else {
            base
        })
    }
    fn base_type(&mut self) -> Result<Type, String> {
        match self.name()?.as_str() {
            "String" => Ok(Type::String),
            "Int" => Ok(Type::Int),
            "Bool" => Ok(Type::Bool),
            "Content" => Ok(Type::Content),
            "Module" => Ok(Type::Module),
            "Target" => Ok(Type::Target),
            "List" => Ok(Type::List),
            "Dict" => Ok(Type::Dict),
            "Function" => Ok(Type::Function),
            "Any" => Ok(Type::Any),
            "Unit" => Ok(Type::Unit),
            other => Err(format!("unknown type `{other}`")),
        }
    }
    pub(super) fn statement(&mut self) -> Result<Statement, String> {
        if self.eat("use") {
            let mut imports = Vec::new();
            self.use_tree(Vec::new(), &mut imports, 0)?;
            self.expect(";")?;
            return Ok(Statement::Use(imports));
        }
        if self.eat("wasm") {
            let path = self.string()?;
            self.expect(";")?;
            return Ok(Statement::Wasm(path));
        }
        if self.eat("let") {
            let name = self.name()?;
            if !valid_binding(&name) {
                return Err(format!("invalid binding `{name}`"));
            }
            let ty = if self.eat(":") {
                Some(self.ty()?)
            } else {
                None
            };
            self.expect("=")?;
            let expr = self.checked_expr(ty)?;
            self.expect(";")?;
            return Ok(Statement::Let(name, expr));
        }
        let expr = self.expr(0)?;
        self.expect(";")?;
        Ok(Statement::Expression(expr))
    }
    pub(super) fn segment(&mut self) -> Result<String, String> {
        let token = self.token();
        if token.string
            || !(valid_binding(token.text) || matches!(token.text, "vault" | "self" | "super"))
        {
            return Err("expected module path segment".into());
        }
        let name = token.text.to_owned();
        self.bump();
        Ok(name)
    }
    /// A module path followed by zero or more quoted label constraints.
    /// Both Code and wikilinks use the same string lexer, so delimiters inside
    /// labels remain text rather than splitting the path or closing a wikilink.
    pub(super) fn reference_path(
        &mut self,
        first: String,
    ) -> Result<(Vec<String>, Vec<String>), String> {
        let mut module = vec![first];
        let mut labels = Vec::new();
        while self.eat("::") {
            if self.token().string {
                let label = self.string()?;
                if label.is_empty() {
                    return Err("label path segments must not be empty".into());
                }
                labels.push(label);
            } else if labels.is_empty() {
                module.push(self.segment()?);
            } else {
                return Err("expected a quoted label after the label path begins".into());
            }
        }
        Ok((module, labels))
    }
    fn use_tree(
        &mut self,
        mut path: Vec<String>,
        imports: &mut Vec<Use>,
        depth: usize,
    ) -> Result<(), String> {
        if depth > 128 {
            return Err("use tree nesting limit exceeded".into());
        }
        if self.eat("{") {
            while !self.at("}") {
                self.use_tree(path.clone(), imports, depth + 1)?;
                if !self.eat(",") {
                    break;
                }
            }
            return self.expect("}");
        }
        if self.eat("*") {
            if path.is_empty() {
                return Err("glob requires a module path".into());
            }
            imports.push(Use {
                path,
                alias: None,
                glob: true,
            });
            return Ok(());
        }
        let segment = self.segment()?;
        if segment != "self" || path.is_empty() {
            path.push(segment);
        }
        if self.eat("::") {
            return self.use_tree(path, imports, depth + 1);
        }
        let alias = if self.eat("as") {
            let name = self.name()?;
            if !valid_binding(&name) {
                return Err("invalid import alias".into());
            }
            Some(name)
        } else {
            None
        };
        if alias.is_none() && !path.last().is_some_and(|s| valid_binding(s)) {
            return Err("module root imports require `as name`".into());
        }
        imports.push(Use {
            path,
            alias,
            glob: false,
        });
        Ok(())
    }
    pub(super) fn expr(&mut self, min: u8) -> Result<Expr, String> {
        if self.depth >= 128 {
            return Err("expression nesting limit exceeded".into());
        }
        self.depth += 1;
        let result = self.expression(min);
        self.depth -= 1;
        result
    }
    fn checked_expr(&mut self, ty: Option<Type>) -> Result<Expr, String> {
        let value = self.expr(0)?;
        Ok(match ty {
            Some(ty) => Expr {
                offset: value.offset,
                end: value.end,
                id: 0,
                kind: ExprKind::Typed(ty, Box::new(value)),
            },
            None => value,
        })
    }
    fn expression(&mut self, min: u8) -> Result<Expr, String> {
        let offset = self.token().offset;
        let kind = if self.token().string {
            ExprKind::String(self.string()?)
        } else if self.eat("[") {
            ExprKind::Content(self.markup(']')?)
        } else if self.eat("(") {
            let checkpoint = self.pos;
            let tokens = self.tokens.len();
            if let Some(params) = self.lambda_params()? {
                let result = if self.eat("->") {
                    Some(self.ty()?)
                } else {
                    None
                };
                self.expect("=>")?;
                ExprKind::Lambda(params, Box::new(self.checked_expr(result)?))
            } else {
                self.pos = checkpoint;
                self.tokens.truncate(tokens);
                self.parenthesized()?
            }
        } else if self.eat("if") {
            let condition = self.expr(0)?;
            self.expect("{")?;
            let yes = self.expr(0)?;
            self.expect("}")?;
            self.expect("else")?;
            self.expect("{")?;
            let no = self.expr(0)?;
            self.expect("}")?;
            ExprKind::If(Box::new(condition), Box::new(yes), Box::new(no))
        } else if self.eat("-") {
            ExprKind::Binary(
                "-".into(),
                Box::new(Expr {
                    offset,
                    end: offset,
                    id: 0,
                    kind: ExprKind::Int(0),
                }),
                Box::new(self.expr(4)?),
            )
        } else if self.at("true") || self.at("false") {
            let b = self.eat("true");
            if !b {
                self.expect("false")?;
            }
            ExprKind::Bool(b)
        } else if self.token().text.starts_with(|c: char| c.is_ascii_digit()) {
            let value = self.token().text.parse().map_err(|_| "invalid integer")?;
            self.bump();
            ExprKind::Int(value)
        } else {
            let first = self.name()?;
            let (module, labels) = self.reference_path(first)?;
            if labels.is_empty() {
                ExprKind::Name(module.join("::"))
            } else {
                ExprKind::Target(module, labels)
            }
        };
        // Every branch above leaves `pos` just past its last consumed token.
        let mut end = self.pos;
        let mut left = Expr {
            offset,
            end,
            id: 0,
            kind,
        };
        loop {
            // Bare markup interpolation consumes only adjacent postfixes.
            let adjacent = self.token().offset == self.pos;
            if min != 5 || adjacent {
                if self.eat("(") {
                    let args = self.arguments(")")?;
                    end = self.pos;
                    left = Expr {
                        offset,
                        end,
                        id: 0,
                        kind: ExprKind::Call(Box::new(left), args),
                    };
                    continue;
                }
                if self.at(".") {
                    let checkpoint = self.pos;
                    let tokens = self.tokens.len();
                    self.bump();
                    if self.token().offset == self.pos
                        && self
                            .token()
                            .text
                            .starts_with(|c: char| c.is_alphabetic() || c == '_')
                    {
                        let field = self.name()?;
                        end = self.pos;
                        left = Expr {
                            offset,
                            end,
                            id: 0,
                            kind: ExprKind::Field(Box::new(left), field),
                        };
                        continue;
                    }
                    self.pos = checkpoint;
                    self.tokens.truncate(tokens);
                }
                if self.eat("[") {
                    let body_offset = self.pos - 1;
                    let parts = self.markup(']')?;
                    end = self.pos;
                    let body = Expr {
                        offset: body_offset,
                        end,
                        id: 0,
                        kind: ExprKind::Content(parts),
                    };
                    match &mut left.kind {
                        ExprKind::Call(_, args) => args.push(Arg {
                            trailing: true,
                            name: None,
                            expr: body,
                        }),
                        _ => {
                            left = Expr {
                                offset,
                                end,
                                id: 0,
                                kind: ExprKind::Call(
                                    Box::new(left),
                                    vec![Arg {
                                        trailing: true,
                                        name: None,
                                        expr: body,
                                    }],
                                ),
                            }
                        }
                    }
                    left.end = self.pos;
                    continue;
                }
            }
            let op = self.token().text;
            let precedence = match op {
                "==" | "!=" | "<" | ">" | "<=" | ">=" => 1,
                "+" | "-" => 2,
                "*" | "/" => 3,
                _ => break,
            };
            if precedence < min {
                break;
            }
            self.bump();
            let rhs = self.expr(precedence + 1)?;
            end = rhs.end;
            left = Expr {
                offset,
                end,
                id: 0,
                kind: ExprKind::Binary(op.into(), Box::new(left), Box::new(rhs)),
            };
        }
        Ok(left)
    }
    fn lambda_params(&mut self) -> Result<Option<Vec<Param>>, String> {
        let checkpoint = self.pos;
        let token_count = self.tokens.len();
        // First recognize the closing delimiter; only then interpret fields as parameters.
        let mut nesting = 0usize;
        while !self.at("") {
            if self.at(")") && nesting == 0 {
                self.bump();
                break;
            }
            if self.at("(") || self.at("[") || self.at("{") {
                nesting += 1;
            } else if self.at(")") || self.at("]") || self.at("}") {
                nesting = nesting.saturating_sub(1);
            }
            self.bump();
        }
        let lambda = self.at("=>") || self.at("->");
        self.pos = checkpoint;
        self.tokens.truncate(token_count);
        if !lambda {
            return Ok(None);
        }
        let mut params: Vec<Param> = Vec::new();
        while !self.at(")") {
            let name = self.name()?;
            if !valid_binding(&name) {
                return Err(format!("invalid parameter `{name}`"));
            }
            if params.iter().any(|p| p.name == name) {
                return Err(format!("duplicate parameter `{name}`"));
            }
            let ty = if self.eat(":") { self.ty()? } else { Type::Any };
            let default = if self.eat("=") {
                Some(self.expr(0)?)
            } else {
                None
            };
            params.push(Param { name, ty, default });
            if !self.eat(",") {
                break;
            }
        }
        self.expect(")")?;
        Ok(Some(params))
    }
    fn parenthesized(&mut self) -> Result<ExprKind, String> {
        if self.eat(")") {
            return Ok(ExprKind::Unit);
        }
        if self.eat(",") {
            self.expect(")")?;
            return Ok(ExprKind::List(vec![]));
        }
        if self.eat(":") {
            self.expect(")")?;
            return Ok(ExprKind::Dict(vec![]));
        }
        let checkpoint = self.pos;
        let tokens = self.tokens.len();
        let key = if self.token().string {
            self.string()
        } else {
            self.name()
        };
        if let Ok(key) = key
            && self.eat(":")
        {
            let mut fields = vec![(key, self.expr(0)?)];
            while self.eat(",") && !self.at(")") {
                let key = if self.token().string {
                    self.string()?
                } else {
                    self.name()?
                };
                if fields.iter().any(|(name, _)| *name == key) {
                    return Err(format!("duplicate dictionary key `{key}`"));
                }
                self.expect(":")?;
                fields.push((key, self.expr(0)?));
            }
            self.expect(")")?;
            return Ok(ExprKind::Dict(fields));
        }
        self.pos = checkpoint;
        self.tokens.truncate(tokens);
        let first = self.expr(0)?;
        if !self.eat(",") {
            self.expect(")")?;
            return Ok(first.kind);
        }
        let mut values = vec![first];
        while !self.at(")") {
            values.push(self.expr(0)?);
            if !self.eat(",") {
                break;
            }
        }
        self.expect(")")?;
        Ok(ExprKind::List(values))
    }
    // The Code grammar only owns the opening bracket. The body is parsed by
    // Markup, then returned as the shared Content expression.
    fn markup(&mut self, end: char) -> Result<Vec<Expr>, String> {
        MarkupParser::new(self.state).parse_literal(end)
    }
    fn arguments(&mut self, end: &str) -> Result<Vec<Arg>, String> {
        let mut args = Vec::new();
        let mut seen_named = false;
        if !self.at(end) {
            loop {
                let checkpoint = self.pos;
                let tokens = self.tokens.len();
                let named = if self
                    .token()
                    .text
                    .starts_with(|c: char| c.is_alphabetic() || c == '_')
                {
                    let name = self.name()?;
                    if self.eat(":") {
                        Some(name)
                    } else {
                        self.pos = checkpoint;
                        self.tokens.truncate(tokens);
                        None
                    }
                } else {
                    None
                };
                if named.is_some() {
                    seen_named = true;
                } else if seen_named {
                    return Err("positional argument cannot follow a named argument".into());
                }
                args.push(Arg {
                    trailing: false,
                    name: named,
                    expr: self.expr(0)?,
                });
                if !self.eat(",") || self.at(end) {
                    break;
                }
            }
        }
        self.expect(end)?;
        Ok(args)
    }
}

impl<'s, 'a> Deref for CodeParser<'s, 'a> {
    type Target = ParseState<'a>;

    fn deref(&self) -> &Self::Target {
        self.state
    }
}

impl DerefMut for CodeParser<'_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.state
    }
}
