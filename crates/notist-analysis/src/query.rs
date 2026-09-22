//! Queries over frozen inputs and immutable, shared evaluation outputs.
//!
//! Handles keep output alive after their query session is dropped. They borrow the
//! input snapshot, so source views cannot accidentally read a newer document.
//! Item equality identifies occurrences in one evaluation, not stable document IDs.
//!
//! ```
//! use notist_analysis::{EvaluationSession, ResolvedTarget, Target};
//! let mut inputs = EvaluationSession::default();
//! inputs.sources.insert("README.not".into(), "= Guide\nBody".into());
//! let snapshot = inputs.snapshot();
//! let item = {
//!     let mut query = snapshot.query();
//!     let target = Target::new("root", vec!["Guide".into()]);
//!     let ResolvedTarget::Item(item) = query.resolve(&target).unwrap() else { panic!() };
//!     item
//! };
//! assert_eq!(item.value().name, "section");
//! assert_eq!(item.origin().syntax.unwrap().text(), "= Guide\nBody");
//! ```
use crate::Snapshot;
use notist_eval::{Evaluation, ModuleProvider, Runtime, TargetObservation};
use notist_ir::{Content, Env, Item, ItemIndex};
use notist_model::Location;
pub use notist_model::{
    Diagnostic, DiagnosticCode, DiagnosticLabel, ModuleAddress, ModuleAddressError, ModuleKey,
    ModuleRoot, OriginKind, SourceSpan, Target,
};
use notist_syntax::{Expr, ExprKind};
use std::{collections::BTreeMap, fmt, rc::Rc};

#[derive(Clone)]
pub struct ModuleRef<'s> {
    snapshot: &'s Snapshot,
    key: ModuleKey,
    source: Option<String>,
}
impl ModuleRef<'_> {
    pub fn key(&self) -> &ModuleKey {
        &self.key
    }
    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }
    pub fn text(&self) -> Option<&str> {
        Some(&self.snapshot.source(self.source()?)?.text)
    }
}
impl fmt::Debug for ModuleRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ModuleRef").field(&self.key).finish()
    }
}
impl PartialEq for ModuleRef<'_> {
    fn eq(&self, rhs: &Self) -> bool {
        std::ptr::eq(self.snapshot, rhs.snapshot) && self.key == rhs.key
    }
}
impl Eq for ModuleRef<'_> {}

