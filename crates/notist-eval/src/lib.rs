//! Evaluation over host-provided syntax, module identities and resources.
mod targets;
mod wasm;
pub use targets::{OutputLink, TargetObservation, expand_module_address};

use notist_ir::Closure;
pub use notist_ir::{Content, Env, Item, Value};
use notist_model::{
    CreationOrigin, Diagnostic, DiagnosticCode, Location, OriginKind, Target, Type,
};
use notist_syntax::{Expr, ExprKind, Param, ParseResult, Statement};
use serde_json::{Value as Json, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

#[derive(Clone)]
struct Argument {
    name: Option<String>,
    trailing: bool,
    value: Value,
}

/// Read-only inputs supplied by the host. Source names are opaque to evaluation.
pub trait ModuleProvider {
    fn parsed(&self, source: &str) -> Option<&ParseResult>;
    /// Canonical `package::module` key; the first segment identifies the package.
    fn module_key(&self, source: &str) -> Option<&str>;
    fn module_source(&self, key: &str) -> Option<&str>;
    fn dependency(&self, package: &str, alias: &str) -> Option<&str>;
    fn binary(&self, path: &str) -> Option<&[u8]>;
    /// A host may retain an unavailable resource as a source-local evaluation failure.
    fn binary_error(&self, _path: &str) -> Option<&str> {
        None
    }
    fn binary_path(&self, source: &str, path: &str) -> Result<String, String>;
    /// Setup failures which prevent evaluation of this input set.
    fn errors(&self) -> &[String];
}

pub(crate) fn matches_type(ty: &Type, v: &Value) -> bool {
    match ty {
        Type::Any => true,
        Type::Optional(t) => matches!(v, Value::None) || matches_type(t, v),
        _ => *ty == v.ty(),
    }
}

#[derive(Clone, Debug)]
pub struct Evaluation {
    pub raw_content: Content,
    pub content: Content,
    pub attributes: Env,
    pub warnings: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
    pub references: Vec<TargetObservation>,
    pub output_links: Vec<OutputLink>,
}

pub struct Runtime<'a> {
    pub document_rules: notist_ir::document::DocumentRules,
    world: &'a dyn ModuleProvider,
    pub module_attributes: BTreeMap<String, Env>,
    pub used_wasm: Vec<String>,
    pub events: Vec<Json>,
    trace_calls: bool,
    modules: BTreeMap<String, Env>,
    contents: BTreeMap<String, Content>,
    references: Vec<TargetObservation>,
    loading: BTreeSet<String>,
    registry: BTreeMap<String, Env>,
    steps: usize,
}

