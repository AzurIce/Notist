use crate::package::{Manifest, module_key};
use crate::{Workspace, file_uri, range, uri_path};
use notist_syntax::Statement;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

// Canonical manifest directories identify packages throughout a session.
pub type PackageId = PathBuf;

#[derive(Default)]
pub struct Package {
    pub name: String,
    pub dependencies: BTreeMap<String, PackageId>,
    pub modules: BTreeMap<String, String>,
}

#[derive(Default)]
pub struct Projects {
    pub roots: BTreeSet<PathBuf>,
    pub packages: BTreeMap<PackageId, Package>,
    pub owners: BTreeMap<String, (PackageId, String)>,
    pub errors: BTreeMap<String, Vec<String>>,
}

fn walk(dir: &Path, visit: &mut impl FnMut(&Path), stop_at_package: bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if matches!(
                entry.file_name().to_str(),
                Some("target" | ".git" | "node_modules" | ".obsidian" | ".direnv")
            ) {
                continue;
            }
            if stop_at_package && path.join("Notist.toml").is_file() {
                continue;
            }
            walk(&path, visit, stop_at_package);
        } else if kind.is_file() {
            visit(&path);
        }
    }
}

impl Projects {
    fn error(&mut self, root: &Path, message: String) {
        self.errors
            .entry(file_uri(&root.join("Notist.toml")))
            .or_default()
            .push(message);
    }

    fn load(&mut self, root: PathBuf, active: &mut BTreeSet<PathBuf>) {
        if active.contains(&root) {
            self.error(&root, "cyclic package dependency".into());
            return;
        }
        if self.packages.contains_key(&root) {
            return;
        }
        self.packages.insert(root.clone(), Package::default());
        let manifest = std::fs::read_to_string(root.join("Notist.toml"))
            .map_err(|e| e.to_string())
            .and_then(|text| toml::from_str::<Manifest>(&text).map_err(|e| e.to_string()));
        let manifest = match manifest {
            Ok(manifest) => manifest,
            Err(error) => {
                self.error(&root, error);
                return;
            }
        };
        if manifest.package.name.is_empty() {
            self.error(&root, "empty package name".into());
        }
        active.insert(root.clone());
        let mut package = Package {
            name: manifest.package.name.clone(),
            ..Package::default()
        };
        for (alias, dependency) in manifest.dependencies {
            if !notist_syntax::valid_binding(&alias) {
                self.error(&root, format!("invalid dependency alias `{alias}`"));
                continue;
            }
            match root.join(dependency.path).canonicalize() {
                Ok(path) => {
                    if active.len() >= 128 {
                        self.error(&root, "package dependency depth limit exceeded".into());
                        continue;
                    }
                    self.load(path.clone(), active);
                    package.dependencies.insert(alias, path);
                }
                Err(error) => self.error(&root, format!("dependency `{alias}`: {error}")),
            }
        }
        let docs = root.join("docs");
        if !docs.is_dir() {
            self.error(&root, "missing docs directory".into());
        }
        walk(
            &docs,
            &mut |path| {
                if !matches!(
                    path.extension().and_then(|s| s.to_str()),
                    Some("not" | "notc")
                ) {
                    return;
                }
                let Some(local) = path.strip_prefix(&root).ok().and_then(Path::to_str) else {
                    return;
                };
                match module_key(local) {
                    Ok(key) => {
                        let key = key.strip_prefix("root").unwrap().to_owned();
                        let uri = file_uri(path);
                        if let Some(previous) = package.modules.insert(key.clone(), uri.clone()) {
                            self.error(
                                &root,
                                format!("module name collision: {previous} and {uri}"),
                            );
                        }
                        self.owners.insert(uri, (root.clone(), key));
                    }
                    Err(error) => self.error(&root, error),
                }
            },
            true,
        );
        self.packages.insert(root.clone(), package);
        active.remove(&root);
    }
}