#[derive(Clone)]
pub struct SyntaxRef<'s> {
    snapshot: &'s Snapshot,
    source: String,
    expression: &'s Expr,
}
impl<'s> SyntaxRef<'s> {
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn node_id(&self) -> usize {
        self.expression.id
    }
    pub fn expression(&self) -> &'s Expr {
        self.expression
    }
    pub fn span(&self) -> SourceSpan {
        let e = self.expression();
        SourceSpan {
            source: self.source.clone(),
            start: e.offset,
            end: e.end,
        }
    }
    pub fn selection_span(&self) -> SourceSpan {
        let mut span = self.span();
        if let ExprKind::Section(_, title, _) = &self.expression().kind {
            span.end = title.last().map_or(span.start, |e| e.end);
        }
        span
    }
    pub fn text(&self) -> &'s str {
        let e = self.expression();
        &self.snapshot.source(&self.source).unwrap().text[e.offset..e.end]
    }
}
impl fmt::Debug for SyntaxRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SyntaxRef")
            .field(&self.source)
            .field(&self.node_id())
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct ItemOrigin<'s> {
    pub location: Location,
    pub span: Option<SourceSpan>,
    pub kind: Option<OriginKind>,
    pub syntax: Option<SyntaxRef<'s>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvaluationStatus {
    Complete,
    Incomplete,
}
struct Evaluated {
    module: ModuleKey,
    result: Evaluation,
    index: ItemIndex,
}
#[derive(Clone)]
pub struct EvaluationRef<'s> {
    snapshot: &'s Snapshot,
    output: Rc<Evaluated>,
}
impl<'s> EvaluationRef<'s> {
    pub fn module(&self) -> &ModuleKey {
        &self.output.module
    }
    pub fn raw_content(&self) -> &Content {
        &self.output.result.raw_content
    }
    pub fn content(&self) -> &Content {
        &self.output.result.content
    }
    pub fn attributes(&self) -> &Env {
        &self.output.result.attributes
    }
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.output.result.diagnostics
    }
    pub fn status(&self) -> EvaluationStatus {
        if self.diagnostics().is_empty() {
            EvaluationStatus::Complete
        } else {
            EvaluationStatus::Incomplete
        }
    }
    pub fn references(&self) -> &[TargetObservation] {
        &self.output.result.references
    }
    pub fn output_links(&self) -> &[notist_eval::OutputLink] {
        &self.output.result.output_links
    }
    /// Empty means unobserved, not a missing target. Foreign syntax is rejected.
    pub fn observations(
        &self,
        syntax: &SyntaxRef<'_>,
    ) -> Result<Vec<&TargetObservation>, ObservationError> {
        if !std::ptr::eq(self.snapshot, syntax.snapshot) {
            return Err(ObservationError::ForeignSnapshot);
        }
        if !matches!(syntax.expression().kind, ExprKind::Target(..)) {
            return Err(ObservationError::NotTargetExpression);
        }
        Ok(self
            .references()
            .iter()
            .filter(|r| r.location.source == syntax.source && r.node_id == syntax.node_id())
            .collect())
    }
    pub fn root_items(&self) -> impl Iterator<Item = ItemRef<'s>> + '_ {
        self.output.index.roots().iter().map(|&id| self.item(id))
    }
    pub fn items(&self) -> impl Iterator<Item = ItemRef<'s>> + '_ {
        (0..self.output.index.nodes().len()).map(|id| self.item(id))
    }
    fn item(&self, id: usize) -> ItemRef<'s> {
        ItemRef {
            evaluation: self.clone(),
            id,
        }
    }
}
impl fmt::Debug for EvaluationRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("EvaluationRef")
            .field(self.module())
            .field(&self.status())
            .finish()
    }
}
impl PartialEq for EvaluationRef<'_> {
    fn eq(&self, rhs: &Self) -> bool {
        Rc::ptr_eq(&self.output, &rhs.output)
    }
}
impl Eq for EvaluationRef<'_> {}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObservationError {
    ForeignSnapshot,
    NotTargetExpression,
}
impl fmt::Display for ObservationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ForeignSnapshot => "syntax belongs to a different snapshot",
            Self::NotTargetExpression => "syntax is not a Target expression",
        })
    }
}
impl std::error::Error for ObservationError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemRef<'s> {
    evaluation: EvaluationRef<'s>,
    id: usize,
}
impl<'s> ItemRef<'s> {
    pub fn value(&self) -> &Item {
        self.evaluation
            .output
            .index
            .item(self.evaluation.content(), self.id)
    }
    pub fn evaluation(&self) -> EvaluationRef<'s> {
        self.evaluation.clone()
    }
    pub fn parent(&self) -> Option<Self> {
        self.evaluation.output.index.nodes()[self.id]
            .parent
            .map(|id| self.evaluation.item(id))
    }
    pub fn children(&self) -> impl Iterator<Item = Self> + '_ {
        self.evaluation.output.index.nodes()[self.id]
            .children
            .iter()
            .map(|&id| self.evaluation.item(id))
    }
    pub fn descendants(&self) -> impl Iterator<Item = Self> + '_ {
        (self.id + 1..self.evaluation.output.index.nodes()[self.id].subtree_end)
            .map(|id| self.evaluation.item(id))
    }
    pub fn label_path(&self) -> Vec<String> {
        self.evaluation.output.index.label_path(self.id)
    }
    pub fn origin(&self) -> ItemOrigin<'s> {
        let item = self.value();
        ItemOrigin {
            location: item.location.clone(),
            span: item.span.clone(),
            kind: item.origin.as_ref().map(|o| o.kind),
            syntax: item
                .origin
                .as_ref()
                .and_then(|o| o.node_id)
                .and_then(|id| self.evaluation.snapshot.syntax(&item.location.source, id)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvedTarget<'s> {
    Module(ModuleRef<'s>),
    Item(ItemRef<'s>),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError<'s> {
    MissingModule(ModuleKey),
    MissingLabel(Target),
    Ambiguous(Vec<ItemRef<'s>>),
    IncompleteEvaluation(EvaluationRef<'s>),
}
impl ResolveError<'_> {
    pub fn code(&self) -> DiagnosticCode {
        match self {
            Self::MissingModule(_) => DiagnosticCode::MissingModule,
            Self::MissingLabel(_) => DiagnosticCode::MissingLabel,
            Self::Ambiguous(_) => DiagnosticCode::AmbiguousLabel,
            Self::IncompleteEvaluation(_) => DiagnosticCode::IncompleteEvaluation,
        }
    }
}
impl fmt::Display for ResolveError<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingModule(m) => write!(f, "missing target module `{m}`"),
            Self::MissingLabel(_) => f.write_str("LabelPath has no matching Item"),
            Self::Ambiguous(items) => {
                write!(f, "ambiguous LabelPath ({} matching Items)", items.len())
            }
            Self::IncompleteEvaluation(e) => write!(
                f,
                "target module `{}` could not be completely evaluated",
                e.module()
            ),
        }
    }
}
impl std::error::Error for ResolveError<'_> {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReferenceKind {
    Expression { node_id: usize, attempt: usize },
    OutputLink { occurrence: usize },
}
#[derive(Clone, Debug)]
pub struct ReferenceCheck<'s> {
    pub location: Location,
    pub kind: ReferenceKind,
    pub target: Target,
    pub result: Result<ResolvedTarget<'s>, ResolveError<'s>>,
}
#[derive(Clone, Debug)]
pub struct CheckReport<'s> {
    pub evaluation: EvaluationRef<'s>,
    pub references: Vec<ReferenceCheck<'s>>,
}
impl CheckReport<'_> {
    pub fn is_ok(&self) -> bool {
        self.evaluation.status() == EvaluationStatus::Complete
            && self.references.iter().all(|r| r.result.is_ok())
    }
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        let mut out = self.evaluation.diagnostics().to_vec();
        for r in &self.references {
            let Err(error) = &r.result else { continue };
            let mut d =
                Diagnostic::error(error.code(), error.to_string(), Some(r.location.clone()));
            d.notes.push(format!("target: {}", r.target));
            match error {
                ResolveError::Ambiguous(items) => {
                    for (i, item) in items.iter().enumerate() {
                        let origin = item.origin();
                        let span = origin
                            .syntax
                            .map(|s| s.selection_span())
                            .or_else(|| self.evaluation.snapshot.span_at(&origin.location));
                        let message =
                            format!("candidate {}: {}", i + 1, item.label_path().join(" → "));
                        if let Some(span) = span {
                            d.related.push(DiagnosticLabel { span, message });
                        } else {
                            d.notes.push(message);
                        }
                    }
                }
                ResolveError::IncompleteEvaluation(e) => {
                    for cause in e.diagnostics() {
                        if let Some(span) = &cause.span {
                            d.related.push(DiagnosticLabel {
                                span: span.clone(),
                                message: cause.message.clone(),
                            });
                        } else {
                            d.notes.push(cause.message.clone());
                        }
                    }
                }
                _ => {}
            }
            self.evaluation.snapshot.enrich(&mut d);
            if !out.contains(&d) {
                out.push(d);
            }
        }
        out
    }
}

