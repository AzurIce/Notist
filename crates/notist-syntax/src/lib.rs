//! Markup and Code parsers sharing one expression representation.
mod code;
mod markup;
mod state;

use code::CodeParser;
use markup::MarkupParser;
use state::ParseState;

pub use notist_model::Type;

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
                | "true"
                | "false"
                | "self"
                | "super"
                | "vault"
        )
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub offset: usize,
    pub end: usize,
    /// Pre-order node id within its module's syntax tree (0 before assignment).
    pub id: usize,
    pub kind: ExprKind,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Typed(Type, Box<Expr>),
    Element(String, Vec<(String, Expr)>),
    Annotation(bool, Box<Expr>),
    Unit,
    String(String),
    Int(i64),
    Bool(bool),
    Name(String),
    /// Module segments and ordered label constraints; an empty label path targets a module.
    Target(Vec<String>, Vec<String>),
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
    let mut state = ParseState::new(source, false);
    let mut parser = MarkupParser::new(&mut state);
    let mut result = ParseResult {
        statements: Vec::new(),
        stmt_ranges: Vec::new(),
        stmt_ids: Vec::new(),
        id_count: 0,
        tokens: Vec::new(),
        errors: Vec::new(),
    };
    match parser.parse_root() {
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
    let mut state = ParseState::new(source, false);
    let mut parser = CodeParser::new(&mut state);
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
    parser.record_code_trivia(parser.token().offset);
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
    let mut state = ParseState::new(source, false);
    MarkupParser::new(&mut state).parse_root()
}

/// Parse documentation without code interpolation or attribute expressions.
pub fn parse_documentation(source: &str) -> Result<Vec<Expr>, String> {
    let mut state = ParseState::new(source, true);
    MarkupParser::new(&mut state).parse_root()
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
            ExprKind::Dict(fields) | ExprKind::Element(_, fields) => {
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
            ExprKind::Field(base, _) | ExprKind::Annotation(_, base) | ExprKind::Typed(_, base) => {
                walk(base, next)
            }
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
            ExprKind::Unit
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

impl ParseResult {
    /// Find an expression using the pre-order IDs assigned by the parser.
    /// Binary searches skip sibling subtrees; only the containing branch is visited.
    pub fn expression(&self, id: usize) -> Option<&Expr> {
        fn statement(s: &Statement) -> Option<&Expr> {
            match s {
                Statement::Let(_, e) | Statement::Expression(e) => Some(e),
                _ => None,
            }
        }
        fn floor<T>(items: &[T], id: usize, key: impl Fn(&T) -> usize) -> Option<&T> {
            let index = items
                .partition_point(|item| key(item) <= id)
                .checked_sub(1)?;
            items.get(index)
        }
        fn find(e: &Expr, id: usize) -> Option<&Expr> {
            if e.id == id {
                return Some(e);
            }
            if e.id > id {
                return None;
            }
            let next = match &e.kind {
                ExprKind::Section(_, title, body) => {
                    floor(body, id, |e| e.id).or_else(|| floor(title, id, |e| e.id))
                }
                ExprKind::Content(v) | ExprKind::List(v) | ExprKind::Styled(_, v) => {
                    floor(v, id, |e| e.id)
                }
                ExprKind::Dict(v) | ExprKind::Element(_, v) => {
                    floor(v, id, |(_, e)| e.id).map(|(_, e)| e)
                }
                ExprKind::Declaration(s) => statement(s),
                ExprKind::Field(e, _) | ExprKind::Typed(_, e) | ExprKind::Annotation(_, e) => {
                    Some(e.as_ref())
                }
                ExprKind::Call(f, args) => floor(args, id, |a| a.expr.id)
                    .map(|a| &a.expr)
                    .or(Some(f.as_ref())),
                ExprKind::Lambda(params, body) => {
                    if body.id <= id {
                        Some(body.as_ref())
                    } else {
                        params
                            .iter()
                            .filter_map(|p| p.default.as_ref())
                            .take_while(|e| e.id <= id)
                            .last()
                    }
                }
                ExprKind::If(a, b, c) => Some(
                    if c.id <= id {
                        c
                    } else if b.id <= id {
                        b
                    } else {
                        a
                    }
                    .as_ref(),
                ),
                ExprKind::Binary(_, a, b) => Some(if b.id <= id { b } else { a }.as_ref()),
                _ => None,
            }?;
            find(next, id)
        }
        let index = self
            .stmt_ids
            .partition_point(|&start| start <= id)
            .checked_sub(1)?;
        find(statement(self.statements.get(index)?)?, id)
    }

    /// Expressions in source-tree order, including unexecuted bodies and defaults.
    pub fn expressions(&self) -> Vec<&Expr> {
        fn statement<'a>(s: &'a Statement, out: &mut Vec<&'a Expr>) {
            match s {
                Statement::Let(_, e) | Statement::Expression(e) => visit(e, out),
                _ => {}
            }
        }
        fn visit<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
            out.push(e);
            match &e.kind {
                ExprKind::Section(_, a, b) => {
                    for e in a.iter().chain(b) {
                        visit(e, out);
                    }
                }
                ExprKind::Content(v) | ExprKind::List(v) | ExprKind::Styled(_, v) => {
                    for e in v {
                        visit(e, out);
                    }
                }
                ExprKind::Dict(v) | ExprKind::Element(_, v) => {
                    for (_, e) in v {
                        visit(e, out);
                    }
                }
                ExprKind::Declaration(s) => statement(s, out),
                ExprKind::Call(f, args) => {
                    visit(f, out);
                    for arg in args {
                        visit(&arg.expr, out);
                    }
                }
                ExprKind::Lambda(params, body) => {
                    for p in params {
                        if let Some(e) = &p.default {
                            visit(e, out);
                        }
                    }
                    visit(body, out);
                }
                ExprKind::Field(e, _) | ExprKind::Typed(_, e) | ExprKind::Annotation(_, e) => {
                    visit(e, out)
                }
                ExprKind::If(a, b, c) => {
                    visit(a, out);
                    visit(b, out);
                    visit(c, out);
                }
                ExprKind::Binary(_, a, b) => {
                    visit(a, out);
                    visit(b, out);
                }
                _ => {}
            }
        }
        let mut out = vec![];
        for s in &self.statements {
            statement(s, &mut out);
        }
        out
    }
}
