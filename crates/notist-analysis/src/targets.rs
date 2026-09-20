//! Editor projections of the evaluated label resolver. Workspace snapshots omit WASM inputs.
use super::{Document, Workspace, range};
use crate::{EvaluationSession, Snapshot};
use notist_eval::{ModuleProvider, Runtime, TargetError};
use notist_ir::{Content, Location, Target, Value as RuntimeValue};
use notist_syntax::{Expr, ExprKind};
use serde_json::{Value, json};

impl Document {
    pub(super) fn target_at(&self, offset: usize) -> Option<&Expr> {
        self.expressions().into_iter().find(|expr| {
            expr.offset <= offset && offset < expr.end && matches!(expr.kind, ExprKind::Target(..))
        })
    }

    fn target_span(&self, offset: usize) -> (usize, usize) {
        let expression = self
            .expressions()
            .into_iter()
            .filter(|expr| expr.offset == offset)
            .min_by_key(|expr| match expr.kind {
                ExprKind::Section(..) => 0,
                ExprKind::Element(..) | ExprKind::Call(..) => 1,
                _ => 2,
            });
        match expression {
            Some(Expr {
                kind: ExprKind::Section(_, title, _),
                ..
            }) => (offset, title.last().map_or(offset, |expr| expr.end)),
            Some(expr) => (expr.offset, expr.end),
            None => (offset, offset),
        }
    }
}

impl Workspace {
    /// Open documents outside a package still support references to their own output.
    fn target_snapshot(&self, uri: &str) -> Option<(Snapshot, String)> {
        let snapshot = self.snapshot();
        if let Some(source) = snapshot
            .sources()
            .keys()
            .find(|source| snapshot.origin(source) == Some(uri))
            .cloned()
        {
            return Some((snapshot, source));
        }
        let document = self.documents.get(uri)?;
        let source = if uri.ends_with(".not") {
            "README.not"
        } else {
            "README.notc"
        };
        let mut session = EvaluationSession::default();
        session.sources.insert(source.into(), document.text.clone());
        Some((session.snapshot(), source.into()))
    }

    pub(super) fn target_definition(&self, uri: &str, expr: &Expr, utf8: bool) -> Value {
        let Some((snapshot, source)) = self.target_snapshot(uri) else {
            return Value::Null;
        };
        let mut runtime = Runtime::new(&snapshot);
        runtime.evaluate(&source);
        let observed = runtime.targets_at(&source, expr.offset);
        let target = match observed.as_slice() {
            [target] => target.clone(),
            [] => {
                // Uncalled function bodies still admit navigation through reserved
                // roots. Named aliases need an evaluated lexical environment.
                let ExprKind::Target(module, labels) = &expr.kind else {
                    return Value::Null;
                };
                if !module
                    .first()
                    .is_some_and(|part| matches!(part.as_str(), "self" | "super" | "vault"))
                {
                    return Value::Null;
                }
                let key = if self.projects.owners.contains_key(uri) {
                    let Some((target_uri, None)) = self.resolve_path(uri, module, expr.offset, 0)
                    else {
                        return Value::Null;
                    };
                    snapshot
                        .sources()
                        .keys()
                        .find(|source| snapshot.origin(source) == Some(target_uri.as_str()))
                        .and_then(|source| snapshot.module_key(source))
                } else if module.len() == 1 && module[0] != "super" {
                    snapshot.module_key(&source)
                } else {
                    None
                };
                let Some(key) = key else {
                    return Value::Null;
                };
                Target::new(key, labels.clone())
            }
            _ => return Value::Null,
        };
        let Ok(target) = runtime.resolve_target(&target) else {
            return Value::Null;
        };
        let (target_source, point) = match target.item {
            Some(item) => (item.location.source, Some(item.location.offset)),
            None => (target.source, None),
        };
        let Some(target_uri) = snapshot
            .origin(&target_source)
            .or_else(|| (source == target_source).then_some(uri))
        else {
            return Value::Null;
        };
        let Some(document) = self.documents.get(target_uri) else {
            return Value::Null;
        };
        let (start, end) = point.map_or((0, 0), |point| document.target_span(point));
        json!({"uri":target_uri,"range":range(&document.text,start,end,utf8)})
    }

    pub(super) fn target_diagnostics(&self, uri: &str, utf8: bool) -> Vec<Value> {
        let Some(document) = self.documents.get(uri) else {
            return Vec::new();
        };
        if !document
            .expressions()
            .iter()
            .any(|expr| matches!(expr.kind, ExprKind::Target(..)))
        {
            return Vec::new();
        }
        let Some((snapshot, source)) = self.target_snapshot(uri) else {
            return Vec::new();
        };
        let mut runtime = Runtime::new(&snapshot);
        let evaluation = runtime.evaluate(&source);
        let mut errors = Vec::new();
        formation_errors(&evaluation.content, &mut errors);
        errors.retain(|(location, _)| document.target_at(location.offset).is_some());
        errors.extend(
            runtime
                .reference_diagnostics()
                .into_iter()
                .filter_map(|diagnostic| {
                    // Static editor queries do not load plugin binaries. Incomplete evaluation
                    // cannot establish that a label is missing or ambiguous.
                    if matches!(diagnostic.error, TargetError::EvaluationFailed(_)) {
                        return None;
                    }
                    Some((diagnostic.location, diagnostic.error.to_string()))
                }),
        );
        errors.into_iter().filter_map(|(location, message)| {
            if !snapshot.origin(&location.source).map_or(location.source == source, |origin| origin == uri) {
                return None;
            }
            let start = location.offset;
            let end = document.expressions().into_iter()
                .filter(|expr| expr.offset <= start && start < expr.end)
                .min_by_key(|expr| expr.end - expr.offset)
                .map_or(start, |expr| expr.end);
            Some(json!({"range":range(&document.text,start,end,utf8),"severity":1,"source":"notist","message":message}))
        }).collect()
    }
}

fn formation_errors(content: &Content, errors: &mut Vec<(Location, String)>) {
    fn value(node: &RuntimeValue, errors: &mut Vec<(Location, String)>) {
        match node {
            RuntimeValue::Content(content) => formation_errors(content, errors),
            RuntimeValue::Item(item) => {
                for field in item.args.values() {
                    value(field, errors);
                }
            }
            RuntimeValue::List(items) => {
                for item in items {
                    value(item, errors);
                }
            }
            RuntimeValue::Dict(fields) => {
                for field in fields.values() {
                    value(field, errors);
                }
            }
            _ => {}
        }
    }
    match content {
        Content::Error { location, message } => errors.push((location.clone(), message.clone())),
        Content::Sequence(parts) => {
            for part in parts {
                formation_errors(part, errors);
            }
        }
        Content::Item(item) => {
            for field in item.args.values() {
                value(field, errors);
            }
        }
        _ => {}
    }
}
