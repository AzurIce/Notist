//! Bounded, static type information for editor queries. Never evaluates packages.
use crate::Workspace;
use notist_next::syntax::{Expr, ExprKind, Statement, Type};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) enum InferredType {
    #[default]
    Unknown,
    Known(Type),
    Function(Vec<InferredType>, Box<InferredType>),
    Dict(BTreeMap<String, InferredType>),
}
impl InferredType {
    fn incompatible(&self, expected: &Type) -> bool {
        if *expected == Type::Any || matches!(self, Self::Unknown | Self::Known(Type::Any)) {
            return false;
        }
        if let Type::Optional(inner) = expected {
            return !matches!(self, Self::Known(Type::None)) && self.incompatible(inner);
        }
        match self {
            Self::Known(Type::Item) if *expected == Type::Content => false,
            Self::Known(Type::Optional(inner)) => {
                Self::Known(*inner.clone()).incompatible(expected)
            }
            Self::Known(actual) => actual != expected,
            Self::Dict(_) => *expected != Type::Dict,
            Self::Function(..) => *expected != Type::Function,
            Self::Unknown => false,
        }
    }
    pub fn result(&self) -> Self {
        match self {
            Self::Function(_, result) => *result.clone(),
            _ => Self::Unknown,
        }
    }
}
impl fmt::Display for InferredType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => f.write_str("{unknown}"),
            Self::Known(Type::Optional(inner)) => write!(f, "{}?", Self::Known(*inner.clone())),
            Self::Known(ty) => write!(f, "{ty:?}"),
            Self::Dict(_) => f.write_str("Dict"),
            Self::Function(params, result) => write!(
                f,
                "({}) -> {result}",
                params
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}
type Env = BTreeMap<String, InferredType>;
#[derive(Default, Clone)]
pub(crate) struct TypeInfo {
    pub errors: Vec<(usize, usize, String)>,
    pub bindings: BTreeMap<usize, InferredType>,
    pub expressions: BTreeMap<(usize, usize), InferredType>,
    exports: Env,
}
pub(crate) struct Analysis<'a> {
    workspace: &'a Workspace,
    cache: BTreeMap<String, TypeInfo>,
    active: BTreeSet<String>,
    remaining: usize,
}
impl<'a> Analysis<'a> {
    pub fn new(workspace: &'a Workspace) -> Self {
        Self {
            workspace,
            cache: BTreeMap::new(),
            active: BTreeSet::new(),
            remaining: 20_000,
        }
    }
    pub fn document(&mut self, uri: &str) -> TypeInfo {
        if let Some(info) = self.cache.get(uri) {
            return info.clone();
        }
        if self.active.len() >= 64 || !self.active.insert(uri.into()) {
            return TypeInfo::default();
        }
        let mut info = TypeInfo::default();
        let mut env = Env::new();
        if let Some(doc) = self.workspace.documents.get(uri) {
            for (statement, (start, _)) in doc.parsed.statements.iter().zip(&doc.parsed.stmt_ranges)
            {
                self.statement(uri, statement, *start, &mut env, &mut info, 0);
            }
        }
        info.exports = env;
        self.active.remove(uri);
        self.cache.insert(uri.into(), info.clone());
        info
    }
    fn resolve(&mut self, uri: &str, name: &str, at: usize) -> InferredType {
        let path = name.split("::").map(str::to_owned).collect::<Vec<_>>();
        let Some((target, symbol)) = self.workspace.resolve_path(uri, &path, at, 0) else {
            return builtin(name);
        };
        match symbol {
            Some(symbol) => self
                .document(&target)
                .exports
                .get(&symbol)
                .cloned()
                .unwrap_or_default(),
            None => InferredType::Known(Type::Module),
        }
    }
    fn statement(
        &mut self,
        uri: &str,
        statement: &Statement,
        start: usize,
        env: &mut Env,
        info: &mut TypeInfo,
        depth: usize,
    ) {
        match statement {
            Statement::Let(name, value) => {
                let ty = self.expr(uri, value, env, info, depth + 1);
                info.bindings.insert(start, ty.clone());
                env.insert(name.clone(), ty);
            }
            Statement::Expression(e) => {
                self.expr(uri, e, env, info, depth + 1);
            }
            Statement::Use(imports) => {
                for import in imports {
                    if import.glob {
                        if let Some((target, None)) =
                            self.workspace.resolve_path(uri, &import.path, start, 0)
                        {
                            env.extend(self.document(&target).exports);
                        }
                    } else if let Some(name) = import.alias.as_ref().or(import.path.last()) {
                        env.insert(
                            name.clone(),
                            self.resolve(uri, &import.path.join("::"), start),
                        );
                    }
                }
            }
            _ => {}
        }
    }
    fn expr(
        &mut self,
        uri: &str,
        expr: &Expr,
        env: &Env,
        info: &mut TypeInfo,
        depth: usize,
    ) -> InferredType {
        use InferredType::{Known, Unknown};
        if depth >= 128 || self.remaining == 0 {
            return Unknown;
        }
        self.remaining -= 1;
        let mut infer = |e: &Expr| self.expr(uri, e, env, info, depth + 1);
        let ty = match &expr.kind {
            ExprKind::Typed(expected, value) => {
                let actual = infer(value);
                if actual.incompatible(expected) {
                    info.errors.push((
                        expr.offset,
                        expr.end,
                        format!(
                            "type annotation expected {}, got {actual}",
                            InferredType::Known(expected.clone())
                        ),
                    ));
                }
                if *expected == Type::Function && matches!(actual, InferredType::Function(..)) {
                    actual
                } else {
                    Known(expected.clone())
                }
            }
            ExprKind::String(_) => Known(Type::String),
            ExprKind::Int(_) => Known(Type::Int),
            ExprKind::Bool(_) => Known(Type::Bool),
            ExprKind::None => Known(Type::None),
            ExprKind::Target(..) => Known(Type::Target),
            ExprKind::Name(name) => env
                .get(name)
                .cloned()
                .unwrap_or_else(|| self.resolve(uri, name, expr.offset)),
            ExprKind::List(values) => {
                for e in values {
                    infer(e);
                }
                Known(Type::List)
            }
            ExprKind::Dict(fields) => {
                InferredType::Dict(fields.iter().map(|(k, e)| (k.clone(), infer(e))).collect())
            }
            ExprKind::Element(_, fields) => {
                for (_, e) in fields {
                    infer(e);
                }
                Known(Type::Item)
            }
            ExprKind::Content(values) | ExprKind::Styled(_, values) => {
                let mut local = env.clone();
                for e in values {
                    if let ExprKind::Declaration(s) = &e.kind {
                        self.statement(uri, s, e.offset, &mut local, info, depth + 1);
                    } else {
                        self.expr(uri, e, &local, info, depth + 1);
                    }
                }
                Known(if matches!(expr.kind, ExprKind::Styled(..)) {
                    Type::Item
                } else {
                    Type::Content
                })
            }
            ExprKind::Section(_, title, body) => {
                for values in [title, body] {
                    let mut local = env.clone();
                    for e in values {
                        if let ExprKind::Declaration(s) = &e.kind {
                            self.statement(uri, s, e.offset, &mut local, info, depth + 1);
                        } else {
                            self.expr(uri, e, &local, info, depth + 1);
                        }
                    }
                }
                Known(Type::Item)
            }
            ExprKind::Lambda(params, body) => {
                let mut local = env.clone();
                for param in params {
                    if let Some(default) = &param.default {
                        self.expr(uri, default, env, info, depth + 1);
                    }
                    local.insert(param.name.clone(), Known(param.ty.clone()));
                }
                let result = self.expr(uri, body, &local, info, depth + 1);
                InferredType::Function(
                    params.iter().map(|p| Known(p.ty.clone())).collect(),
                    Box::new(result),
                )
            }
            ExprKind::Call(callee, args) => {
                let function = infer(callee);
                for arg in args {
                    infer(&arg.expr);
                }
                function.result()
            }
            ExprKind::Field(base, field) => match infer(base) {
                InferredType::Dict(fields) => fields.get(field).cloned().unwrap_or_default(),
                Known(Type::Item) if field == "name" => Known(Type::String),
                Known(Type::Item) if matches!(field.as_str(), "args" | "attributes") => {
                    Known(Type::Dict)
                }
                _ => Unknown,
            },
            ExprKind::If(condition, yes, no) => {
                infer(condition);
                let a = infer(yes);
                let b = infer(no);
                if a == b { a } else { Unknown }
            }
            ExprKind::Binary(op, left, right) => {
                let a = infer(left);
                let b = infer(right);
                match (op.as_str(), &a, &b) {
                    ("==" | "!=", _, _) => Known(Type::Bool),
                    ("<" | ">" | "<=" | ">=", Known(Type::Int), Known(Type::Int)) => {
                        Known(Type::Bool)
                    }
                    ("+" | "-" | "*" | "/", Known(Type::Int), Known(Type::Int)) => Known(Type::Int),
                    ("+", Known(Type::String), Known(Type::String)) => Known(Type::String),
                    ("+", Known(Type::Content | Type::Item), Known(Type::Content | Type::Item)) => {
                        Known(Type::Content)
                    }
                    _ => Unknown,
                }
            }
            ExprKind::Annotation(_, e) => {
                infer(e);
                Known(Type::None)
            }
            ExprKind::Declaration(s) => {
                self.statement(uri, s, expr.offset, &mut env.clone(), info, depth + 1);
                Known(Type::None)
            }
        };
        info.expressions.insert((expr.offset, expr.end), ty.clone());
        ty
    }
}
fn builtin(name: &str) -> InferredType {
    use Type::*;
    let (params, result) = match name {
        "text" => (vec![String], Content),
        "math" | "raw" => (vec![String], Item),
        "item" => (vec![String, Dict], Item),
        "link" => (vec![String, Any], Item),
        "str" => (vec![Any], String),
        "len" => (vec![Any], Int),
        "concat" => (vec![List], Content),
        "map" => (vec![Function, List], List),
        "is_error" => (vec![Any], Bool),
        _ => return InferredType::Unknown,
    };
    InferredType::Function(
        params.into_iter().map(InferredType::Known).collect(),
        Box::new(InferredType::Known(result)),
    )
}
