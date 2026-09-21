use crate::package::{module_key, relative};
use crate::{CheckReport, ModuleAddressError, ModuleKey, ResolveError, ResolvedTarget};
#[cfg(feature = "filesystem")]
use crate::{Workspace, uri_path};
use notist_eval::{Evaluation, ModuleProvider, Runtime};
use notist_ir::Env;
use notist_syntax::ParseResult;
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug)]
pub struct SourceSnapshot {
    pub text: Arc<str>,
    pub parsed: Arc<ParseResult>,
}

/// Immutable evaluation inputs. Clones share source text, syntax and binaries.
#[derive(Clone, Default)]
pub struct Snapshot {
    sources: BTreeMap<String, SourceSnapshot>,
    index: BTreeMap<String, String>,
    keys: BTreeMap<String, String>,
    dependencies: BTreeMap<String, BTreeMap<String, String>>,
    binaries: BTreeMap<String, Arc<[u8]>>,
    binary_errors: BTreeMap<String, String>,
    origins: BTreeMap<String, String>,
    errors: Vec<String>,
}

impl Snapshot {
    pub fn sources(&self) -> &BTreeMap<String, SourceSnapshot> {
        &self.sources
    }
    pub fn source(&self, source: &str) -> Option<&SourceSnapshot> {
        self.sources.get(source)
    }
    pub fn origin(&self, source: &str) -> Option<&str> {
        self.origins.get(source).map(String::as_str)
    }
    pub fn evaluate(&self, source: &str) -> Evaluation {
        Runtime::new(self).evaluate(source)
    }

    /// Resolve within a temporary query session; returned handles retain their output.
    pub fn resolve_target(
        &self,
        target: &notist_ir::Target,
    ) -> Result<ResolvedTarget<'_>, ResolveError<'_>> {
        self.query().resolve(target)
    }

    /// Check a source module. Use QuerySession for several queries sharing evaluations.
    pub fn check(&self, source: &str) -> Result<CheckReport<'_>, ModuleAddressError> {
        let key = self
            .module_key(source)
            .ok_or_else(|| ModuleAddressError::UnknownModule(source.into()))?;
        self.query().check(&ModuleKey::from(key))
    }

    fn insert_module(&mut self, source: String, key: String) {
        if let Some(other) = self.index.insert(key.clone(), source.clone()) {
            self.errors.push(format!(
                "module path conflict `{key}`: `{other}` and `{source}`"
            ));
        }
        self.keys.insert(source, key);
    }

    fn complete_namespaces(&mut self) {
        // Namespace-only parents have identity but no source or evaluation body.
        for key in self.index.keys().cloned().collect::<Vec<_>>() {
            let mut parts = key.split("::").collect::<Vec<_>>();
            while parts.len() > 1 {
                parts.pop();
                let parent = parts.join("::");
                if self.index.contains_key(&parent) {
                    continue;
                }
                let directory = parts[1..].join("/");
                let suffix = if directory.is_empty() {
                    "README.notc".into()
                } else {
                    format!("{directory}/README.notc")
                };
                let source = if parts[0] == "root" {
                    suffix
                } else {
                    format!("{}/docs/{suffix}", parts[0])
                };
                self.index.insert(parent.clone(), source.clone());
                self.keys.insert(source, parent);
            }
        }
    }
}

impl ModuleProvider for Snapshot {
    fn parsed(&self, source: &str) -> Option<&ParseResult> {
        self.source(source).map(|source| source.parsed.as_ref())
    }
    fn module_key(&self, source: &str) -> Option<&str> {
        self.keys.get(source).map(String::as_str)
    }
    fn module_source(&self, key: &str) -> Option<&str> {
        self.index.get(key).map(String::as_str)
    }
    fn dependency(&self, package: &str, alias: &str) -> Option<&str> {
        self.dependencies
            .get(package)?
            .get(alias)
            .map(String::as_str)
    }
    fn binary(&self, path: &str) -> Option<&[u8]> {
        self.binaries.get(path).map(AsRef::as_ref)
    }
    fn binary_error(&self, path: &str) -> Option<&str> {
        self.binary_errors.get(path).map(String::as_str)
    }
    fn binary_path(&self, source: &str, path: &str) -> Result<String, String> {
        relative(source, path)
    }
    fn errors(&self) -> &[String] {
        &self.errors
    }
}