/// Cache complete entry evaluations. Dependency caches and budgets never leak between entries.
pub struct QuerySession<'s> {
    snapshot: &'s Snapshot,
    evaluations: BTreeMap<ModuleKey, EvaluationRef<'s>>,
}
impl<'s> QuerySession<'s> {
    pub fn evaluate(&mut self, key: &ModuleKey) -> Result<EvaluationRef<'s>, ModuleAddressError> {
        if let Some(e) = self.evaluations.get(key) {
            return Ok(e.clone());
        }
        let module = self.snapshot.module(key)?;
        // A namespace has identity but no physical source; its evaluation is empty.
        let source = self.snapshot.module_source(&key.0).unwrap();
        let mut result = Runtime::new(self.snapshot).evaluate(source);
        for diagnostic in &mut result.diagnostics {
            self.snapshot.enrich(diagnostic);
        }
        if module.source().is_none() && self.snapshot.errors().is_empty() {
            debug_assert!(result.diagnostics.is_empty());
        }
        let index = ItemIndex::new(&result.content);
        let evaluation = EvaluationRef {
            snapshot: self.snapshot,
            output: Rc::new(Evaluated {
                module: key.clone(),
                result,
                index,
            }),
        };
        self.evaluations.insert(key.clone(), evaluation.clone());
        Ok(evaluation)
    }
    pub fn resolve(&mut self, target: &Target) -> Result<ResolvedTarget<'s>, ResolveError<'s>> {
        let key = ModuleKey::from(target.module.clone());
        let module = self
            .snapshot
            .module(&key)
            .map_err(|_| ResolveError::MissingModule(key.clone()))?;
        if target.labels.is_empty() && self.snapshot.errors().is_empty() {
            return Ok(ResolvedTarget::Module(module));
        }
        let evaluation = self
            .evaluate(&key)
            .map_err(|_| ResolveError::MissingModule(key))?;
        if evaluation.status() == EvaluationStatus::Incomplete {
            return Err(ResolveError::IncompleteEvaluation(evaluation));
        }
        let matches: Vec<_> = evaluation
            .output
            .index
            .matches(&target.labels)
            .into_iter()
            .map(|id| evaluation.item(id))
            .collect();
        match matches.len() {
            0 => Err(ResolveError::MissingLabel(target.clone())),
            1 => Ok(ResolvedTarget::Item(matches.into_iter().next().unwrap())),
            _ => Err(ResolveError::Ambiguous(matches)),
        }
    }
    pub fn check(&mut self, key: &ModuleKey) -> Result<CheckReport<'s>, ModuleAddressError> {
        let evaluation = self.evaluate(key)?;
        let mut references = vec![];
        for (attempt, r) in evaluation.references().iter().enumerate() {
            if let Ok(target) = &r.result {
                references.push(ReferenceCheck {
                    location: r.location.clone(),
                    kind: ReferenceKind::Expression {
                        node_id: r.node_id,
                        attempt,
                    },
                    target: target.clone(),
                    result: self.resolve(target),
                });
            }
        }
        for (occurrence, r) in evaluation.output_links().iter().enumerate() {
            references.push(ReferenceCheck {
                location: r.location.clone(),
                kind: ReferenceKind::OutputLink { occurrence },
                target: r.target.clone(),
                result: self.resolve(&r.target),
            });
        }
        Ok(CheckReport {
            evaluation,
            references,
        })
    }
}