impl<'a> Runtime<'a> {
    pub fn new(world: &'a dyn ModuleProvider) -> Self {
        Self {
            document_rules: Default::default(),
            world,
            module_attributes: BTreeMap::new(),
            used_wasm: Vec::new(),
            events: Vec::new(),
            trace_calls: false,
            modules: BTreeMap::new(),
            contents: BTreeMap::new(),
            references: Vec::new(),
            loading: BTreeSet::new(),
            registry: BTreeMap::new(),
            steps: 20_000,
        }
    }
    pub fn evaluate(&mut self, source: &str) -> Evaluation {
        self.evaluate_with_env(source).0
    }
    /// Opt into debugger call events. Ordinary evaluation does not serialize them.
    pub fn with_call_trace(mut self) -> Self {
        self.trace_calls = true;
        self
    }
    pub fn evaluate_with_env(&mut self, source: &str) -> (Evaluation, Env) {
        self.document_rules = Default::default();
        self.module_attributes.clear();
        self.modules.clear();
        self.contents.clear();
        self.references.clear();
        self.loading.clear();
        self.registry.clear();
        self.used_wasm.clear();
        self.events.clear();
        self.steps = 20_000;
        let errors = self.world.errors();
        let location = Location {
            source: source.into(),
            offset: 0,
        };
        let (env, content) = if errors.is_empty() {
            self.module(source, 0)
        } else {
            (
                Env::new(),
                Content::seq(
                    errors
                        .iter()
                        .map(|message| {
                            Content::error(message, DiagnosticCode::Setup, location.clone())
                        })
                        .collect(),
                ),
            )
        };
        let raw_content = content;
        let formed = self.document_rules.form(&raw_content);
        let content = formed.content;
        let output_links = targets::output_links(&content);
        let mut diagnostics = self.module_diagnostics(source, &content);
        for diagnostic in formed.diagnostics {
            if !diagnostics.contains(&diagnostic) {
                diagnostics.push(diagnostic);
            }
        }
        let attributes = self
            .module_attributes
            .get(source)
            .cloned()
            .unwrap_or_default();
        let warnings = diagnostics.iter().map(ToString::to_string).collect();
        (
            Evaluation {
                raw_content,
                content,
                attributes,
                warnings,
                diagnostics,
                references: self.references.clone(),
                output_links,
            },
            env,
        )
    }
    // A dependency failure is propagated by import resolution as an Error value.
    // Do not reintroduce errors from dependency caches after callers recover them.
    fn module_diagnostics(&self, source: &str, content: &Content) -> Vec<Diagnostic> {
        let mut diagnostics = content.diagnostics();
        if let Some(attributes) = self.module_attributes.get(source) {
            for value in attributes.values() {
                for diagnostic in value.diagnostics() {
                    if !diagnostics.contains(&diagnostic) {
                        diagnostics.push(diagnostic);
                    }
                }
            }
        }
        diagnostics
    }
    fn failure(message: impl Into<String>, location: &Location) -> Value {
        Value::Content(Content::error(
            message,
            DiagnosticCode::Evaluation,
            location.clone(),
        ))
    }
    pub fn content(value: Value, location: &Location) -> Content {
        match value {
            Value::None => Content::seq(vec![]).at(location),
            Value::Content(c) => c,
            Value::Target(target) => Content::link(target, location.clone()),
            Value::String(s) => Content::text(s).at(location),
            Value::Int(n) => Content::text(n.to_string()).at(location),
            Value::List(v) => {
                Content::seq(v.into_iter().map(|v| Self::content(v, location)).collect())
                    .at(location)
            }
            v => Content::error(
                format!("expected Content, found {:?}", v.ty()),
                DiagnosticCode::Evaluation,
                location.clone(),
            ),
        }
    }
    fn module(&mut self, source: &str, depth: usize) -> (Env, Content) {
        let location = Location {
            source: source.into(),
            offset: 0,
        };
        if let Some(env) = self.modules.get(source) {
            return (
                env.clone(),
                self.contents
                    .get(source)
                    .cloned()
                    .unwrap_or_else(|| Content::seq(vec![])),
            );
        }
        if depth >= 128 || !self.loading.insert(source.into()) {
            return (
                Env::new(),
                Self::content(
                    Self::failure("cyclic or too deeply nested module import", &location),
                    &location,
                ),
            );
        }
        let Some(parsed) = self.world.parsed(source) else {
            self.loading.remove(source);
            if self.world.module_key(source).is_some() {
                self.modules.insert(source.into(), Env::new());
                return (Env::new(), Content::seq(vec![]));
            }
            return (
                Env::new(),
                Self::content(
                    Self::failure(format!("missing module `{source}`"), &location),
                    &location,
                ),
            );
        };
        let statements = parsed
            .statements
            .iter()
            .cloned()
            .zip(parsed.stmt_ranges.iter().map(|r| r.0))
            .collect();
        let result = self.statements(statements, Env::new(), source, depth);
        self.loading.remove(source);
        self.modules.insert(source.into(), result.0.clone());
        self.contents.insert(source.into(), result.1.clone());
        result
    }
    fn statements(
        &mut self,
        statements: Vec<(Statement, usize)>,
        mut env: Env,
        source: &str,
        depth: usize,
    ) -> (Env, Content) {
        let mut exports = Env::new();
        let mut output = Vec::new();
        for (statement, statement_offset) in statements {
            let location = Location {
                source: source.into(),
                offset: statement_offset,
            };
            match statement {
                Statement::Let(name, expr) => {
                    if env.contains_key(&name) {
                        output.push(Self::content(
                            Self::failure(format!("duplicate binding `{name}`"), &location),
                            &location,
                        ));
                        continue;
                    }
                    let mut value = self.eval(&expr, &env, source, depth + 1);
                    if let Value::Closure(closure) = &mut value {
                        Rc::make_mut(closure).name = Some(name.clone());
                    }
                    if value.is_error() {
                        output.push(Self::content(value.clone(), &location));
                    }
                    env.insert(name.clone(), value.clone());
                    exports.insert(name, value);
                }
                Statement::Expression(expr) => {
                    if let ExprKind::Annotation(module, value) = &expr.kind {
                        let location = Location {
                            source: source.into(),
                            offset: expr.offset,
                        };
                        match self.eval(value, &env, source, depth + 1) {
                            Value::Dict(attributes) => {
                                if *module {
                                    self.module_attributes
                                        .entry(source.into())
                                        .or_default()
                                        .extend(attributes);
                                } else {
                                    output.push(Item::new(
                                        "annotation",
                                        Env::from([("attributes".into(), Value::Dict(attributes))]),
                                        location,
                                    ));
                                }
                            }
                            value if value.is_error() => {
                                output.push(Self::content(value, &location))
                            }
                            _ => output.push(Self::content(
                                Self::failure(
                                    "annotation expression must evaluate to Dict",
                                    &location,
                                ),
                                &location,
                            )),
                        }
                        continue;
                    }
                    let value = self.eval(&expr, &env, source, depth + 1);
                    if matches!(value, Value::None) {
                        continue;
                    }
                    let mut content = Self::content(
                        value,
                        &Location {
                            source: source.into(),
                            offset: expr.offset,
                        },
                    );
                    if content.span.is_none()
                        && content.location.source == source
                        && content.location.offset == expr.offset
                    {
                        content.span = Some(notist_model::SourceSpan {
                            source: source.into(),
                            start: expr.offset,
                            end: expr.end,
                        });
                    }
                    output.push(content);
                }
                Statement::Error(offset, message) => output.push(Content::error(
                    message,
                    DiagnosticCode::Syntax,
                    Location {
                        source: source.into(),
                        offset,
                    },
                )),
                Statement::Use(imports) => {
                    for import in imports {
                        match self.resolve(&import.path, &env, source, depth + 1) {
                            Ok(value) => {
                                let bindings = if import.glob {
                                    if let Value::Module(path) = value {
                                        self.module(&path, depth + 1).0
                                    } else {
                                        output.push(Self::content(
                                            Self::failure("glob requires module", &location),
                                            &location,
                                        ));
                                        continue;
                                    }
                                } else {
                                    BTreeMap::from([(
                                        import
                                            .alias
                                            .unwrap_or_else(|| import.path.last().unwrap().clone()),
                                        value,
                                    )])
                                };
                                for (name, value) in bindings {
                                    match env.entry(name) {
                                        std::collections::btree_map::Entry::Occupied(entry) => {
                                            output.push(Self::content(
                                                Self::failure(
                                                    format!("duplicate binding `{}`", entry.key()),
                                                    &location,
                                                ),
                                                &location,
                                            ))
                                        }
                                        std::collections::btree_map::Entry::Vacant(entry) => {
                                            entry.insert(value);
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                output.push(Self::content(Self::failure(e, &location), &location))
                            }
                        }
                    }
                }
                Statement::Wasm(path) => {
                    let result = self
                        .world
                        .binary_path(source, &path)
                        .and_then(|path| self.register(&path, &location));
                    match result {
                        Ok(bindings) => {
                            if let Some(name) = bindings.keys().find(|k| env.contains_key(*k)) {
                                output.push(Self::content(
                                    Self::failure(
                                        format!("duplicate WASM binding `{name}`"),
                                        &location,
                                    ),
                                    &location,
                                ));
                            } else {
                                env.extend(bindings.clone());
                                exports.extend(bindings);
                            }
                        }
                        Err(e) => {
                            output.push(Self::content(Self::failure(e, &location), &location))
                        }
                    }
                }
            }
        }
        (
            exports,
            Content::seq(output).at(&Location {
                source: source.into(),
                offset: 0,
            }),
        )
    }
    fn resolve(
        &mut self,
        path: &[String],
        env: &Env,
        source: &str,
        depth: usize,
    ) -> Result<Value, String> {
        if path.len() == 1
            && let Some(value) = env.get(&path[0])
        {
            return Ok(value.clone());
        }
        let key = self
            .path_key(path, env, source)
            .map_err(|e| e.to_string())?;
        self.resolve_key(&key, depth)
    }

    fn resolve_key(&mut self, key: &str, depth: usize) -> Result<Value, String> {
        if let Some(source) = self.world.module_source(key).map(str::to_owned) {
            let (_, content) = self.module(&source, depth + 1);
            let errors: Vec<_> = self
                .module_diagnostics(&source, &content)
                .iter()
                .map(ToString::to_string)
                .collect();
            if !errors.is_empty() {
                return Err(errors.join("; "));
            }
            return Ok(Value::Module(source));
        }
        let Some((parent, name)) = key.rsplit_once("::") else {
            return Err(format!("missing module `{key}`"));
        };
        let source = self
            .world
            .module_source(parent)
            .map(str::to_owned)
            .ok_or_else(|| format!("missing module `{parent}`"))?;
        let (env, content) = self.module(&source, depth + 1);
        let errors: Vec<_> = self
            .module_diagnostics(&source, &content)
            .iter()
            .map(ToString::to_string)
            .collect();
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
        env.get(name)
            .cloned()
            .ok_or_else(|| format!("missing export `{key}`"))
    }
    fn register(&mut self, path: &str, location: &Location) -> Result<Env, String> {
        if let Some(env) = self.registry.get(path) {
            return Ok(env.clone());
        }
        let bytes = self.world.binary(path).ok_or_else(|| {
            self.world
                .binary_error(path)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("missing WASM `{path}`"))
        })?;
        let (env, elements) = wasm::registration(bytes, path, location)?;
        let mut rules = self.document_rules.clone();
        for (name, model) in elements {
            rules.define(name, model)?;
        }
        self.document_rules = rules;
        self.used_wasm.push(path.into());
        self.registry.insert(path.into(), env.clone());
        Ok(env)
    }
    fn eval(&mut self, expr: &Expr, env: &Env, source: &str, depth: usize) -> Value {
        let loc = Location {
            source: source.into(),
            offset: expr.offset,
        };
        if depth >= 128 || self.steps == 0 {
            return Self::failure("evaluation limit exceeded", &loc);
        }
        self.steps -= 1;
        let mut value = match &expr.kind {
            ExprKind::Element(name, fields) => {
                let args = fields
                    .iter()
                    .map(|(key, value)| (key.clone(), self.eval(value, env, source, depth + 1)))
                    .collect();
                Value::Content(Item {
                    span: None,
                    name: name.clone(),
                    args,
                    attributes: Env::new(),
                    origin: Some(CreationOrigin {
                        node_id: Some(expr.id),
                        kind: OriginKind::Syntax,
                    }),
                    location: loc,
                })
            }
            ExprKind::Annotation(_, _) => Self::failure("annotation requires Markup scope", &loc),
            ExprKind::Section(level, title, body) => {
                let mut field = |parts: &Vec<Expr>| {
                    self.eval(
                        &Expr {
                            kind: ExprKind::Content(parts.clone()),
                            ..expr.clone()
                        },
                        env,
                        source,
                        depth + 1,
                    )
                };
                let title = field(title);
                let body = field(body);
                Value::Content(Item {
                    span: None,
                    name: "section".into(),
                    attributes: Env::new(),
                    args: BTreeMap::from([
                        ("level".into(), Value::Int(*level as i64)),
                        ("title".into(), title),
                        ("body".into(), body),
                    ]),
                    origin: Some(CreationOrigin {
                        node_id: Some(expr.id),
                        kind: OriginKind::Syntax,
                    }),
                    location: loc,
                })
            }
            ExprKind::Declaration(_) => Self::failure("declaration requires Content scope", &loc),
            ExprKind::Typed(ty, value) => {
                let value = self.eval(value, env, source, depth + 1);
                if value.is_error() || matches_type(ty, &value) {
                    value
                } else {
                    Self::failure(
                        format!("type annotation expected {ty:?}, got {:?}", value.ty()),
                        &loc,
                    )
                }
            }
            ExprKind::None => Value::None,
            ExprKind::String(v) => Value::String(v.clone()),
            ExprKind::Int(v) => Value::Int(*v),
            ExprKind::Bool(v) => Value::Bool(*v),
            ExprKind::Name(name) => {
                if let Some(v) = env.get(name) {
                    return v.clone();
                }
                if matches!(
                    name.as_str(),
                    "item"
                        | "define_element"
                        | "with_attributes"
                        | "math"
                        | "raw"
                        | "link"
                        | "text"
                        | "error"
                        | "concat"
                        | "map"
                        | "str"
                        | "is_error"
                        | "recover"
                        | "get"
                        | "len"
                ) {
                    return Value::Named(name.clone());
                }
                self.resolve(
                    &name.split("::").map(str::to_owned).collect::<Vec<_>>(),
                    env,
                    source,
                    depth + 1,
                )
                .unwrap_or_else(|e| Self::failure(e, &loc))
            }
            ExprKind::Target(path, labels) => {
                let result = self
                    .target_module(path, env, source)
                    .map(|module| Target::new(module, labels.clone()));
                self.references.push(TargetObservation {
                    location: loc.clone(),
                    node_id: expr.id,
                    result: result.clone(),
                });
                match result {
                    Ok(target) => Value::Target(target),
                    Err(error) => Value::Content(Content::error(
                        error.to_string(),
                        DiagnosticCode::TargetFormation,
                        loc,
                    )),
                }
            }
            ExprKind::List(values) => Value::List(
                values
                    .iter()
                    .map(|e| self.eval(e, env, source, depth + 1))
                    .collect(),
            ),
            ExprKind::Dict(fields) => Value::Dict(
                fields
                    .iter()
                    .map(|(k, e)| (k.clone(), self.eval(e, env, source, depth + 1)))
                    .collect(),
            ),
            ExprKind::Field(base, key) => match self.eval(base, env, source, depth + 1) {
                Value::Dict(fields) => fields
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| Self::failure(format!("missing field `{key}`"), &loc)),
                Value::Content(i) => match key.as_str() {
                    "name" => Value::String(i.name),
                    "args" => Value::Dict(i.args),
                    "attributes" => Value::Dict(i.attributes),
                    _ => Self::failure("Item has name and args fields", &loc),
                },
                v if v.is_error() => v,
                _ => Self::failure("field access requires Dict or Item", &loc),
            },
            ExprKind::Content(parts) | ExprKind::Styled(_, parts) => {
                let statements = parts
                    .iter()
                    .map(|e| {
                        (
                            match &e.kind {
                                ExprKind::Declaration(s) => *s.clone(),
                                _ => Statement::Expression(e.clone()),
                            },
                            e.offset,
                        )
                    })
                    .collect();
                let (_, content) = self.statements(statements, env.clone(), source, depth + 1);
                if let ExprKind::Styled(name, _) = &expr.kind {
                    Value::Content(Item {
                        span: None,
                        name: (*name).into(),
                        attributes: Env::new(),
                        args: BTreeMap::from([("body".into(), Value::Content(content))]),
                        origin: Some(CreationOrigin {
                            node_id: Some(expr.id),
                            kind: OriginKind::Syntax,
                        }),
                        location: loc,
                    })
                } else {
                    let mut content = content.at(&loc);
                    content.origin = Some(CreationOrigin {
                        node_id: Some(expr.id),
                        kind: OriginKind::Syntax,
                    });
                    Value::Content(content)
                }
            }
            ExprKind::Lambda(params, body) => Value::Closure(Rc::new(Closure {
                params: params.clone(),
                body: *body.clone(),
                env: env.clone(),
                source: source.into(),
                name: None,
            })),
            ExprKind::If(cond, yes, no) => match self.eval(cond, env, source, depth + 1) {
                Value::Bool(b) => self.eval(if b { yes } else { no }, env, source, depth + 1),
                v if v.is_error() => v,
                _ => Self::failure("if condition must be Bool", &loc),
            },
            ExprKind::Call(callee, args) => {
                let function = self.eval(callee, env, source, depth + 1);
                let args = args
                    .iter()
                    .map(|a| Argument {
                        name: a.name.clone(),
                        trailing: a.trailing,
                        value: self.eval(&a.expr, env, source, depth + 1),
                    })
                    .collect();
                let kind = if matches!(function, Value::External(_)) {
                    OriginKind::Plugin
                } else {
                    OriginKind::Constructor
                };
                let mut value = self.call(function, args, &loc, depth + 1);
                stamp_origin(&mut value, &loc, expr.id, kind);
                if self.trace_calls && self.events.len() < 4000 {
                    self.events.push(json!({"kind":"call","source":source,"range":[expr.offset,expr.end],"out":value.to_json()}));
                }
                value
            }
            ExprKind::Binary(op, a, b) => {
                let a = self.eval(a, env, source, depth + 1);
                let b = self.eval(b, env, source, depth + 1);
                if a.is_error() {
                    return a;
                }
                if b.is_error() {
                    return b;
                }
                if matches!(op.as_str(), "==" | "!=")
                    && (matches!(a, Value::None) || matches!(b, Value::None))
                {
                    return Value::Bool(
                        matches!((&a, &b), (Value::None, Value::None)) == (op == "=="),
                    );
                }
                match (a, b) {
                    (Value::Int(a), Value::Int(b)) => {
                        let n = match op.as_str() {
                            "+" => a.checked_add(b),
                            "-" => a.checked_sub(b),
                            "*" => a.checked_mul(b),
                            "/" => a.checked_div(b),
                            "==" => return Value::Bool(a == b),
                            "!=" => return Value::Bool(a != b),
                            "<" => return Value::Bool(a < b),
                            ">" => return Value::Bool(a > b),
                            "<=" => return Value::Bool(a <= b),
                            ">=" => return Value::Bool(a >= b),
                            _ => None,
                        };
                        n.map(Value::Int).unwrap_or_else(|| {
                            Self::failure("integer overflow or division by zero", &loc)
                        })
                    }
                    (Value::String(a), Value::String(b)) => match op.as_str() {
                        "+" => Value::String(a + &b),
                        "==" => Value::Bool(a == b),
                        "!=" => Value::Bool(a != b),
                        _ => Self::failure("unsupported String operator", &loc),
                    },
                    (Value::Bool(a), Value::Bool(b)) if op == "==" || op == "!=" => {
                        Value::Bool((a == b) == (op == "=="))
                    }
                    (a, b)
                        if op == "+"
                            && matches_type(&Type::Content, &a)
                            && matches_type(&Type::Content, &b) =>
                    {
                        Value::Content(
                            Content::seq(vec![Self::content(a, &loc), Self::content(b, &loc)])
                                .at(&loc),
                        )
                    }
                    _ => Self::failure("operator argument type mismatch", &loc),
                }
            }
        };
        if let Value::Content(content) = &mut value
            && content.span.is_none()
            && content.location.source == source
            && content.location.offset == expr.offset
        {
            content.span = Some(notist_model::SourceSpan {
                source: source.into(),
                start: expr.offset,
                end: expr.end,
            });
        }
        value
    }
    #[allow(clippy::too_many_arguments)]
    fn bind(
        &mut self,
        params: &[Param],
        args: Vec<Argument>,
        mut env: Env,
        defaults: &Env,
        source: &str,
        loc: &Location,
        depth: usize,
    ) -> Result<(Env, Vec<Value>), String> {
        let mut bound = vec![None; params.len()];
        let mut pos = 0;
        for arg in args {
            let index = if arg.trailing {
                params.iter().rposition(|p| {
                    p.ty == Type::Content || matches!(&p.ty,Type::Optional(t) if **t==Type::Content)
                })
            } else if let Some(name) = arg.name {
                params.iter().position(|p| p.name == name)
            } else {
                while pos < bound.len() && bound[pos].is_some() {
                    pos += 1;
                }
                let i = pos;
                pos += 1;
                (i < bound.len()).then_some(i)
            };
            let index = index.ok_or("unknown or excess argument")?;
            if bound[index].is_some() {
                return Err(format!("duplicate argument `{}`", params[index].name));
            }
            bound[index] = Some(arg.value);
        }
        let mut values = Vec::new();
        for (param, value) in params.iter().zip(bound) {
            let value = match value {
                Some(v) => v,
                None => {
                    if let Some(e) = &param.default {
                        self.eval(e, &env, source, depth + 1)
                    } else if let Some(value) = defaults.get(&param.name) {
                        value.clone()
                    } else if matches!(param.ty, Type::Optional(_)) {
                        Value::None
                    } else {
                        return Err(format!("missing argument `{}`", param.name));
                    }
                }
            };
            if !value.is_error() && !matches_type(&param.ty, &value) {
                return Err(format!(
                    "argument `{}` expects {:?}, found {:?}",
                    param.name,
                    param.ty,
                    value.ty()
                ));
            }
            env.insert(param.name.clone(), value.clone());
            values.push(value);
        }
        let _ = loc;
        Ok((env, values))
    }
    fn call(
        &mut self,
        function: Value,
        args: Vec<Argument>,
        loc: &Location,
        depth: usize,
    ) -> Value {
        if depth >= 128 || self.steps == 0 {
            return Self::failure("evaluation limit exceeded", loc);
        }
        self.steps -= 1;
        match function {
            Value::Closure(c) => {
                let mut env = c.env.clone();
                if let Some(name) = &c.name {
                    env.insert(name.clone(), Value::Closure(c.clone()));
                }
                match self.bind(&c.params, args, env, &Env::new(), &c.source, loc, depth) {
                    Ok((env, _)) => self.eval(&c.body, &env, &c.source, depth + 1),
                    Err(e) => Self::failure(e, loc),
                }
            }
            Value::External(f) => {
                match self.bind(
                    &f.params,
                    args,
                    Env::new(),
                    &f.defaults,
                    &loc.source,
                    loc,
                    depth,
                ) {
                    Ok((_, values)) => self
                        .world
                        .binary(&f.path)
                        .ok_or_else(|| "missing WASM binary".into())
                        .and_then(|bytes| wasm::invoke(bytes, &f.export, &values, &f.result, loc))
                        .unwrap_or_else(|e| Self::failure(e, loc)),
                    Err(e) => Self::failure(e, loc),
                }
            }
            Value::Named(name) => {
                if args.iter().any(|a| a.name.is_some())
                    && !((name == "math" || name == "raw")
                        && args.len() == 1
                        && args[0].name.as_deref() == Some("content"))
                {
                    return Self::failure("builtin requires positional arguments", loc);
                }
                let v = args.into_iter().map(|a| a.value).collect::<Vec<_>>();
                match (name.as_str(), v.as_slice()) {
                    (
                        "define_element",
                        [
                            Value::String(element),
                            Value::Bool(inline),
                            Value::Dict(slots),
                        ],
                    )
                    | (
                        "define_element",
                        [
                            Value::String(element),
                            Value::Bool(inline),
                            Value::Dict(slots),
                            Value::String(_),
                        ],
                    ) => {
                        let slots = slots
                            .iter()
                            .map(|(name, mode)| match mode {
                                Value::String(mode) if mode == "flow" => {
                                    Ok((name.clone(), notist_model::ContentMode::Flow))
                                }
                                Value::String(mode) if mode == "inline" => {
                                    Ok((name.clone(), notist_model::ContentMode::Inline))
                                }
                                _ => Err(format!("content slot `{name}` requires inline or flow")),
                            })
                            .collect::<Result<BTreeMap<_, _>, String>>();
                        let result = slots.and_then(|slots| {
                            self.document_rules.define(
                                element.clone(),
                                notist_model::ElementModel {
                                    inline: *inline,
                                    slots,
                                    block_field: match v.get(3) {
                                        Some(Value::String(field)) => Some(field.clone()),
                                        _ => None,
                                    },
                                },
                            )
                        });
                        match result {
                            Ok(()) => Value::None,
                            Err(e) => Self::failure(e, loc),
                        }
                    }
                    ("with_attributes", [Value::Content(item), Value::Dict(attributes)]) => {
                        let mut item = item.clone();
                        item.apply_attributes(attributes.clone());
                        Value::Content(item)
                    }
                    ("item", [Value::String(name), Value::Dict(args)]) => Value::Content(Item {
                        span: None,
                        origin: None,
                        name: name.clone(),
                        attributes: Env::new(),
                        args: args.clone(),
                        location: loc.clone(),
                    }),
                    ("text", [Value::String(s)]) => {
                        Value::Content(Content::text(s.clone()).at(loc))
                    }
                    ("math" | "raw", [Value::String(s)]) => Value::Content(Item {
                        span: None,
                        origin: None,
                        name: name.clone(),
                        args: Env::from([
                            ("content".into(), Value::String(s.clone())),
                            ("block".into(), Value::Bool(false)),
                        ]),
                        attributes: Env::new(),
                        location: loc.clone(),
                    }),
                    ("link", [Value::String(dest)]) | ("link", [Value::String(dest), _]) => {
                        let body = v
                            .get(1)
                            .cloned()
                            .unwrap_or_else(|| Value::Content(Content::text(dest.clone()).at(loc)));
                        Value::Content(Item {
                            span: None,
                            origin: None,
                            name: "link".into(),
                            args: Env::from([
                                ("dest".into(), Value::String(dest.clone())),
                                ("body".into(), body),
                            ]),
                            attributes: Env::new(),
                            location: loc.clone(),
                        })
                    }
                    ("error", [Value::String(s)]) => Self::failure(s, loc),
                    ("is_error", [v]) => Value::Bool(v.is_error()),
                    ("recover", [v, fallback]) => {
                        if v.is_error() {
                            fallback.clone()
                        } else {
                            v.clone()
                        }
                    }
                    ("str", [Value::String(s)]) => Value::String(s.clone()),
                    ("str", [v @ (Value::Int(_) | Value::Bool(_))]) => {
                        Value::String(v.to_json().to_string())
                    }
                    ("len", [Value::List(v)]) => Value::Int(v.len() as i64),
                    ("len", [Value::Dict(v)]) => Value::Int(v.len() as i64),
                    ("get", [Value::Dict(v), Value::String(k)]) => v
                        .get(k)
                        .cloned()
                        .unwrap_or_else(|| Self::failure("missing dictionary key", loc)),
                    ("get", [Value::List(v), Value::Int(i)]) => usize::try_from(*i)
                        .ok()
                        .and_then(|i| v.get(i))
                        .cloned()
                        .unwrap_or_else(|| Self::failure("list index out of bounds", loc)),
                    ("concat", [Value::List(v)]) => {
                        Value::Content(Self::content(Value::List(v.clone()), loc))
                    }
                    ("map", [f, Value::List(v)]) => Value::List(
                        v.iter()
                            .map(|v| {
                                self.call(
                                    f.clone(),
                                    vec![Argument {
                                        name: None,
                                        trailing: false,
                                        value: v.clone(),
                                    }],
                                    loc,
                                    depth + 1,
                                )
                            })
                            .collect(),
                    ),
                    _ => v.iter().find(|v| v.is_error()).cloned().unwrap_or_else(|| {
                        Self::failure(format!("invalid arguments to `{name}`"), loc)
                    }),
                }
            }
            v if v.is_error() => v,
            _ => Self::failure("value is not callable", loc),
        }
    }
}

// Only newly created values at this call site are stamped; forwarded values retain their origin.
fn stamp_origin(value: &mut Value, location: &Location, node_id: usize, kind: OriginKind) {
    fn item(item: &mut Item, location: &Location, node_id: usize, kind: OriginKind) {
        if item.origin.is_none() && item.location == *location {
            item.origin = Some(CreationOrigin {
                node_id: Some(node_id),
                kind,
            });
        } else if item.location == *location
            && let Some(origin) = &mut item.origin
            && origin.node_id.is_none()
        {
            origin.node_id = Some(node_id);
        }
        for value in item.args.values_mut().chain(item.attributes.values_mut()) {
            stamp_origin(value, location, node_id, kind);
        }
    }
    match value {
        Value::Content(c) => item(c, location, node_id, kind),
        Value::List(v) => {
            for v in v {
                stamp_origin(v, location, node_id, kind);
            }
        }
        Value::Dict(v) => {
            for v in v.values_mut() {
                stamp_origin(v, location, node_id, kind);
            }
        }
        _ => {}
    }
}
