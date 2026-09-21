//! Editor projections of the evaluated label resolver. Workspace snapshots omit WASM inputs.
use super::{Document, Workspace, range};
use crate::{DiagnosticCode, EvaluationSession, ModuleKey, ResolvedTarget, Snapshot};
use notist_eval::ModuleProvider;
use notist_ir::Target;
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
        let mut query = snapshot.query();
        let Some(key) = snapshot.module_key(&source) else {
            return Value::Null;
        };
        let Ok(evaluation) = query.evaluate(&ModuleKey::from(key)) else {
            return Value::Null;
        };
        let Some(syntax) = snapshot.syntax(&source, expr.id) else {
            return Value::Null;
        };
        let Ok(attempts) = evaluation.observations(&syntax) else {
            return Value::Null;
        };
        if attempts.iter().any(|r| r.result.is_err()) {
            return Value::Null;
        }
        let mut observed = Vec::new();
        for r in attempts {
            if let Ok(target) = &r.result {
                if !observed.contains(target) {
                    observed.push(target.clone());
                }
            }
        }
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
        let Ok(target) = query.resolve(&target) else {
            return Value::Null;
        };
        let (target_source, point, exact_span) = match target {
            ResolvedTarget::Item(item) => {
                let origin = item.origin();
                let span = origin.syntax.map(|s| s.selection_span());
                (origin.location.source, Some(origin.location.offset), span)
            }
            ResolvedTarget::Module(module) => {
                let Some(source) = module.source() else {
                    return Value::Null;
                };
                (source.to_owned(), None, None)
            }
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
        let (start, end) = exact_span
            .map(|s| (s.start, s.end))
            .unwrap_or_else(|| point.map_or((0, 0), |point| document.target_span(point)));
        json!({"uri":target_uri,"range":range(&document.text,start,end,utf8)})
    }

    pub(super) fn target_diagnostics(&self, uri: &str, utf8: bool) -> Vec<Value> {
        let Some((snapshot, source)) = self.target_snapshot(uri) else {
            return vec![];
        };
        let Some(document) = self.documents.get(uri) else {
            return vec![];
        };
        let Ok(report) = snapshot.check(&source) else {
            return vec![];
        };
        report.diagnostics().into_iter().filter_map(|d| {
            if !matches!(d.code, DiagnosticCode::TargetFormation | DiagnosticCode::MissingModule | DiagnosticCode::MissingLabel | DiagnosticCode::AmbiguousLabel) { return None; }
            let location = d.location.as_ref()?;
            if !snapshot.origin(&location.source).map_or(location.source == source, |origin| origin == uri) { return None; }
            let span = d.span?;
            let related: Vec<_> = d.related.iter().filter_map(|r| {
                let target_uri = snapshot.origin(&r.span.source).or_else(|| (r.span.source == source).then_some(uri))?;
                let text = &snapshot.source(&r.span.source)?.text;
                Some(json!({"location":{"uri":target_uri,"range":range(text,r.span.start,r.span.end,utf8)},"message":r.message}))
            }).collect();
            Some(json!({"range":range(&document.text,span.start,span.end,utf8),"severity":1,"source":"notist","code":d.code.as_str(),"message":d.message,"relatedInformation":related}))
        }).collect()
    }
}
