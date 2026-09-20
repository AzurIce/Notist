use super::{
    Workspace, offset, position, range,
    types::{Analysis, InferredType},
};
use notist_syntax::{ExprKind, Statement, Type};
use serde_json::{Value, json};

impl Workspace {
    pub fn inlay_hints(&self, uri: &str, requested: &Value, utf8: bool) -> Value {
        let Some(doc) = self.documents.get(uri) else {
            return json!([]);
        };
        if !doc.parsed.errors.is_empty() {
            return json!([]);
        }
        let start = offset(&doc.text, &requested["start"], utf8);
        let end = offset(&doc.text, &requested["end"], utf8);
        if start > end {
            return json!([]);
        }
        let info = Analysis::new(self).document(uri);
        let mut hints = Vec::new();
        let mut add = |at: usize, label: String| {
            if start <= at && at < end {
                hints.push(json!({"position":position(&doc.text,at,utf8),"label":label,"kind":1}));
            }
        };
        let expressions = doc.expressions();
        let declarations = doc
            .parsed
            .statements
            .iter()
            .zip(&doc.parsed.stmt_ranges)
            .map(|(s, (start, _))| (s, *start))
            .chain(expressions.iter().filter_map(|e| {
                if let ExprKind::Declaration(s) = &e.kind {
                    Some((s.as_ref(), e.offset))
                } else {
                    None
                }
            }));
        for (s, start) in declarations {
            let Statement::Let(name, value) = s else {
                continue;
            };
            if matches!(value.kind, ExprKind::Typed(..)) {
                continue;
            }
            let Some(ty) = info.bindings.get(&start).and_then(hint_type) else {
                continue;
            };
            if let Some(token) = doc.parsed.tokens.iter().find(|t| {
                t.kind == "name"
                    && t.start >= start
                    && t.end <= value.offset
                    && doc.text[t.start..t.end] == *name
            }) {
                add(token.end, format!(": {ty}"));
            }
        }
        for e in expressions {
            let ExprKind::Lambda(_, body) = &e.kind else {
                continue;
            };
            if matches!(body.kind, ExprKind::Typed(..)) {
                continue;
            }
            let Some(result) = info
                .expressions
                .get(&(e.offset, e.end))
                .and_then(|ty| hint_type(&ty.result()))
            else {
                continue;
            };
            if let Some(arrow) = doc
                .parsed
                .tokens
                .iter()
                .rev()
                .find(|t| t.start >= e.offset && t.end <= body.offset && t.text == "=>")
                && let Some(close) = doc
                    .parsed
                    .tokens
                    .iter()
                    .rev()
                    .find(|t| t.start >= e.offset && t.end <= arrow.start && t.text == ")")
            {
                add(close.end, format!(" -> {result}"));
            }
        }
        hints.sort_by_key(|hint| offset(&doc.text, &hint["position"], utf8));
        json!(hints)
    }

    pub fn diagnostics(&self, uri: &str, utf8: bool) -> Value {
        let Some(doc) = self.documents.get(uri) else {
            return json!([]);
        };
        let mut diagnostics = doc
            .diagnostics(utf8)
            .as_array()
            .cloned()
            .unwrap_or_default();
        if diagnostics.is_empty() {
            diagnostics.extend(Analysis::new(self).document(uri).errors.into_iter().map(|(start,end,message)|
                json!({"range":range(&doc.text,start,end,utf8),"severity":1,"source":"notist","message":message})));
            diagnostics.extend(self.target_diagnostics(uri, utf8));
        }
        json!(diagnostics)
    }
}

fn hint_type(ty: &InferredType) -> Option<String> {
    match ty {
        InferredType::Unknown | InferredType::Known(Type::Any) => None,
        InferredType::Function(..) => Some("Function".into()),
        _ => Some(ty.to_string()),
    }
}
