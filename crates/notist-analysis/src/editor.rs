use notist_syntax::{self as syntax, Expr, ExprKind, ParseResult, Statement};
#[path = "documentation.rs"]
mod documentation;
#[path = "hints.rs"]
mod hints;
#[path = "project.rs"]
pub mod project;
#[path = "targets.rs"]
mod targets;
#[path = "types.rs"]
mod types;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

pub fn position(text: &str, offset: usize, utf8: bool) -> Value {
    let mut end = offset.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let prefix = &text[..end];
    let line = prefix.bytes().filter(|b| *b == b'\n').count();
    let tail = prefix.rsplit('\n').next().unwrap_or("");
    json!({"line": line, "character": if utf8 {tail.len()} else {tail.encode_utf16().count()}})
}
pub fn offset(text: &str, pos: &Value, utf8: bool) -> usize {
    let line = pos["line"].as_u64().unwrap_or(0) as usize;
    let col = pos["character"].as_u64().unwrap_or(0) as usize;
    let mut start = 0;
    for _ in 0..line {
        match text[start..].find('\n') {
            Some(n) => start += n + 1,
            None => return text.len(),
        }
    }
    let mut units = 0;
    for (n, c) in text[start..].char_indices() {
        if c == '\n' || units >= col {
            return start + n;
        }
        let width = if utf8 { c.len_utf8() } else { c.len_utf16() };
        if units + width > col {
            return start + n;
        }
        units += width;
    }
    text.len()
}
pub fn range(text: &str, start: usize, end: usize, utf8: bool) -> Value {
    json!({"start":position(text,start,utf8),"end":position(text,end,utf8)})
}
pub fn file_uri(path: &Path) -> String {
    url::Url::from_file_path(path).unwrap().into()
}
pub fn uri_path(uri: &str) -> Option<PathBuf> {
    url::Url::parse(uri).ok()?.to_file_path().ok()
}

