use notist_next::syntax::{self, Expr, ExprKind, ParseResult, Statement};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
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
    pub parsed: ParseResult,
    pub version: Option<i64>,
}
impl Document {
    pub fn new(path: &str, text: String, version: Option<i64>) -> Self {
        let parsed = syntax::parse_source(path, &text);
        Self {
            text,
            parsed,
            version,
        }
    }
    pub fn diagnostics(&self, utf8: bool) -> Value {
        json!(self.parsed.errors.iter().map(|e| json!({"range":range(&self.text,e.start,e.end,utf8),"severity":1,"source":"notist","message":e.message})).collect::<Vec<_>>())
    }
    pub fn symbols(&self, utf8: bool) -> Value {
        fn section(e: &Expr, text: &str, utf8: bool, out: &mut Vec<Value>) {
            if let ExprKind::Section(_, title, body) = &e.kind {
                let label = title
                    .iter()
                    .filter_map(|e| {
                        if let ExprKind::String(s) = &e.kind {
                            Some(s.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<String>();
                let mut children = Vec::new();
                for e in body {
                    section(e, text, utf8, &mut children);
                }
                let end = title.last().map_or(e.offset, |e| e.end);
                out.push(json!({"name":label,"kind":3,"range":range(text,e.offset,e.end,utf8),"selectionRange":range(text,e.offset,end,utf8),"children":children}));
            }
        }
        let mut out = Vec::new();
        for (s, (start, end)) in self.parsed.statements.iter().zip(&self.parsed.stmt_ranges) {
            match s {
                Statement::Let(name, e) => out.push(json!({"name":name,"kind":if matches!(e.kind,ExprKind::Lambda(..)){12}else{13},"range":range(&self.text,*start,*end,utf8),"selectionRange":range(&self.text,*start,*end,utf8)})),
                Statement::Expression(e) => section(e,&self.text,utf8,&mut out),
                _=>{}
            }
        }
        json!(out)
    }
    pub fn bindings(&self) -> impl Iterator<Item = (&str, usize, usize)> {
        self.parsed
            .statements
            .iter()
            .zip(&self.parsed.stmt_ranges)
            .filter_map(|(s, (start, end))| match s {
                Statement::Let(name, _) => Some((name.as_str(), *start, *end)),
                _ => None,
            })
    }
    fn definition_at(&self, point: usize) -> Option<(usize, usize)> {
        type Scope = BTreeMap<String, Option<(usize, usize)>>;
        fn sequence(values: &[Expr], point: usize, scope: &Scope) -> Option<(usize, usize)> {
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
            }
            None
        }
        fn visit(e: &Expr, point: usize, scope: &Scope) -> Option<(usize, usize)> {
            if point < e.offset || point >= e.end {
                return None;
            }
            match &e.kind {
                ExprKind::Name(name) => scope.get(name).copied().flatten(),
                ExprKind::Content(values) | ExprKind::Styled(_, values) => {
                    sequence(values, point, scope)
                }
                ExprKind::Section(_, title, body) => {
                    sequence(title, point, scope).or_else(|| sequence(body, point, scope))
                }
                ExprKind::List(values) => values.iter().find_map(|e| visit(e, point, scope)),
                ExprKind::Dict(values) => values.iter().find_map(|(_, e)| visit(e, point, scope)),
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
}
impl Workspace {
    pub fn open(&mut self, uri: String, text: String, version: Option<i64>) {
        self.documents
            .insert(uri.clone(), Document::new(&uri, text, version));
    }
    pub fn load(&mut self, root: &Path) {
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                if !matches!(
                    path.file_name().and_then(|s| s.to_str()),
                    Some("target" | ".git" | "node_modules")
                ) {
                    self.load(&path);
                }
            } else if matches!(
                path.extension().and_then(|s| s.to_str()),
                Some("not" | "notc")
            ) && let Ok(text) = std::fs::read_to_string(&path)
            {
                self.open(file_uri(&path), text, None);
            }
        }
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
        doc.definition_at(n).map_or(
            Value::Null,
            |(start, end)| json!({"uri":uri,"range":range(&doc.text,start,end,utf8)}),
        )
    }
    pub fn completions(&self, uri: &str) -> Value {
        let mut names = BTreeMap::new();
        for name in [
            "item", "text", "concat", "map", "str", "get", "len", "error", "recover", "vault",
            "self", "super",
        ] {
            names.insert(name.to_owned(), 3);
        }
        if let Some(doc) = self.documents.get(uri) {
            for (name, _, _) in doc.bindings() {
                names.insert(name.into(), 6);
            }
        }
        json!(
            names
                .into_iter()
                .map(|(label, kind)| json!({"label":label,"kind":kind}))
                .collect::<Vec<_>>()
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
}