impl Snapshot {
    pub fn query(&self) -> QuerySession<'_> {
        QuerySession {
            snapshot: self,
            evaluations: BTreeMap::new(),
        }
    }
    pub fn module(&self, key: &ModuleKey) -> Result<ModuleRef<'_>, ModuleAddressError> {
        let source = self
            .module_source(&key.0)
            .ok_or_else(|| ModuleAddressError::UnknownModule(key.0.clone()))?;
        Ok(ModuleRef {
            snapshot: self,
            key: key.clone(),
            source: self.source(source).map(|_| source.to_owned()),
        })
    }
    pub fn locate_module(
        &self,
        from: &ModuleKey,
        address: &ModuleAddress,
    ) -> Result<ModuleRef<'_>, ModuleAddressError> {
        let key = notist_eval::expand_module_address(self, &from.0, address)?;
        self.module(&ModuleKey(key))
    }
    pub fn syntax(&self, source: &str, node_id: usize) -> Option<SyntaxRef<'_>> {
        let expression = self.source(source)?.parsed.expression(node_id)?;
        Some(SyntaxRef {
            snapshot: self,
            source: source.into(),
            expression,
        })
    }
    pub(crate) fn span_at(&self, location: &Location) -> Option<SourceSpan> {
        let source = self.source(&location.source)?;
        if let Some(e) = source
            .parsed
            .expressions()
            .into_iter()
            .filter(|e| e.offset <= location.offset && location.offset < e.end)
            .min_by_key(|e| e.end - e.offset)
        {
            return Some(SourceSpan {
                source: location.source.clone(),
                start: e.offset,
                end: e.end,
            });
        }
        let span = source
            .parsed
            .stmt_ranges
            .iter()
            .find(|(a, b)| *a <= location.offset && location.offset < *b);
        let (start, end) = span.copied().unwrap_or((location.offset, location.offset));
        Some(SourceSpan {
            source: location.source.clone(),
            start,
            end,
        })
    }
    pub(crate) fn enrich(&self, diagnostic: &mut Diagnostic) {
        if diagnostic.span.is_some() {
            return;
        }
        let Some(location) = &diagnostic.location else {
            return;
        };
        if diagnostic.code == DiagnosticCode::Syntax
            && let Some(error) = self
                .source(&location.source)
                .and_then(|s| s.parsed.errors.iter().find(|e| e.start == location.offset))
        {
            diagnostic.span = Some(SourceSpan {
                source: location.source.clone(),
                start: error.start,
                end: error.end,
            });
            return;
        }
        diagnostic.span = self.span_at(location);
    }
}