impl Workspace {
    // Imports are resolved against the shared source snapshots, never by evaluation.
    pub(crate) fn resolve_path(
        &self,
        uri: &str,
        path: &[String],
        before: usize,
        depth: usize,
    ) -> Option<(String, Option<String>)> {
        if depth > 64 {
            return None;
        }
        let (owner, local) = self.projects.owners.get(uri)?;
        let package = self.projects.packages.get(owner)?;
        let first = path.first()?;
        let mut tail = &path[1..];
        let (target, mut key) = match first.as_str() {
            "vault" => (owner, String::new()),
            "self" => (owner, local.clone()),
            "super" => {
                let mut key = local.clone();
                let count = path.iter().take_while(|s| *s == "super").count();
                for _ in 0..count {
                    key = key.rsplit_once("::")?.0.to_owned();
                }
                tail = &path[count..];
                (owner, key)
            }
            _ => {
                if let Some(doc) = self.documents.get(uri) {
                    for (statement, (start, _)) in
                        doc.parsed.statements.iter().zip(&doc.parsed.stmt_ranges)
                    {
                        if *start >= before {
                            break;
                        }
                        if let Statement::Use(imports) = statement {
                            for import in imports {
                                let alias = import.alias.as_ref().or(import.path.last())?;
                                if !import.glob && alias == first {
                                    let (module, symbol) =
                                        self.resolve_path(uri, &import.path, *start, depth + 1)?;
                                    if tail.is_empty() {
                                        return Some((module, symbol));
                                    }
                                    if symbol.is_some() {
                                        return None;
                                    }
                                    let mut relative = vec!["self".to_owned()];
                                    relative.extend_from_slice(tail);
                                    return self.resolve_path(&module, &relative, 0, depth + 1);
                                }
                                if import.glob {
                                    let Some((module, None)) =
                                        self.resolve_path(uri, &import.path, *start, depth + 1)
                                    else {
                                        continue;
                                    };
                                    if tail.is_empty()
                                        && self
                                            .documents
                                            .get(&module)?
                                            .bindings()
                                            .any(|(name, _, _)| name == first)
                                    {
                                        return Some((module, Some(first.clone())));
                                    }
                                }
                            }
                        }
                    }
                }
                (package.dependencies.get(first)?, String::new())
            }
        };
        for segment in tail {
            key.push_str("::");
            key.push_str(segment);
        }
        let modules = &self.projects.packages.get(target)?.modules;
        if let Some(uri) = modules.get(&key) {
            return Some((uri.clone(), None));
        }
        let (parent, symbol) = key.rsplit_once("::")?;
        let uri = modules.get(parent)?;
        if self
            .documents
            .get(uri)?
            .bindings()
            .any(|(name, _, _)| name == symbol)
        {
            Some((uri.clone(), Some(symbol.into())))
        } else {
            None
        }
    }

    pub(crate) fn import_definition(
        &self,
        uri: &str,
        name: &str,
        point: usize,
        utf8: bool,
    ) -> Value {
        let parts = name.split("::").map(str::to_owned).collect::<Vec<_>>();
        let Some((target, symbol)) = self.resolve_path(uri, &parts, point, 0) else {
            return Value::Null;
        };
        let Some(doc) = self.documents.get(&target) else {
            return Value::Null;
        };
        let (start, end) = match symbol {
            Some(symbol) => match doc.bindings().find(|(name, _, _)| *name == symbol) {
                Some((_, start, end)) => (start, end),
                None => return Value::Null,
            },
            None => (0, 0),
        };
        json!({"uri":target,"range":range(&doc.text,start,end,utf8)})
    }

    pub(crate) fn import_names(&self, uri: &str) -> Vec<String> {
        let mut names = Vec::new();
        if let Some((owner, _)) = self.projects.owners.get(uri) {
            names.extend(self.projects.packages[owner].dependencies.keys().cloned());
        }
        if let Some(doc) = self.documents.get(uri) {
            for (statement, (start, _)) in doc.parsed.statements.iter().zip(&doc.parsed.stmt_ranges)
            {
                if let Statement::Use(imports) = statement {
                    for import in imports {
                        if import.glob {
                            if let Some((target, None)) =
                                self.resolve_path(uri, &import.path, *start, 0)
                                && let Some(doc) = self.documents.get(&target)
                            {
                                names.extend(doc.bindings().map(|(name, _, _)| name.to_owned()));
                            }
                        } else if let Some(name) = import.alias.as_ref().or(import.path.last()) {
                            names.push(name.clone());
                        }
                    }
                }
            }
        }
        names
    }

    pub fn set_roots(&mut self, roots: impl IntoIterator<Item = PathBuf>) {
        self.projects.roots = roots
            .into_iter()
            .filter_map(|p| p.canonicalize().ok())
            .collect();
        self.refresh();
    }

    /// Rebuild the inexpensive package graph; unchanged document ASTs remain shared.
    pub fn refresh(&mut self) {
        let mut projects = Projects {
            roots: self.projects.roots.clone(),
            ..Projects::default()
        };
        let mut manifests = BTreeSet::new();
        for root in &projects.roots {
            if let Some(parent) = root.ancestors().find(|p| p.join("Notist.toml").is_file()) {
                manifests.insert(parent.to_path_buf());
            }
            walk(
                root,
                &mut |path| {
                    if path.file_name().is_some_and(|n| n == "Notist.toml") {
                        manifests.insert(path.parent().unwrap().to_path_buf());
                    }
                },
                false,
            );
        }
        // Open documents outside editor roots can introduce a package too.
        for (uri, doc) in &self.documents {
            if doc.version.is_some()
                && let Some(path) = uri_path(uri)
                && let Some(root) = path
                    .ancestors()
                    .skip(1)
                    .find(|p| p.join("Notist.toml").is_file())
                && let Ok(root) = root.canonicalize()
            {
                manifests.insert(root);
            }
        }
        for root in manifests {
            projects.load(root, &mut BTreeSet::new());
        }
        self.documents
            .retain(|uri, doc| doc.version.is_some() || projects.owners.contains_key(uri));
        for uri in projects.owners.keys() {
            if self
                .documents
                .get(uri)
                .is_some_and(|doc| doc.version.is_some())
            {
                continue;
            }
            if let Some(path) = uri_path(uri)
                && let Ok(text) = std::fs::read_to_string(path)
            {
                self.open(uri.clone(), text, None);
            }
        }
        self.projects = projects;
    }
}
