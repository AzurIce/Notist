use crate::{
    content::{Content, Item, Location},
    syntax::{self, Expr, ExprKind, Param, Statement, Type},
};
use serde_json::{Value as Json, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

pub type Env = BTreeMap<String, Value>;

#[derive(Clone, Debug)]
pub enum Value {
    String(String),
    Int(i64),
    Bool(bool),
    None,
    List(Vec<Value>),
    Dict(Env),
    Content(Content),
    Item(Item),
    Module(String),
    Named(String),
    Closure(Rc<Closure>),
    External(Rc<External>),
}
#[derive(Clone, Debug)]
pub struct Closure {
    params: Vec<Param>,
    body: Expr,
    env: Env,
    source: String,
    name: Option<String>,
}
#[derive(Clone, Debug)]
pub struct External {
    params: Vec<Param>,
    defaults: Env,
    path: String,
    export: String,
    result: Type,
}
#[derive(Clone)]
struct Argument {
    name: Option<String>,
    trailing: bool,
    value: Value,
}

impl Value {
    pub fn ty(&self) -> Type {
        match self {
            Self::String(_) => Type::String,
            Self::Int(_) => Type::Int,
            Self::Bool(_) => Type::Bool,
            Self::None => Type::None,
            Self::List(_) => Type::List,
            Self::Dict(_) => Type::Dict,
            Self::Content(_) => Type::Content,
            Self::Item(_) => Type::Item,
            Self::Module(_) => Type::Module,
            _ => Type::Function,
        }
    }
    pub fn to_json(&self) -> Json {
        match self {
            Self::String(v) => json!(v),
            Self::Int(v) => json!(v),
            Self::Bool(v) => json!(v),
            Self::None => Json::Null,
            Self::List(v) => json!(v.iter().map(Self::to_json).collect::<Vec<_>>()),
            Self::Dict(v) => {
                json!({"dict": v.iter().map(|(k,v)| (k,v.to_json())).collect::<BTreeMap<_,_>>()})
            }
            Self::Content(v) => v.to_json(),
            Self::Item(v) => v.to_json(),
            Self::Module(v) => json!({"module":v}),
            Self::Named(v) => json!({"function":v}),
            _ => json!({"function":"<function>"}),
        }
    }
    pub fn serializable(&self) -> bool {
        match self {
            Self::Module(_) | Self::Named(_) | Self::Closure(_) | Self::External(_) => false,
            Self::List(v) => v.iter().all(Self::serializable),
            Self::Dict(v) => v.values().all(Self::serializable),
            Self::Item(i) | Self::Content(Content::Item(i)) => {
                i.args.values().all(Self::serializable)
            }
            Self::Content(Content::Sequence(v)) => {
                v.iter().all(|c| Self::Content(c.clone()).serializable())
            }
            _ => true,
        }
    }
    fn error(&self) -> bool {
        matches!(self, Self::Content(Content::Error { .. }))
    }
}

pub(crate) fn matches_type(ty: &Type, v: &Value) -> bool {
    match ty {
        Type::Any => true,
        Type::Optional(t) => matches!(v, Value::None) || matches_type(t, v),
        Type::Content => matches!(v, Value::Content(_) | Value::Item(_)),
        _ => *ty == v.ty(),
    }
}
fn type_name(name: &str) -> Result<Type, String> {
    if let Some(s) = name.strip_suffix('?') {
        return Ok(Type::Optional(Box::new(type_name(s)?)));
    }
    Ok(match name {
        "String" => Type::String,
        "Int" => Type::Int,
        "Bool" => Type::Bool,
        "Content" => Type::Content,
        "Item" => Type::Item,
        "List" => Type::List,
        "Dict" => Type::Dict,
        "Function" => Type::Function,
        "Any" => Type::Any,
        "None" => Type::None,
        _ => return Err(format!("unknown registered type `{name}`")),
    })
}

pub struct Evaluation {
    pub content: Content,
    pub warnings: Vec<String>,
}

#[derive(Default)]
pub struct Runtime {
    pub sources: BTreeMap<String, String>,
    pub binaries: BTreeMap<String, Vec<u8>>,
    /// Package id -> (dependency alias -> package id).
    pub dependencies: BTreeMap<String, BTreeMap<String, String>>,
    pub used_wasm: Vec<String>,
    pub events: Vec<Json>,
    modules: BTreeMap<String, Env>,
    loading: BTreeSet<String>,
    index: BTreeMap<String, String>,
    registry: BTreeMap<String, Env>,
    steps: usize,
}

impl Runtime {
    pub fn evaluate(&mut self, source: &str) -> Evaluation {
        self.evaluate_with_env(source).0
    }
    pub fn evaluate_with_env(&mut self, source: &str) -> (Evaluation, Env) {
        self.modules.clear();
        self.loading.clear();
        self.registry.clear();
        self.used_wasm.clear();
        self.events.clear();
        self.steps = 20_000;
        self.index.clear();
        let mut errors = Vec::new();
        for path in self.sources.keys() {
            match crate::package::module_key(path) {
                Ok(key) => {
                    if let Some(other) = self.index.insert(key.clone(), path.clone()) {
                        errors.push(format!(
                            "module path conflict `{key}`: `{other}` and `{path}`"
                        ));
                    }
                }
                Err(e) => errors.push(e),
            }
        }
        let keys = self.index.keys().cloned().collect::<Vec<_>>();
        for key in keys {
            let mut parts = key.split("::").collect::<Vec<_>>();
            while parts.len() > 1 {
                parts.pop();
                let source = if parts[0] == "root" {
                    format!("{}/README.notc", parts[1..].join("/"))
                } else {
                    format!(
                        "{}/docs/{}README.notc",
                        parts[0],
                        if parts.len() > 1 {
                            format!("{}/", parts[1..].join("/"))
                        } else {
                            String::new()
                        }
                    )
                };
                self.index
                    .entry(parts.join("::"))
                    .or_insert(source.trim_start_matches('/').into());
            }
        }
        let location = Location {
            source: source.into(),
            offset: 0,
        };
        let (env, content) = if errors.is_empty() {
            self.module(source, 0)
        } else {
            (
                Env::new(),
                Content::Sequence(
                    errors
                        .into_iter()
                        .map(|message| Content::Error {
                            message,
                            location: location.clone(),
                        })
                        .collect(),
                ),
            )
        };
        let mut warnings = Vec::new();
        content.warnings(&mut warnings);
        (Evaluation { content, warnings }, env)
    }
    fn failure(message: impl Into<String>, location: &Location) -> Value {
        Value::Content(Content::Error {
            message: message.into(),
            location: location.clone(),
        })
    }
    pub fn content(value: Value, location: &Location) -> Content {
        match value {
            Value::None => Content::Sequence(vec![]),
            Value::Content(c) => c,
            Value::Item(i) => Content::Item(i),
            Value::String(s) => Content::Text(s),
            Value::Int(n) => Content::Text(n.to_string()),
            Value::List(v) => {
                Content::Sequence(v.into_iter().map(|v| Self::content(v, location)).collect())
            }
            v => Content::Error {
                message: format!("expected Content, found {:?}", v.ty()),
                location: location.clone(),
            },
        }
    }
    fn module(&mut self, source: &str, depth: usize) -> (Env, Content) {
        let location = Location {
            source: source.into(),
            offset: 0,
        };
        if let Some(env) = self.modules.get(source) {
            return (env.clone(), Content::Sequence(vec![]));
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
        let Some(text) = self.sources.get(source).cloned() else {
            self.loading.remove(source);
            if self.index.values().any(|p| p == source) {
                self.modules.insert(source.into(), Env::new());
                return (Env::new(), Content::Sequence(vec![]));
            }
            return (
                Env::new(),
                Self::content(
                    Self::failure(format!("missing module `{source}`"), &location),
                    &location,
                ),
            );
        };
        if source.ends_with(".not") {
            self.loading.remove(source);
            return (
                Env::new(),
                Self::content(
                    Self::failure(
                        "markup .not modules are indexed but lowering is not implemented; use .notc",
                        &location,
                    ),
                    &location,
                ),
            );
        }
        let statements = syntax::parse(&text);
        let mut env = Env::new();
        let mut exports = Env::new();
        let mut output = Vec::new();
        for statement in statements {
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
                    if let Value::Closure(c) = &mut value {
                        Rc::make_mut(c).name = Some(name.clone());
                    }
                    if value.error() {
                        output.push(Self::content(value.clone(), &location));
                    }
                    env.insert(name.clone(), value.clone());
                    exports.insert(name, value);
                }
                Statement::Expression(expr) => {
                    let value = self.eval(&expr, &env, source, depth + 1);
                    output.push(Self::content(
                        value,
                        &Location {
                            source: source.into(),
                            offset: expr.offset,
                        },
                    ));
                }
                Statement::Error(offset, message) => output.push(Content::Error {
                    message,
                    location: Location {
                        source: source.into(),
                        offset,
                    },
                }),
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
                    let result =
                        relative(source, &path).and_then(|path| self.register(&path, &location));
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
        self.loading.remove(source);
        self.modules.insert(source.into(), exports.clone());
        (exports, Content::Sequence(output))
    }
    fn resolve(
        &mut self,
        path: &[String],
        env: &Env,
        source: &str,
        depth: usize,
    ) -> Result<Value, String> {
        let Some(first) = path.first() else {
            return Err("empty module path".into());
        };
        if let Some(value) = env.get(first) {
            if path.len() == 1 {
                return Ok(value.clone());
            }
            if let Value::Module(module) = value {
                let key = crate::package::module_key(module)?;
                return self.resolve_key(&format!("{key}::{}", path[1..].join("::")), depth);
            }
            return Err(format!("`{first}` is not a module"));
        }
        let current = crate::package::module_key(source)?;
        let package = current.split("::").next().unwrap();
        let mut parts = current.split("::").map(str::to_owned).collect::<Vec<_>>();
        let mut rest = 1;
        match first.as_str() {
            "vault" => parts.truncate(1),
            "self" => {}
            "super" => {
                let mut n = 0;
                while path.get(n).is_some_and(|p| p == "super") {
                    n += 1;
                }
                if n >= parts.len() {
                    return Err("super escapes package root".into());
                }
                parts.truncate(parts.len() - n);
                rest = n;
            }
            name => {
                let dep = self
                    .dependencies
                    .get(package)
                    .and_then(|deps| deps.get(name));
                if let Some(dep) = dep {
                    parts = vec![dep.clone()];
                } else {
                    return Err(format!("unknown binding or dependency `{name}`"));
                }
            }
        }
        parts.extend_from_slice(&path[rest..]);
        self.resolve_key(&parts.join("::"), depth)
    }
    fn resolve_key(&mut self, key: &str, depth: usize) -> Result<Value, String> {
        if let Some(source) = self.index.get(key).cloned() {
            let (_, content) = self.module(&source, depth + 1);
            let mut errors = Vec::new();
            content.warnings(&mut errors);
            if !errors.is_empty() {
                return Err(errors.join("; "));
            }
            return Ok(Value::Module(source));
        }
        let Some((parent, name)) = key.rsplit_once("::") else {
            return Err(format!("missing module `{key}`"));
        };
        let source = self
            .index
            .get(parent)
            .cloned()
            .ok_or_else(|| format!("missing module `{parent}`"))?;
        let (env, content) = self.module(&source, depth + 1);
        let mut errors = Vec::new();
        content.warnings(&mut errors);
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
        let bytes = self
            .binaries
            .get(path)
            .ok_or_else(|| format!("missing WASM `{path}`"))?;
        let registry = crate::wasm::registration(bytes)?;
        let exports = registry
            .get("functions")
            .and_then(Json::as_object)
            .ok_or("registration requires functions object")?;
        let mut env = Env::new();
        for (name, desc) in exports {
            if !syntax::valid_binding(name) {
                return Err(format!("invalid registered name `{name}`"));
            }
            let export = desc
                .get("export")
                .and_then(Json::as_str)
                .ok_or("missing function export")?;
            let result = type_name(desc.get("result").and_then(Json::as_str).unwrap_or("Any"))?;
            let mut params = Vec::new();
            let mut defaults = Env::new();
            for p in desc
                .get("params")
                .and_then(Json::as_array)
                .ok_or("missing params array")?
            {
                let name = p
                    .get("name")
                    .and_then(Json::as_str)
                    .ok_or("missing parameter name")?;
                if !syntax::valid_binding(name) || params.iter().any(|p: &Param| p.name == name) {
                    return Err("invalid or duplicate parameter name".into());
                }
                let ty = type_name(
                    p.get("type")
                        .and_then(Json::as_str)
                        .ok_or("missing parameter type")?,
                )?;
                if let Some(default) = p.get("default") {
                    let value = crate::wasm::decode(default.clone(), location)?;
                    if !matches_type(&ty, &value) {
                        return Err(format!("default type mismatch for `{name}`"));
                    }
                    defaults.insert(name.into(), value);
                }
                params.push(Param {
                    name: name.into(),
                    ty,
                    default: None,
                });
            }
            crate::wasm::validate_export(bytes, export)?;
            env.insert(
                name.clone(),
                Value::External(Rc::new(External {
                    params,
                    defaults,
                    path: path.into(),
                    export: export.into(),
                    result,
                })),
            );
        }
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
        match &expr.kind {
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
                Value::Item(i) => match key.as_str() {
                    "name" => Value::String(i.name),
                    "args" => Value::Dict(i.args),
                    _ => Self::failure("Item has name and args fields", &loc),
                },
                v if v.error() => v,
                _ => Self::failure("field access requires Dict or Item", &loc),
            },
            ExprKind::Content(parts) | ExprKind::Styled(_, parts) => {
                let content = Content::Sequence(
                    parts
                        .iter()
                        .map(|e| {
                            let v = self.eval(e, env, source, depth + 1);
                            Self::content(v, &loc)
                        })
                        .collect(),
                );
                if let ExprKind::Styled(name, _) = &expr.kind {
                    Value::Item(Item {
                        name: (*name).into(),
                        args: BTreeMap::from([("body".into(), Value::Content(content))]),
                        location: loc,
                    })
                } else {
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
                v if v.error() => v,
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
                let value = self.call(function, args, &loc, depth + 1);
                if self.events.len() < 4000 {
                    self.events.push(json!({"kind":"call","source":source,"range":[expr.offset,expr.end],"out":value.to_json()}));
                }
                value
            }
            ExprKind::Binary(op, a, b) => {
                let a = self.eval(a, env, source, depth + 1);
                let b = self.eval(b, env, source, depth + 1);
                if a.error() {
                    return a;
                }
                if b.error() {
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
                        Value::Content(Content::Sequence(vec![
                            Self::content(a, &loc),
                            Self::content(b, &loc),
                        ]))
                    }
                    _ => Self::failure("operator argument type mismatch", &loc),
                }
            }
        }
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
                    } else {
                        defaults
                            .get(&param.name)
                            .cloned()
                            .ok_or_else(|| format!("missing argument `{}`", param.name))?
                    }
                }
            };
            if !value.error() && !matches_type(&param.ty, &value) {
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
            Value::External(f) => match self.bind(
                &f.params,
                args,
                Env::new(),
                &f.defaults,
                &loc.source,
                loc,
                depth,
            ) {
                Ok((_, values)) => self
                    .binaries
                    .get(&f.path)
                    .ok_or_else(|| "missing WASM binary".into())
                    .and_then(|bytes| {
                        crate::wasm::invoke(bytes, &f.export, &values, &f.result, loc)
                    })
                    .unwrap_or_else(|e| Self::failure(e, loc)),
                Err(e) => Self::failure(e, loc),
            },
            Value::Named(name) => {
                if args.iter().any(|a| a.name.is_some()) {
                    return Self::failure("builtin requires positional arguments", loc);
                }
                let v = args.into_iter().map(|a| a.value).collect::<Vec<_>>();
                match (name.as_str(), v.as_slice()) {
                    ("item", [Value::String(name), Value::Dict(args)]) => Value::Item(Item {
                        name: name.clone(),
                        args: args.clone(),
                        location: loc.clone(),
                    }),
                    ("text", [Value::String(s)]) => Value::Content(Content::Text(s.clone())),
                    ("error", [Value::String(s)]) => Self::failure(s, loc),
                    ("is_error", [v]) => Value::Bool(v.error()),
                    ("recover", [v, fallback]) => {
                        if v.error() {
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
                    _ => v.iter().find(|v| v.error()).cloned().unwrap_or_else(|| {
                        Self::failure(format!("invalid arguments to `{name}`"), loc)
                    }),
                }
            }
            v if v.error() => v,
            _ => Self::failure("value is not callable", loc),
        }
    }
}

pub fn relative(source: &str, target: &str) -> Result<String, String> {
    if target.starts_with('/') || target.contains('\\') {
        return Err("expected relative resource path".into());
    }
    let mut parts = source.split('/').collect::<Vec<_>>();
    parts.pop();
    let floor = usize::from(source.contains("/docs/"));
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.len() <= floor {
                    return Err("resource path escapes package".into());
                }
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    Ok(parts.join("/"))
}
