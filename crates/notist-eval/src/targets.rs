use super::{Content, Env, Location, Runtime, Target, Value};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedItem {
    pub location: Location,
    /// Complete labeled ancestry of this occurrence, ending at the Item itself.
    pub path: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub source: String,
    pub item: Option<ResolvedItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetError {
    MissingModule(String),
    MissingLabel,
    Ambiguous(Vec<ResolvedItem>),
    EvaluationFailed(Vec<String>),
}

impl fmt::Display for TargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingModule(module) => write!(f, "missing target module `{module}`"),
            Self::MissingLabel => f.write_str("LabelPath has no matching Item"),
            Self::Ambiguous(items) => {
                write!(f, "ambiguous LabelPath ({} matching Items)", items.len())?;
                for item in items {
                    let path = item
                        .path
                        .iter()
                        .map(|s| serde_json::to_string(s).unwrap())
                        .collect::<Vec<_>>()
                        .join("::");
                    write!(
                        f,
                        "; {path} at {}:{}",
                        item.location.source, item.location.offset
                    )?;
                }
                Ok(())
            }
            Self::EvaluationFailed(errors) => write!(
                f,
                "target module could not be evaluated: {}",
                errors.join("; ")
            ),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReferenceDiagnostic {
    pub location: Location,
    pub target: Target,
    pub error: TargetError,
}

impl Runtime<'_> {
    /// Return the canonical Target observed at a source expression during evaluation.
    /// One expression can run under different module arguments; such a location has no
    /// single target and must not silently select the first invocation's destination.
    pub fn target_at(&self, source: &str, offset: usize) -> Option<Target> {
        let targets = self.targets_at(source, offset);
        (targets.len() == 1).then(|| targets[0].clone())
    }

    /// Distinct canonical Targets observed at one source expression, in evaluation order.
    pub fn targets_at(&self, source: &str, offset: usize) -> Vec<Target> {
        let mut targets = Vec::new();
        for (target, location) in &self.references {
            if location.source == source && location.offset == offset && !targets.contains(target) {
                targets.push(target.clone());
            }
        }
        targets
    }

    /// Resolve a module path without executing its contents or treating an export as a module.
    pub(super) fn target_module(
        &self,
        path: &[String],
        env: &Env,
        source: &str,
    ) -> Result<String, String> {
        self.path_key(path, env, source)
    }

    pub(super) fn path_key(
        &self,
        path: &[String],
        env: &Env,
        source: &str,
    ) -> Result<String, String> {
        let first = path.first().ok_or("empty module path")?;
        if let Some(value) = env.get(first) {
            let Value::Module(module) = value else {
                return Err(format!("`{first}` is not a module"));
            };
            let key = self
                .world
                .module_key(module)
                .ok_or_else(|| format!("missing module `{module}`"))?;
            return Ok(if path.len() == 1 {
                key.into()
            } else {
                format!("{key}::{}", path[1..].join("::"))
            });
        }
        let current = self
            .world
            .module_key(source)
            .ok_or_else(|| format!("missing module `{source}`"))?;
        let package = current.split("::").next().unwrap();
        let mut parts = current.split("::").map(str::to_owned).collect::<Vec<_>>();
        let mut rest = 1;
        match first.as_str() {
            "vault" => parts.truncate(1),
            "self" => {}
            "super" => {
                let n = path.iter().take_while(|s| s.as_str() == "super").count();
                if n >= parts.len() {
                    return Err("super escapes package root".into());
                }
                parts.truncate(parts.len() - n);
                rest = n;
            }
            name => {
                let dep = self
                    .world
                    .dependency(package, name)
                    .ok_or_else(|| format!("unknown binding or dependency `{name}`"))?;
                parts = vec![dep.to_owned()];
            }
        }
        parts.extend_from_slice(&path[rest..]);
        Ok(parts.join("::"))
    }

    /// `target.module` is a canonical module key. Only Item queries evaluate target contents.
    pub fn resolve_target(&mut self, target: &Target) -> Result<ResolvedTarget, TargetError> {
        let source = self
            .world
            .module_source(&target.module)
            .map(str::to_owned)
            .ok_or_else(|| TargetError::MissingModule(target.module.clone()))?;
        if target.labels.is_empty() {
            return Ok(ResolvedTarget { source, item: None });
        }
        let (_, content) = self.module(&source, 0);
        let mut errors = Vec::new();
        content.warnings(&mut errors);
        if !errors.is_empty() {
            return Err(TargetError::EvaluationFailed(errors));
        }
        let matches: Vec<_> = content
            .label_matches(&target.labels)
            .into_iter()
            .map(|candidate| ResolvedItem {
                location: candidate.item.location.clone(),
                path: candidate.path,
            })
            .collect();
        match matches.len() {
            0 => Err(TargetError::MissingLabel),
            1 => Ok(ResolvedTarget {
                source,
                item: matches.into_iter().next(),
            }),
            _ => Err(TargetError::Ambiguous(matches)),
        }
    }

    /// Check references already encountered during evaluation. Resolving a link does not follow
    /// links in its destination, so mutually linking modules do not become import cycles.
    pub fn reference_diagnostics(&mut self) -> Vec<ReferenceDiagnostic> {
        let references = self.references.clone();
        let mut seen = std::collections::BTreeSet::new();
        references
            .into_iter()
            .filter_map(|(target, location)| {
                if !seen.insert((location.source.clone(), location.offset, target.to_string())) {
                    return None;
                }
                self.resolve_target(&target)
                    .err()
                    .map(|error| ReferenceDiagnostic {
                        location,
                        target,
                        error,
                    })
            })
            .collect()
    }

    pub(super) fn collect_links(&mut self, content: &Content) {
        fn visit_value(runtime: &mut Runtime<'_>, value: &Value) {
            match value {
                Value::Content(content) => runtime.collect_links(content),
                Value::Item(item) => {
                    for v in item.args.values() {
                        visit_value(runtime, v);
                    }
                }
                Value::List(values) => {
                    for v in values {
                        visit_value(runtime, v);
                    }
                }
                Value::Dict(values) => {
                    for v in values.values() {
                        visit_value(runtime, v);
                    }
                }
                _ => {}
            }
        }
        match content {
            Content::Link { target, location } => {
                // Evaluated Target expressions already carry their defining location. A returned
                // Target can become Content at a different call site; do not invent a second
                // reference there. Links returned through the plugin ABI have no such record.
                if !self
                    .references
                    .iter()
                    .any(|(recorded, _)| recorded == target)
                {
                    self.references.push((target.clone(), location.clone()));
                }
            }
            Content::Sequence(parts) => {
                for c in parts {
                    self.collect_links(c);
                }
            }
            Content::Item(item) => {
                for v in item.args.values() {
                    visit_value(self, v);
                }
            }
            _ => {}
        }
    }
}
