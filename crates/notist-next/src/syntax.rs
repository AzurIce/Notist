#[derive(Clone, Debug)]
pub struct Expr {
    pub offset: usize,
    pub end: usize,
    /// Pre-order node id within its module's syntax tree (0 before assignment).
    pub id: usize,
    pub kind: ExprKind,
}

pub fn valid_binding(name: &str) -> bool {
    name.starts_with(|c: char| c.is_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !matches!(
            name,
            "let"
                | "use"
                | "wasm"
                | "as"
                | "if"
                | "else"
                | "none"
                | "true"
                | "false"
                | "self"
                | "super"
                | "vault"
        )
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    None,
    String(String),
    Int(i64),
    Bool(bool),
    Name(String),
    Target(Vec<String>, Option<String>),
    Declaration(Box<Statement>),
    Section(usize, Vec<Expr>, Vec<Expr>),
    List(Vec<Expr>),
    Dict(Vec<(String, Expr)>),
    Content(Vec<Expr>),
    Styled(&'static str, Vec<Expr>),
    Call(Box<Expr>, Vec<Arg>),
    Field(Box<Expr>, String),
    Lambda(Vec<Param>, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    Binary(String, Box<Expr>, Box<Expr>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    String,
    Int,
    Bool,
    Content,
    Item,
    Module,
    Target,
    List,
    Dict,
    Function,
    Any,
    None,
    Optional(Box<Type>),
}

#[derive(Clone, Debug)]
pub struct Arg {
    pub name: Option<String>,
    pub expr: Expr,
    pub trailing: bool,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub ty: Type,
    pub default: Option<Expr>,
}

#[derive(Clone, Debug)]
pub struct Use {
    pub path: Vec<String>,
    pub alias: Option<String>,
    pub glob: bool,
}

#[derive(Clone, Debug)]
pub enum Statement {
    Wasm(String),
    Use(Vec<Use>),
    Let(String, Expr),
    Expression(Expr),
    Error(usize, String),
}

struct Token<'a> {
    text: &'a str,
    offset: usize,
    end: usize,
    string: bool,
}

#[derive(Clone, Debug)]
pub struct TokenEvent {
    pub i: usize,
    pub start: usize,
    pub end: usize,
    pub kind: &'static str,
    pub text: String,
    pub recovery: bool,
}

#[derive(Clone, Debug)]
pub struct SyntaxError {
    pub start: usize,
    pub end: usize,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct ParseResult {
    pub statements: Vec<Statement>,
    /// Full statement range (including `;`), aligned with `statements`.
    pub stmt_ranges: Vec<(usize, usize)>,
    /// Pre-order statement ids, aligned with `statements` (expr ids live on `Expr::id`).
    pub stmt_ids: Vec<usize>,
    /// Total pre-order ids assigned (ids are dense 1..=id_count).
    pub id_count: usize,
    pub tokens: Vec<TokenEvent>,
    pub errors: Vec<SyntaxError>,
}

pub fn parse(source: &str) -> Vec<Statement> {
    parse_traced(source).statements
}

pub fn parse_source(path: &str, source: &str) -> ParseResult {
    if !path.ends_with(".not") {
        return parse_traced(source);
    }
    let mut parser = Parser {
        source,
        pos: 0,
        depth: 0,
        tokens: Vec::new(),
        recovery: false,
    };
    let mut result = ParseResult {
        statements: Vec::new(),
        stmt_ranges: Vec::new(),
        stmt_ids: Vec::new(),
        id_count: 0,
        tokens: Vec::new(),
        errors: Vec::new(),
    };
    match parser.markup_root() {
        Ok(parts) => {
            for expr in parts {
                result.stmt_ranges.push((expr.offset, expr.end));
                result.statements.push(match expr.kind {
                    ExprKind::Declaration(statement) => *statement,
                    _ => Statement::Expression(expr),
                });
            }
        }
        Err(message) => {
            let offset = parser.pos;
            result.errors.push(SyntaxError {
                start: offset,
                end: offset,
                message: message.clone(),
            });
            result.statements.push(Statement::Error(offset, message));
            result.stmt_ranges.push((offset, offset));
        }
    }
    result.id_count = assign_ids(
        &mut result.statements,
        &mut result.stmt_ids,
        &mut parser.tokens,
        &mut result.tokens,
    );
    result
}

pub fn parse_traced(source: &str) -> ParseResult {
    let mut parser = Parser {
        source,
        pos: 0,
        depth: 0,
        tokens: Vec::new(),
        recovery: false,
    };
    let mut result = ParseResult {
        statements: Vec::new(),
        stmt_ranges: Vec::new(),
        stmt_ids: Vec::new(),
        id_count: 0,
        tokens: Vec::new(),
        errors: Vec::new(),
    };
    while !parser.at("") {
        let start = parser.token().offset;
        match parser.statement() {
            Ok(statement) => {
                result.statements.push(statement);
                result.stmt_ranges.push((start, parser.pos));
            }
            Err(message) => {
                let offset = parser.token().offset;
                result
                    .statements
                    .push(Statement::Error(offset, message.clone()));
                result.stmt_ranges.push((offset, offset));
                result.errors.push(SyntaxError {
                    start: offset,
                    end: offset,
                    message,
                });
                // Top-level semicolons remain the experimental recovery boundary.
                parser.recovery = true;
                let skip_start = parser.token().offset;
                while !parser.at("") && !parser.at(";") {
                    parser.bump();
                }
                let skip_end = parser.pos.max(skip_start);
                parser.record(
                    skip_start,
                    skip_end,
                    "recovery",
                    &source[skip_start..skip_end],
                );
                parser.recovery = false;
                parser.eat(";");
            }
        }
    }
    result.id_count = assign_ids(
        &mut result.statements,
        &mut result.stmt_ids,
        &mut parser.tokens,
        &mut result.tokens,
    );
    result
}

/// Parse a `.not` source file as the implicit outer Content literal.
pub fn parse_markup(source: &str) -> Result<Vec<Expr>, String> {
    let mut parser = Parser {
        source,
        pos: 0,
        depth: 0,
        tokens: Vec::new(),
        recovery: false,
    };
    parser.markup_root()
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
            "use" | "wasm" | "as" | "let" | "if" | "else" | "none" | "true" | "false" => "keyword",
            _ => "name",
        }
    } else {
        "punct"
    }
}

/// Assign pre-order node ids (statements and expressions share one counter,
/// starting at 1) and move token events out of the parser. Returns the total
/// id count (ids are dense 1..=count).
fn assign_ids(
    statements: &mut [Statement],
    stmt_ids: &mut Vec<usize>,
    tokens: &mut Vec<TokenEvent>,
    out_tokens: &mut Vec<TokenEvent>,
) -> usize {
    let mut next = 1;
    fn walk(expr: &mut Expr, next: &mut usize) {
        expr.id = *next;
        *next += 1;
        match &mut expr.kind {
            ExprKind::Section(_, title, body) => {
                for child in title.iter_mut().chain(body.iter_mut()) {
                    walk(child, next);
                }
            }
            ExprKind::Declaration(statement) => match statement.as_mut() {
                Statement::Let(_, value) | Statement::Expression(value) => walk(value, next),
                _ => {}
            },
            ExprKind::List(items) | ExprKind::Content(items) | ExprKind::Styled(_, items) => {
                for item in items {
                    walk(item, next);
                }
            }
            ExprKind::Dict(fields) => {
                for (_, value) in fields {
                    walk(value, next);
                }
            }
            ExprKind::Call(callee, args) => {
                walk(callee, next);
                for arg in args {
                    walk(&mut arg.expr, next);
                }
            }
            ExprKind::Field(base, _) => walk(base, next),
            ExprKind::Lambda(params, body) => {
                for param in params {
                    if let Some(default) = &mut param.default {
                        walk(default, next);
                    }
                }
                walk(body, next);
            }
            ExprKind::If(condition, yes, no) => {
                walk(condition, next);
                walk(yes, next);
                walk(no, next);
            }
            ExprKind::Binary(_, a, b) => {
                walk(a, next);
                walk(b, next);
            }
            ExprKind::None
            | ExprKind::String(_)
            | ExprKind::Int(_)
            | ExprKind::Bool(_)
            | ExprKind::Name(_)
            | ExprKind::Target(_, _) => {}
        }
    }
    stmt_ids.reserve(statements.len());
    for statement in statements {
        stmt_ids.push(next);
        next += 1;
        match statement {
            Statement::Let(_, expr) | Statement::Expression(expr) => walk(expr, &mut next),
            Statement::Wasm(_) | Statement::Use(_) | Statement::Error(..) => {}
        }
    }
    out_tokens.append(tokens);
    next - 1
}

struct Parser<'a> {
    source: &'a str,
    pos: usize,
    depth: usize,
    tokens: Vec<TokenEvent>,
    recovery: bool,
}

impl<'a> Parser<'a> {
    fn record(&mut self, start: usize, end: usize, kind: &'static str, text: &str) {
        let i = self.tokens.len();
        let text = if text.chars().count() > 32 {
            let mut cut: String = text.chars().take(31).collect();
            cut.push('…');
            cut
        } else {
            text.into()
        };
        self.tokens.push(TokenEvent {
            i,
            start,
            end,
            kind,
            text,
            recovery: self.recovery,
        });
    }
    // Lex code on demand: markup must see its original whitespace and punctuation.
    fn token(&self) -> Token<'a> {
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
    fn bump(&mut self) {
        let token = self.token();
        self.pos = token.end;
        self.record(token.offset, token.end, token_kind(&token), token.text);
    }
    fn at(&self, text: &str) -> bool {
        let t = self.token();
        !t.string && t.text == text
    }
    fn eat(&mut self, text: &str) -> bool {
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
        self.record(token.offset, token.end, token_kind(&token), token.text);
        Ok(token.text.into())
    }
    fn string(&mut self) -> Result<String, String> {
        let token = self.token();
        if !token.string {
            return Err("expected a string".into());
        }
        let value = serde_json::from_str(token.text).map_err(|e| format!("invalid string: {e}"))?;
        self.pos = token.end;
        self.record(token.offset, token.end, "string", token.text);
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
            "Item" => Ok(Type::Item),
            "Module" => Ok(Type::Module),
            "Target" => Ok(Type::Target),
            "List" => Ok(Type::List),
            "Dict" => Ok(Type::Dict),
            "Function" => Ok(Type::Function),
            "Any" => Ok(Type::Any),
            "None" => Ok(Type::None),
            other => Err(format!("unknown type `{other}`")),
        }
    }
    fn statement(&mut self) -> Result<Statement, String> {
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
            self.expect("=")?;
            let expr = self.expr(0)?;
            self.expect(";")?;
            return Ok(Statement::Let(name, expr));
        }
        let expr = self.expr(0)?;
        self.expect(";")?;
        Ok(Statement::Expression(expr))
    }
    fn segment(&mut self) -> Result<String, String> {
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
    fn expr(&mut self, min: u8) -> Result<Expr, String> {
        if self.depth >= 128 {
            return Err("expression nesting limit exceeded".into());
        }
        self.depth += 1;
        let result = self.expression(min);
        self.depth -= 1;
        result
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
                self.expect("=>")?;
                ExprKind::Lambda(params, Box::new(self.expr(0)?))
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
        } else if self.eat("none") {
            ExprKind::None
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
            let mut name = self.name()?;
            let mut item = None;
            while self.eat("::") {
                if self.token().string {
                    item = Some(self.string()?);
                    break;
                }
                name.push_str("::");
                name.push_str(&self.segment()?);
            }
            match item {
                Some(item) => {
                    ExprKind::Target(name.split("::").map(str::to_owned).collect(), Some(item))
                }
                None => ExprKind::Name(name),
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
        let lambda = self.at("=>");
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
    fn markup(&mut self, end: char) -> Result<Vec<Expr>, String> {
        if self.depth >= 128 {
            return Err("content nesting limit exceeded".into());
        }
        self.depth += 1;
        let result = self.markup_inner(Some(end), 0);
        self.depth -= 1;
        result
    }
    fn markup_root(&mut self) -> Result<Vec<Expr>, String> {
        if self.depth >= 128 {
            return Err("content nesting limit exceeded".into());
        }
        self.depth += 1;
        let result = self.markup_inner(None, 0);
        self.depth -= 1;
        result
    }
    fn markup_inner(
        &mut self,
        end: Option<char>,
        section_level: usize,
    ) -> Result<Vec<Expr>, String> {
        let mut parts = Vec::new();
        let mut text = String::new();
        let mut start = self.pos;
        loop {
            let offset = self.pos;
            let line_start = self.pos == 0 || self.source[..self.pos].ends_with('\n');
            let level = self.source[self.pos..]
                .chars()
                .take_while(|c| *c == '=')
                .count();
            let heading =
                line_start && level > 0 && self.source[self.pos + level..].starts_with(' ');
            let section_end = section_level > 0
                && (heading && level <= section_level
                    || end.is_some_and(|c| self.source[self.pos..].starts_with(c)));
            if heading || section_end {
                if !text.is_empty() {
                    parts.push(Expr {
                        offset: start,
                        end: offset,
                        id: 0,
                        kind: ExprKind::String(std::mem::take(&mut text)),
                    });
                }
                if section_end {
                    return Ok(parts);
                }
                if self.depth >= 128 {
                    return Err("section nesting limit exceeded".into());
                }
                self.pos += level + 1;
                self.depth += 1;
                let title = self.markup_inner(Some('\n'), 0)?;
                let body = self.markup_inner(end, level)?;
                self.depth -= 1;
                parts.push(Expr {
                    offset,
                    end: self.pos,
                    id: 0,
                    kind: ExprKind::Section(level, title, body),
                });
                start = self.pos;
                continue;
            }
            if self.source[self.pos..].starts_with("[[") {
                if !text.is_empty() {
                    self.record(start, offset, "markup-text", &self.source[start..offset]);
                    parts.push(Expr {
                        offset: start,
                        end: offset,
                        id: 0,
                        kind: ExprKind::String(std::mem::take(&mut text)),
                    });
                }
                let target_start = self.pos + 2;
                let close = self.source[target_start..]
                    .find("]]")
                    .ok_or("unclosed wikilink")?;
                let target_end = target_start + close;
                let raw = &self.source[target_start..target_end];
                let (module, item) = raw
                    .split_once('#')
                    .map_or((raw, None), |(m, i)| (m, Some(i)));
                if module.is_empty()
                    || module.split("::").any(|part| part.is_empty())
                    || item.is_some_and(str::is_empty)
                {
                    return Err("invalid wikilink target".into());
                }
                self.record(
                    self.pos,
                    target_end + 2,
                    "wikilink",
                    &self.source[self.pos..target_end + 2],
                );
                self.pos = target_end + 2;
                parts.push(Expr {
                    offset,
                    end: self.pos,
                    id: 0,
                    kind: ExprKind::Target(
                        module.split("::").map(str::to_owned).collect(),
                        item.map(str::to_owned),
                    ),
                });
                start = self.pos;
                continue;
            }
            let Some(c) = self.source[self.pos..].chars().next() else {
                if let Some(end) = end.filter(|c| *c != '\n') {
                    return Err(format!("unclosed markup, expected `{end}`"));
                }
                if !text.is_empty() {
                    self.record(
                        start,
                        self.pos,
                        "markup-text",
                        &self.source[start..self.pos],
                    );
                    parts.push(Expr {
                        offset: start,
                        end: self.pos,
                        id: 0,
                        kind: ExprKind::String(text),
                    });
                }
                return Ok(parts);
            };
            if end.is_some_and(|end| c == end) || matches!(c, '#' | '[' | '*' | '_') {
                if !text.is_empty() {
                    self.record(start, offset, "markup-text", &self.source[start..offset]);
                    parts.push(Expr {
                        offset: start,
                        end: offset,
                        id: 0,
                        kind: ExprKind::String(std::mem::take(&mut text)),
                    });
                }
                self.pos += c.len_utf8();
                if end.is_some_and(|end| c == end) {
                    self.record(
                        offset,
                        self.pos,
                        "markup-close",
                        &self.source[offset..self.pos],
                    );
                    return Ok(parts);
                }
                let expr = match c {
                    '#' => {
                        self.record(offset, self.pos, "markup-interp", "#");
                        if self.at("let") || self.at("use") || self.at("wasm") {
                            let statement = self.statement()?;
                            Expr {
                                offset,
                                end: self.pos,
                                id: 0,
                                kind: ExprKind::Declaration(Box::new(statement)),
                            }
                        } else {
                            self.expr(5)?
                        }
                    }
                    '[' => {
                        self.record(offset, self.pos, "markup-open", "[");
                        let inner = self.markup(']')?;
                        Expr {
                            offset,
                            end: self.pos,
                            id: 0,
                            kind: ExprKind::Content(inner),
                        }
                    }
                    '*' | '_' => {
                        self.record(
                            offset,
                            self.pos,
                            "markup-style",
                            &self.source[offset..self.pos],
                        );
                        let inner = self.markup(c)?;
                        Expr {
                            offset,
                            end: self.pos,
                            id: 0,
                            kind: ExprKind::Styled(if c == '*' { "strong" } else { "em" }, inner),
                        }
                    }
                    _ => unreachable!(),
                };
                parts.push(expr);
                start = self.pos;
            } else if c == '\\' {
                self.pos += 1;
                let next = self.source[self.pos..]
                    .chars()
                    .next()
                    .ok_or("unfinished markup escape")?;
                self.pos += next.len_utf8();
                self.record(
                    offset,
                    self.pos,
                    "markup-escape",
                    &self.source[offset..self.pos],
                );
                text.push(next);
            } else if c == ']' {
                return Err(end.map_or_else(
                    || "unexpected `]` in markup".into(),
                    |end| format!("unclosed markup, expected `{end}` before `]`"),
                ));
            } else {
                self.pos += c.len_utf8();
                text.push(c);
            }
        }
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