/// Mutable host inputs; each snapshot freezes inputs and shares unchanged parses.
#[derive(Default)]
pub struct EvaluationSession {
    pub sources: BTreeMap<String, String>,
    pub dependencies: BTreeMap<String, BTreeMap<String, String>>,
    pub binaries: BTreeMap<String, Vec<u8>>,
    pub binary_errors: BTreeMap<String, String>,
    pub module_attributes: BTreeMap<String, Env>,
    pub used_wasm: Vec<String>,
    pub events: Vec<Value>,
    parsed: BTreeMap<String, SourceSnapshot>,
}

impl EvaluationSession {
    #[cfg(feature = "filesystem")]
    pub(crate) fn parse(&mut self, path: &str, text: &str) -> Arc<ParseResult> {
        if let Some(previous) = self.parsed.get(path)
            && previous.text.as_ref() == text
        {
            return previous.parsed.clone();
        }
        let parsed = Arc::new(notist_syntax::parse_source(path, text));
        self.parsed.insert(
            path.into(),
            SourceSnapshot {
                text: text.into(),
                parsed: parsed.clone(),
            },
        );
        parsed
    }

    pub fn snapshot(&mut self) -> Snapshot {
        self.parsed
            .retain(|source, _| self.sources.contains_key(source));
        for (path, text) in &self.sources {
            if self
                .parsed
                .get(path)
                .is_some_and(|previous| previous.text.as_ref() == text)
            {
                continue;
            }
            self.parsed.insert(
                path.clone(),
                SourceSnapshot {
                    text: text.as_str().into(),
                    parsed: Arc::new(notist_syntax::parse_source(path, text)),
                },
            );
        }
        let mut snapshot = Snapshot {
            sources: self.parsed.clone(),
            binary_errors: self.binary_errors.clone(),
            dependencies: self.dependencies.clone(),
            binaries: self
                .binaries
                .iter()
                .map(|(path, bytes)| (path.clone(), Arc::from(bytes.as_slice())))
                .collect(),
            ..Snapshot::default()
        };
        for path in self.sources.keys() {
            match module_key(path) {
                Ok(key) => snapshot.insert_module(path.clone(), key),
                Err(error) => snapshot.errors.push(error),
            }
        }
        snapshot.complete_namespaces();
        snapshot
    }

    pub fn evaluate(&mut self, source: &str) -> Evaluation {
        self.evaluate_with_env(source).0
    }

    pub fn evaluate_with_env(&mut self, source: &str) -> (Evaluation, Env) {
        let snapshot = self.snapshot();
        let mut runtime = Runtime::new(&snapshot);
        let result = runtime.evaluate_with_env(source);
        self.module_attributes = runtime.module_attributes;
        self.used_wasm = runtime.used_wasm;
        self.events = runtime.events;
        result
    }
}

#[cfg(feature = "filesystem")]
impl Workspace {
    /// Capture editor overlays without reparsing or executing WASM.
    /// Binaries are explicit host inputs; static editor queries never load them.
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot_with_binaries(BTreeMap::new())
    }

    pub fn snapshot_with_binaries(&self, binaries: BTreeMap<String, Arc<[u8]>>) -> Snapshot {
        let ids: BTreeMap<_, _> = self
            .projects
            .packages
            .keys()
            .enumerate()
            .map(|(i, root)| (root, format!("p{i}")))
            .collect();
        let mut snapshot = Snapshot {
            binaries,
            ..Snapshot::default()
        };
        for (root, package) in &self.projects.packages {
            let id = &ids[root];
            snapshot.dependencies.insert(
                id.clone(),
                package
                    .dependencies
                    .iter()
                    .filter_map(|(alias, target)| {
                        ids.get(target).map(|id| (alias.clone(), id.clone()))
                    })
                    .collect(),
            );
        }
        for (uri, (root, key)) in &self.projects.owners {
            let (Some(document), Some(path), Some(id)) =
                (self.documents.get(uri), uri_path(uri), ids.get(root))
            else {
                continue;
            };
            let Ok(local) = path.strip_prefix(root) else {
                continue;
            };
            let Some(local) = local.to_str() else {
                continue;
            };
            let source = format!("{id}/{}", local.replace('\\', "/"));
            snapshot.sources.insert(
                source.clone(),
                SourceSnapshot {
                    text: document.text.as_str().into(),
                    parsed: document.parsed.clone(),
                },
            );
            snapshot.insert_module(source.clone(), format!("{id}{key}"));
            snapshot.origins.insert(source, uri.clone());
        }
        snapshot
            .errors
            .extend(self.projects.errors.values().flatten().cloned());
        snapshot.complete_namespaces();
        snapshot
    }
}