pub struct Document {
    pub text: String,
    pub parsed: Arc<ParseResult>,
    pub version: Option<i64>,
    exports: BTreeMap<String, (usize, usize)>,
}
#[derive(Debug, PartialEq)]
enum Definition {
    Local(usize, usize),
    Name(String),
}
impl Document {
    fn expressions(&self) -> Vec<&Expr> {
        fn visit<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
            out.push(e);
            match &e.kind {
                ExprKind::Content(v) | ExprKind::List(v) | ExprKind::Styled(_, v) => {
                    for e in v {
                        visit(e, out);
                    }
                }
                ExprKind::Section(_, title, body) => {
                    for e in title.iter().chain(body) {
                        visit(e, out);
                    }
                }
                ExprKind::Dict(v) | ExprKind::Element(_, v) => {
                    for (_, e) in v {
                        visit(e, out);
                    }
                }
                ExprKind::Annotation(_, e) | ExprKind::Field(e, _) | ExprKind::Typed(_, e) => {
                    visit(e, out)
                }
                ExprKind::Call(e, args) => {
                    visit(e, out);
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
                ExprKind::Binary(_, a, b) => {
                    visit(a, out);
                    visit(b, out);
                }
                ExprKind::If(a, b, c) => {
                    visit(a, out);
                    visit(b, out);
                    visit(c, out);
                }
                ExprKind::Declaration(s) => statement(s, out),
                _ => {}
            }
        }
        fn statement<'a>(s: &'a Statement, out: &mut Vec<&'a Expr>) {
            if let Statement::Let(_, e) | Statement::Expression(e) = s {
                visit(e, out);
            }
        }
        let mut out = Vec::new();
        for s in &self.parsed.statements {
            statement(s, &mut out);
        }
        out
    }
    pub fn new(path: &str, text: String, version: Option<i64>) -> Self {
        let parsed = syntax::parse_source(path, &text);
        let mut exports = BTreeMap::new();
        for (statement, span) in parsed.statements.iter().zip(&parsed.stmt_ranges) {
            if let Statement::Let(name, _) = statement {
                let name_span = parsed
                    .tokens
                    .iter()
                    .find(|t| {
                        t.kind == "name" && t.text == *name && t.start >= span.0 && t.end <= span.1
                    })
                    .map(|t| (t.start, t.end))
                    .unwrap_or(*span);
                exports.entry(name.clone()).or_insert(name_span);
            }
        }
        Self {
            text,
            parsed: Arc::new(parsed),
            version,
            exports,
        }
    }
    pub fn diagnostics(&self, utf8: bool) -> Value {
        json!(self.parsed.errors.iter().map(|e| json!({"range":range(&self.text,e.start,e.end,utf8),"severity":1,"source":"notist","message":e.message})).collect::<Vec<_>>())
    }
    pub fn symbols(&self, utf8: bool) -> Value {
        fn title_text(e: &Expr) -> String {
            match &e.kind {
                ExprKind::String(s) => s.clone(),
                ExprKind::Element(name, _) if name == "space" => " ".into(),
                ExprKind::Content(v) | ExprKind::Styled(_, v) => v.iter().map(title_text).collect(),
                ExprKind::Element(name, fields) if name == "raw" || name == "math" => fields
                    .iter()
                    .find(|(key, _)| key == "content")
                    .map(|(_, e)| title_text(e))
                    .unwrap_or_default(),
                _ => String::new(),
            }
        }
        fn section(e: &Expr, text: &str, utf8: bool, out: &mut Vec<Value>) {
            if let ExprKind::Section(_, title, body) = &e.kind {
                let label = title.iter().map(title_text).collect::<String>();
                let label = if label.trim().is_empty() {
                    "(untitled)".into()
                } else {
                    label
                };
                let mut children = Vec::new();
                for e in body {
                    section(e, text, utf8, &mut children);
                }
                let end = title.last().map_or(e.offset, |e| e.end);
                out.push(json!({"name":label,"kind":3,"range":range(text,e.offset,e.end,utf8),"selectionRange":range(text,e.offset,end,utf8),"children":children}));
            } else {
                match &e.kind {
                    ExprKind::Content(v) | ExprKind::List(v) | ExprKind::Styled(_, v) => {
                        for e in v {
                            section(e, text, utf8, out);
                        }
                    }
                    ExprKind::Element(_, v) | ExprKind::Dict(v) => {
                        for (_, e) in v {
                            section(e, text, utf8, out);
                        }
                    }
                    ExprKind::Call(_, args) => {
                        for arg in args {
                            section(&arg.expr, text, utf8, out);
                        }
                    }
                    ExprKind::Declaration(s) => {
                        if let Statement::Let(name, value) = s.as_ref() {
                            out.push(json!({"name":name,"kind":if matches!(value.kind,ExprKind::Lambda(..)){12}else{13},"range":range(text,e.offset,e.end,utf8),"selectionRange":range(text,e.offset,e.end,utf8)}));
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        for (s, (start, end)) in self.parsed.statements.iter().zip(&self.parsed.stmt_ranges) {
            match s {
                Statement::Let(name, e) => {
                    let selection = self
                        .parsed
                        .tokens
                        .iter()
                        .find(|t| {
                            t.kind == "name"
                                && t.text == *name
                                && t.start >= *start
                                && t.end <= *end
                        })
                        .map(|t| (t.start, t.end))
                        .unwrap_or((*start, *end));
                    out.push(json!({"name":name,"kind":if matches!(e.kind,ExprKind::Lambda(..)){12}else{13},"range":range(&self.text,*start,*end,utf8),"selectionRange":range(&self.text,selection.0,selection.1,utf8)}));
                }
                Statement::Expression(e) => section(e, &self.text, utf8, &mut out),
                _ => {}
            }
        }
        json!(out)
    }
    pub fn bindings(&self) -> impl Iterator<Item = (&str, usize, usize)> {
        self.exports
            .iter()
            .map(|(name, (start, end))| (name.as_str(), *start, *end))
    }
    fn definition_at(&self, point: usize) -> Option<Definition> {
        type Scope = BTreeMap<String, Option<(usize, usize)>>;
        fn sequence(values: &[Expr], point: usize, scope: &Scope) -> Option<Definition> {
            let mut scope = scope.clone();
            for value in values {
                if let Some(found) = visit(value, point, &scope) {
                    return Some(found);
                }
                if let ExprKind::Declaration(statement) = &value.kind
                    && let Statement::Let(name, _) = statement.as_ref()
                {
                    scope.insert(name.clone(), Some((value.offset, value.end)));
                }
                if let ExprKind::Declaration(statement) = &value.kind
                    && let Statement::Use(imports) = statement.as_ref()
                {
                    for import in imports {
                        if let Some(name) = import.alias.as_ref().or(import.path.last()) {
                            scope.insert(name.clone(), None);
                        }
                    }
                }
            }
            None
        }
        fn visit(e: &Expr, point: usize, scope: &Scope) -> Option<Definition> {
            if point < e.offset || point >= e.end {
                return None;
            }
            match &e.kind {
                ExprKind::Name(name) => {
                    let first = name.split("::").next().unwrap();
                    match scope.get(first) {
                        Some(Some((start, end))) if first == name => {
                            Some(Definition::Local(*start, *end))
                        }
                        Some(_) => None,
                        None => Some(Definition::Name(name.clone())),
                    }
                }
                ExprKind::Content(values) | ExprKind::Styled(_, values) => {
                    sequence(values, point, scope)
                }
                ExprKind::Section(_, title, body) => {
                    sequence(title, point, scope).or_else(|| sequence(body, point, scope))
                }
                ExprKind::List(values) => values.iter().find_map(|e| visit(e, point, scope)),
                ExprKind::Dict(values) | ExprKind::Element(_, values) => {
                    values.iter().find_map(|(_, e)| visit(e, point, scope))
                }
                ExprKind::Annotation(_, value) | ExprKind::Typed(_, value) => {
                    visit(value, point, scope)
                }
                ExprKind::Call(callee, args) => visit(callee, point, scope)
                    .or_else(|| args.iter().find_map(|arg| visit(&arg.expr, point, scope))),
                ExprKind::Field(base, _) => visit(base, point, scope),
                ExprKind::Binary(_, left, right) => {
                    visit(left, point, scope).or_else(|| visit(right, point, scope))
                }
                ExprKind::If(condition, yes, no) => visit(condition, point, scope)
                    .or_else(|| visit(yes, point, scope))
                    .or_else(|| visit(no, point, scope)),
                ExprKind::Lambda(params, body) => {
                    let mut local = scope.clone();
                    for param in params {
                        if let Some(default) = &param.default
                            && let Some(found) = visit(default, point, &local)
                        {
                            return Some(found);
                        }
                        // Parameter source ranges are not represented in the AST yet.
                        local.insert(param.name.clone(), None);
                    }
                    visit(body, point, &local)
                }
                ExprKind::Declaration(statement) => match statement.as_ref() {
                    Statement::Let(_, value) | Statement::Expression(value) => {
                        visit(value, point, scope)
                    }
                    _ => None,
                },
                _ => None,
            }
        }
        let mut scope = Scope::new();
        for (statement, span) in self.parsed.statements.iter().zip(&self.parsed.stmt_ranges) {
            match statement {
                Statement::Let(name, value) => {
                    if let Some(found) = visit(value, point, &scope) {
                        return Some(found);
                    }
                    scope.insert(name.clone(), Some(*span));
                }
                Statement::Expression(value) => {
                    if let Some(found) = visit(value, point, &scope) {
                        return Some(found);
                    }
                }
                _ => {}
            }
        }
        None
    }
}

#[derive(Default)]
pub struct Workspace {
    pub documents: BTreeMap<String, Document>,
    pub projects: project::Projects,
}
impl Workspace {
    pub fn open(&mut self, uri: String, text: String, version: Option<i64>) {
        if let Some(doc) = self.documents.get_mut(&uri)
            && doc.text == text
        {
            doc.version = version;
            return;
        }
        self.documents
            .insert(uri.clone(), Document::new(&uri, text, version));
    }
    pub fn load(&mut self, root: &Path) {
        if let Ok(root) = root.canonicalize() {
            self.projects.roots.insert(root);
        }
        self.refresh();
    }
    pub fn word(&self, uri: &str, pos: &Value, utf8: bool) -> Option<String> {
        let doc = self.documents.get(uri)?;
        let n = offset(&doc.text, pos, utf8);
        let valid = |c: char| c.is_alphanumeric() || c == '_';
        let start = doc.text[..n]
            .rfind(|c: char| !valid(c))
            .map_or(0, |n| n + doc.text[n..].chars().next().unwrap().len_utf8());
        let end = doc.text[n..]
            .find(|c: char| !valid(c))
            .map_or(doc.text.len(), |e| n + e);
        Some(doc.text[start..end].to_owned())
    }
    pub fn definition(&self, uri: &str, pos: &Value, utf8: bool) -> Value {
        let Some(doc) = self.documents.get(uri) else {
            return Value::Null;
        };
        let n = offset(&doc.text, pos, utf8);
        if let Some(target) = doc.target_at(n) {
            return self.target_definition(uri, target, utf8);
        }
        if let Some((_, start, end)) = doc
            .bindings()
            .find(|(_, start, end)| *start <= n && n < *end)
        {
            return json!({"uri":uri,"range":range(&doc.text,start,end,utf8)});
        }
        match doc.definition_at(n) {
            Some(Definition::Local(start, end)) => {
                let (start, end) = doc
                    .parsed
                    .tokens
                    .iter()
                    .find(|t| t.kind == "name" && t.start >= start && t.end <= end)
                    .map(|t| (t.start, t.end))
                    .unwrap_or((start, end));
                json!({"uri":uri,"range":range(&doc.text,start,end,utf8)})
            }
            Some(Definition::Name(name)) => self.import_definition(uri, &name, n, utf8),
            None => Value::Null,
        }
    }

    pub fn hover(&self, uri: &str, pos: &Value, utf8: bool) -> Value {
        self.hover_with_format(uri, pos, utf8, false)
    }

    /// Render hover information for the requested LSP content format. Documentation
    /// comments are intentionally read from source text and never evaluated.
    pub fn hover_with_format(&self, uri: &str, pos: &Value, utf8: bool, markdown: bool) -> Value {
        let Some(doc) = self.documents.get(uri) else {
            return Value::Null;
        };
        let n = offset(&doc.text, pos, utf8);
        if !doc
            .parsed
            .tokens
            .iter()
            .any(|t| t.kind == "name" && t.start <= n && n < t.end)
        {
            return Value::Null;
        }
        let Some(name) = self.word(uri, pos, utf8).filter(|name| !name.is_empty()) else {
            return Value::Null;
        };
        let target = self.definition(uri, pos, utf8);
        let mut analysis = types::Analysis::new(self);
        let Some(target_doc) = target["uri"]
            .as_str()
            .and_then(|uri| self.documents.get(uri))
        else {
            let info = analysis.document(uri);
            let Some((span, ty)) = info
                .expressions
                .iter()
                .filter(|((start, end), _)| *start <= n && n < *end)
                .min_by_key(|((start, end), _)| end - start)
            else {
                return Value::Null;
            };
            if *ty == types::InferredType::Unknown {
                return Value::Null;
            }
            let expression = doc
                .expressions()
                .into_iter()
                .find(|e| (e.offset, e.end) == *span);
            let signature = if let Some(Expr {
                kind: ExprKind::Name(name),
                ..
            }) = expression
            {
                format!("{name}: {ty}")
            } else {
                ty.to_string()
            };
            return json!({"contents":{"kind":if markdown {"markdown"} else {"plaintext"},"value":documentation::hover(&signature,"",markdown)},"range":range(&doc.text,span.0,span.1,utf8)});
        };
        let target_uri = target["uri"].as_str().unwrap();
        let at = offset(&target_doc.text, &target["range"]["start"], utf8);
        let module = self
            .projects
            .owners
            .get(target_uri)
            .and_then(|(owner, local)| {
                self.projects
                    .packages
                    .get(owner)
                    .map(|p| format!("{}{local}", p.name))
            })
            .unwrap_or_else(|| name.clone());
        let types = analysis.document(target_uri);
        let binding = types
            .bindings
            .range(..=at)
            .next_back()
            .map(|(_, ty)| ty.clone())
            .unwrap_or_default();
        let info = if target["range"]["start"] == target["range"]["end"] {
            documentation::SymbolInfo {
                signature: format!("module {module}"),
                documentation: target_doc.module_docs(),
            }
        } else if let Some(info) = target_doc.symbol_info(at, &binding) {
            info
        } else {
            return Value::Null;
        };
        let valid = |c: char| c.is_alphanumeric() || c == '_';
        let start = doc.text[..n]
            .rfind(|c: char| !valid(c))
            .map_or(0, |p| p + doc.text[p..].chars().next().unwrap().len_utf8());
        let end = doc.text[n..]
            .find(|c: char| !valid(c))
            .map_or(doc.text.len(), |p| n + p);
        let value = documentation::hover(&info.signature, &info.documentation, markdown);
        json!({"contents":{"kind":if markdown {"markdown"} else {"plaintext"},"value":value},"range":range(&doc.text,start,end,utf8)})
    }

    pub fn completions(&self, uri: &str) -> Value {
        let mut names = BTreeMap::new();
        for name in [
            "item",
            "text",
            "math",
            "raw",
            "link",
            "concat",
            "map",
            "str",
            "get",
            "len",
            "error",
            "recover",
            "with_attributes",
            "define_element",
            "vault",
            "self",
            "super",
        ] {
            names.insert(name.to_owned(), 3);
        }
        if let Some(doc) = self.documents.get(uri) {
            for (name, _, _) in doc.bindings() {
                names.insert(name.into(), 6);
            }
            for name in self.import_names(uri) {
                names.insert(name, 9);
            }
        }
        json!(
            names
                .into_iter()
                .map(|(label, kind)| json!({"label":label,"kind":kind}))
                .collect::<Vec<_>>()
        )
    }

    pub fn completions_at(&self, uri: &str, pos: &Value, utf8: bool) -> Value {
        let Some(doc) = self.documents.get(uri) else {
            return json!([]);
        };
        let at = offset(&doc.text, pos, utf8);
        if doc.parsed.tokens.iter().any(|t| {
            t.start < at && at < t.end && matches!(t.kind, "raw" | "math" | "string" | "comment")
        }) {
            return json!([]);
        }
        let start = doc.text[..at]
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
            .map_or(0, |i| i + doc.text[i..].chars().next().unwrap().len_utf8());
        if uri.ends_with(".not")
            && !doc
                .parsed
                .tokens
                .iter()
                .any(|t| t.start <= start && start < t.end && t.kind == "name")
            && !doc.text[..start].ends_with(['#', '@'])
        {
            return json!([]);
        }
        let chain = &doc.text[start..at];
        let (prefix, edit_start, candidates) =
            if let Some((module, prefix)) = chain.rsplit_once("::") {
                let path = module.split("::").map(str::to_owned).collect::<Vec<_>>();
                let Some((target, None)) = self.resolve_path(uri, &path, at, 0) else {
                    return json!([]);
                };
                let mut names = BTreeMap::new();
                if let Some(module_doc) = self.documents.get(&target) {
                    for (name, _, _) in module_doc.bindings() {
                        names.insert(name.to_owned(), 6);
                    }
                }
                if let Some((owner, local)) = self.projects.owners.get(&target) {
                    let base = format!("{local}::");
                    for key in self.projects.packages[owner].modules.keys() {
                        if let Some(tail) = key.strip_prefix(&base)
                            && let Some(child) = tail.split("::").next()
                        {
                            names.insert(child.into(), 9);
                        }
                    }
                }
                let candidates = names
                    .into_iter()
                    .map(|(label, kind)| json!({"label":label,"kind":kind}))
                    .collect::<Vec<_>>();
                (prefix, at - prefix.len(), candidates)
            } else {
                let candidates = self
                    .completions(uri)
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|item| {
                        let name = item["label"].as_str().unwrap_or("");
                        doc.exports.get(name).is_none_or(|(start, _)| *start < at)
                    })
                    .collect();
                (chain, start, candidates)
            };
        json!(
            candidates
                .into_iter()
                .filter_map(|mut item| {
                    let label = item["label"].as_str()?.to_owned();
                    if !label.starts_with(prefix) {
                        return None;
                    }
                    item["textEdit"] =
                        json!({"range":range(&doc.text,edit_start,at,utf8),"newText":label});
                    item["detail"] = json!(match label.as_str() {
                        "math" | "raw" => format!("{label}(content: String) -> Content"),
                        "text" => "text(content: String) -> Content".into(),
                        "item" => "item(name: String, args: Dict) -> Content".into(),
                        "with_attributes" =>
                            "with_attributes(item: Content, attributes: Dict) -> Content".into(),
                        "define_element" =>
                            "define_element(name: String, inline: Bool, slots: Dict, block_field?: String) -> Unit".into(),
                        _ =>
                            if item["kind"] == 9 {
                                "module or import".into()
                            } else {
                                label
                            },
                    });
                    Some(item)
                })
                .collect::<Vec<_>>()
        )
    }

    pub fn references(&self, uri: &str, pos: &Value, utf8: bool) -> Value {
        self.references_with_declaration(uri, pos, utf8, true)
    }

    pub fn references_with_declaration(
        &self,
        uri: &str,
        pos: &Value,
        utf8: bool,
        include_declaration: bool,
    ) -> Value {
        let target = self.definition(uri, pos, utf8);
        if target.is_null() {
            return json!([]);
        }
        let mut found = BTreeMap::new();
        if include_declaration {
            found.insert(target.to_string(), target.clone());
        }
        for (candidate_uri, doc) in &self.documents {
            for e in doc.expressions() {
                if let ExprKind::Name(name) = &e.kind {
                    let segment = name.rsplit("::").next().unwrap();
                    let start = e.end.saturating_sub(segment.len());
                    if self.definition(candidate_uri, &position(&doc.text, start, utf8), utf8)
                        == target
                    {
                        let location =
                            json!({"uri":candidate_uri,"range":range(&doc.text,start,e.end,utf8)});
                        found.insert(location.to_string(), location);
                    }
                }
            }
        }
        json!(found.into_values().collect::<Vec<_>>())
    }

    pub fn prepare_rename(&self, uri: &str, pos: &Value, utf8: bool) -> Result<Value, String> {
        if self
            .documents
            .get(uri)
            .is_some_and(|doc| doc.target_at(offset(&doc.text, pos, utf8)).is_some())
        {
            return Err("Rename of label paths is not implemented".into());
        }
        let target = self.definition(uri, pos, utf8);
        let target_uri = target["uri"]
            .as_str()
            .ok_or("No renameable binding at this position")?;
        let doc = self
            .documents
            .get(target_uri)
            .ok_or("Missing definition source")?;
        let start = offset(&doc.text, &target["range"]["start"], utf8);
        let end = offset(&doc.text, &target["range"]["end"], utf8);
        let name = &doc.text[start..end];
        if !syntax::valid_binding(name) || !doc.parsed.errors.is_empty() {
            return Err(
                "Rename requires a valid binding and a document without parse errors".into(),
            );
        }
        // Until import bindings have source spans, reject edits which might
        // leave import clauses or aliases referring to the previous name.
        if self.documents.values().any(|d| {
            d.parsed
                .tokens
                .iter()
                .any(|t| t.text == "use" && t.kind == "keyword")
        }) {
            return Err("Rename across import bindings is not implemented yet".into());
        }
        let binding_count = doc
            .parsed
            .tokens
            .windows(2)
            .filter(|pair| pair[0].text == "let" && pair[1].text == name)
            .count();
        if binding_count != 1 {
            return Err(
                "Rename of duplicate or shadowed declarations is not implemented yet".into(),
            );
        }
        for (candidate_uri, candidate) in &self.documents {
            for e in candidate.expressions() {
                if matches!(&e.kind, ExprKind::Name(found) if found.rsplit("::").next() == Some(name))
                    && self
                        .definition(
                            candidate_uri,
                            &position(&candidate.text, e.end - name.len(), utf8),
                            utf8,
                        )
                        .is_null()
                {
                    return Err("Rename requires all potentially affected names to resolve".into());
                }
            }
        }
        if self.documents.values().any(|d| d.expressions().iter().any(|e| matches!(&e.kind, ExprKind::Lambda(params, _) if params.iter().any(|p| p.name == name)))) {
            return Err("Rename involving a shadowing parameter is not implemented yet".into());
        }
        let source = self.documents.get(uri).ok_or("Missing source document")?;
        let at = offset(&source.text, pos, utf8);
        let token = source
            .parsed
            .tokens
            .iter()
            .find(|t| t.kind == "name" && t.start <= at && at < t.end)
            .ok_or("Place the cursor on the binding name")?;
        if token.text != name {
            return Err("Place the cursor on the referenced symbol, not its module path".into());
        }
        Ok(json!({"range":range(&source.text,token.start,token.end,utf8),"placeholder":name}))
    }

    pub fn rename(
        &self,
        uri: &str,
        pos: &Value,
        new_name: &str,
        utf8: bool,
    ) -> Result<Value, String> {
        if !syntax::valid_binding(new_name) {
            return Err("The new name is not a valid identifier".into());
        }
        let prepared = self.prepare_rename(uri, pos, utf8)?;
        let old_name = prepared["placeholder"].as_str().unwrap();
        if old_name == new_name {
            return Ok(json!({"documentChanges":[]}));
        }
        if self.documents.values().any(|d| {
            !d.parsed.errors.is_empty()
                || d.parsed
                    .tokens
                    .iter()
                    .any(|t| t.kind == "name" && t.text == new_name)
        }) {
            return Err(
                "The new name may capture an existing binding, or a document has parse errors"
                    .into(),
            );
        }
        let refs = self.references(uri, pos, utf8);
        let mut changes = BTreeMap::<String, Vec<Value>>::new();
        for location in refs.as_array().unwrap() {
            changes
                .entry(location["uri"].as_str().unwrap().into())
                .or_default()
                .push(json!({"range":location["range"],"newText":new_name}));
        }
        Ok(
            json!({"documentChanges":changes.into_iter().map(|(uri, edits)| {
            let version = self.documents.get(&uri).and_then(|d| d.version);
            json!({"textDocument":{"uri":uri,"version":version},"edits":edits})
        }).collect::<Vec<_>>()}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_positions_round_trip() {
        let text = "中😀x\n你好";
        for n in text.char_indices().map(|(n, _)| n).chain([text.len()]) {
            for utf8 in [true, false] {
                assert_eq!(offset(text, &position(text, n, utf8), utf8), n);
            }
        }
    }
    #[test]
    fn markup_exports_and_sections() {
        let doc = Document::new(
            "a.not",
            "#let x = 3;\n= Title\nBody\n== Child\nText\n".into(),
            None,
        );
        assert!(doc.parsed.errors.is_empty());
        assert_eq!(doc.bindings().next().unwrap().0, "x");
        assert_eq!(doc.symbols(false)[1]["children"][0]["name"], "Child");
    }
    #[test]
    fn definitions_respect_code_context_and_shadowing() {
        let source = "#let x = 1;\nplain x\n#x\n#((x: Int) => x)(2)\n";
        let doc = Document::new("a.not", source.into(), None);
        assert!(doc.parsed.errors.is_empty(), "{:?}", doc.parsed.errors);
        assert_eq!(doc.definition_at(source.find("plain x").unwrap() + 6), None);
        assert!(doc.definition_at(source.find("#x").unwrap() + 1).is_some());
        assert_eq!(doc.definition_at(source.find("=> x").unwrap() + 3), None);
    }

    #[test]
    fn references_rename_and_hover_cover_local_bindings() {
        let mut workspace = Workspace::default();
        let uri = "file:///tmp/lsp-local.notc";
        workspace.open(uri.into(), "let value = 1;\nvalue;\nvalue;".into(), Some(1));
        let position = json!({"line":1,"character":2});
        let references = workspace.references(uri, &position, false);
        assert_eq!(references.as_array().unwrap().len(), 3);
        let edit = workspace.rename(uri, &position, "renamed", false).unwrap();
        let edits = edit["documentChanges"][0]["edits"].as_array().unwrap();
        assert_eq!(edits.len(), 3);
        assert_eq!(edit["documentChanges"][0]["textDocument"]["version"], 1);
        let mut text = workspace.documents[uri].text.clone();
        let mut replacements = edits
            .iter()
            .map(|edit| {
                (
                    offset(&text, &edit["range"]["start"], false),
                    offset(&text, &edit["range"]["end"], false),
                )
            })
            .collect::<Vec<_>>();
        replacements.sort_unstable();
        for (start, end) in replacements.into_iter().rev() {
            text.replace_range(start..end, "renamed");
        }
        assert_eq!(text, "let renamed = 1;\nrenamed;\nrenamed;");
        assert_eq!(
            workspace.definition(uri, &json!({"line":0,"character":5}), false)["range"],
            json!({"start":{"line":0,"character":4},"end":{"line":0,"character":9}})
        );
        assert!(
            workspace.hover(uri, &position, false)["contents"]["value"]
                .as_str()
                .unwrap()
                .contains("value")
        );
        assert!(
            workspace
                .rename(uri, &position, "not-valid-name!", false)
                .is_err()
        );
    }

    #[test]
    fn hover_renders_restricted_documentation_markup() {
        let mut workspace = Workspace::default();
        let uri = "file:///tmp/docs.notc";
        workspace.open(
            uri.into(),
            "//! Module docs.\n/// *Adds* two values.\n/// Use `value` carefully.\nlet add = (left: Int, right: Int) => left + right;\nadd;".into(),
            Some(1),
        );
        let hover =
            workspace.hover_with_format(uri, &json!({"line": 4, "character": 1}), false, true);
        assert_eq!(hover["contents"]["kind"], "markdown");
        let value = hover["contents"]["value"].as_str().unwrap();
        assert!(value.contains("Adds"));
        assert!(value.contains("` value `"));
        assert!(value.contains("**Adds**"));
        assert!(value.contains("left: Int"));
        assert!(!value.contains("//!"));
    }

    #[test]
    fn navigation_ignores_literal_text_and_rejects_unsafe_rename() {
        let uri = "file:///tmp/navigation.not";
        let source =
            "#let value = 1;\nvalue `value` $value$\n#value\n#((value: Int) => value)(1)\n";
        let mut workspace = Workspace::default();
        workspace.open(uri.into(), source.into(), Some(3));
        let point = json!({"line":2,"character":3});
        assert_eq!(
            workspace
                .references_with_declaration(uri, &point, false, false)
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(workspace.rename(uri, &point, "renamed", false).is_err());
        assert!(
            workspace
                .hover(uri, &json!({"line":1,"character":3}), false)
                .is_null()
        );
        workspace.open(
            uri.into(),
            "#let value = 1;\n#let other = 2;\n#value".into(),
            Some(4),
        );
        assert!(workspace.rename(uri, &point, "other", false).is_err());
        workspace.open(
            uri.into(),
            "#let value = (x: Int) => value(x);\n#value".into(),
            Some(5),
        );
        assert!(
            workspace
                .rename(uri, &json!({"line":1,"character":3}), "renamed", false)
                .is_err()
        );
    }

    #[test]
    fn completion_and_outline_respect_markup_context() {
        let uri = "file:///tmp/completion.not";
        let mut workspace = Workspace::default();
        workspace.open(uri.into(),"#let earlier = 1;\n#ea\n#let later = 2;\nplain ea\n`#ea`\n= A *strong* title\n- List\n  == Nested\n  Body".into(),Some(1));
        let completion = workspace.completions_at(uri, &json!({"line":1,"character":3}), false);
        assert_eq!(completion.as_array().unwrap().len(), 1, "{completion}");
        assert_eq!(completion[0]["label"], "earlier");
        assert_eq!(completion[0]["textEdit"]["range"]["start"]["character"], 1);
        for point in [
            json!({"line":3,"character":8}),
            json!({"line":4,"character":4}),
        ] {
            assert_eq!(workspace.completions_at(uri, &point, false), json!([]));
        }
        let symbols = workspace.documents[uri].symbols(false);
        assert_eq!(symbols[2]["name"], "A strong title", "{symbols}");
        assert_eq!(symbols[2]["children"][0]["name"], "Nested", "{symbols}");
    }
}
