use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use notist_model::{ModulePath, TextRange};
use notist_syntax::{Parse, parse};

#[derive(Clone, Debug)]
pub struct Module {
    pub logical_path: ModulePath,
    pub source_path: Option<PathBuf>,
    pub parse: Option<Parse>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticKind {
    DuplicateModule,
    InvalidReference,
    UnresolvedModule,
    UnsupportedLabelReference,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub message: String,
    pub source_path: Option<PathBuf>,
    pub range: Option<TextRange>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedReference {
    pub source_module: ModulePath,
    pub source_path: PathBuf,
    pub range: TextRange,
    pub target_module: ModulePath,
}

#[derive(Debug)]
pub struct Workspace {
    root: PathBuf,
    modules: BTreeMap<ModulePath, Module>,
    references: Vec<ResolvedReference>,
    diagnostics: Vec<Diagnostic>,
}

impl Workspace {
    pub fn load(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = dunce::canonicalize(root)?;
        let mut workspace = Self {
            root: root.clone(),
            modules: BTreeMap::new(),
            references: Vec::new(),
            diagnostics: Vec::new(),
        };
        workspace.insert_virtual_module(ModulePath::root());
        workspace.scan_directory(&root, &ModulePath::root())?;
        workspace.analyze_references();
        Ok(workspace)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn modules(&self) -> impl Iterator<Item = &Module> {
        self.modules.values()
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn references(&self) -> &[ResolvedReference] {
        &self.references
    }

    fn scan_directory(&mut self, directory: &Path, module_path: &ModulePath) -> io::Result<bool> {
        let mut entries: Vec<_> = fs::read_dir(directory)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        let mut contains_notist_file = false;

        for entry in entries {
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                let child = module_path.child([entry.file_name().to_string_lossy().into_owned()]);
                if self.scan_directory(&path, &child)? {
                    self.insert_virtual_module(child);
                    contains_notist_file = true;
                }
            } else if file_type.is_file() && is_notist_file(&path) {
                let logical_path = if is_readme(&path) {
                    module_path.clone()
                } else {
                    module_path.child([file_stem(&path)?])
                };
                self.insert_source_module(logical_path, path)?;
                contains_notist_file = true;
            }
        }
        Ok(contains_notist_file)
    }

    fn insert_virtual_module(&mut self, logical_path: ModulePath) {
        self.modules.entry(logical_path.clone()).or_insert(Module {
            logical_path,
            source_path: None,
            parse: None,
        });
    }

    fn insert_source_module(&mut self, logical_path: ModulePath, path: PathBuf) -> io::Result<()> {
        let source = fs::read_to_string(&path)?;
        let parse = parse(&source);
        let module = self.modules.entry(logical_path.clone()).or_insert(Module {
            logical_path: logical_path.clone(),
            source_path: None,
            parse: None,
        });

        if let Some(existing) = &module.source_path {
            self.diagnostics.push(Diagnostic {
                kind: DiagnosticKind::DuplicateModule,
                message: format!(
                    "`{}` and `{}` both map to module `{logical_path}`",
                    existing.display(),
                    path.display()
                ),
                source_path: Some(path),
                range: None,
            });
        } else {
            module.source_path = Some(path);
            module.parse = Some(parse);
        }
        Ok(())
    }

    fn analyze_references(&mut self) {
        let mut diagnostics = Vec::new();
        let mut references = Vec::new();
        for module in self.modules.values() {
            let (Some(source_path), Some(parse)) = (&module.source_path, &module.parse) else {
                continue;
            };

            diagnostics.extend(parse.errors.iter().map(|error| Diagnostic {
                kind: DiagnosticKind::InvalidReference,
                message: error.message.clone(),
                source_path: Some(source_path.clone()),
                range: Some(error.range),
            }));

            for link in &parse.links {
                if link.target.label.is_some() {
                    diagnostics.push(Diagnostic {
                        kind: DiagnosticKind::UnsupportedLabelReference,
                        message: "block label references are reserved but not implemented yet"
                            .into(),
                        source_path: Some(source_path.clone()),
                        range: Some(link.range),
                    });
                    continue;
                }

                let Some(target) = link.target.module.resolve_from(&module.logical_path) else {
                    diagnostics.push(Diagnostic {
                        kind: DiagnosticKind::UnresolvedModule,
                        message: "module path escapes above `vault`".into(),
                        source_path: Some(source_path.clone()),
                        range: Some(link.range),
                    });
                    continue;
                };

                if !self.modules.contains_key(&target) {
                    diagnostics.push(Diagnostic {
                        kind: DiagnosticKind::UnresolvedModule,
                        message: format!("unresolved module `{target}`"),
                        source_path: Some(source_path.clone()),
                        range: Some(link.range),
                    });
                } else {
                    references.push(ResolvedReference {
                        source_module: module.logical_path.clone(),
                        source_path: source_path.clone(),
                        range: link.range,
                        target_module: target,
                    });
                }
            }
        }
        self.references = references;
        self.diagnostics.extend(diagnostics);
    }
}

fn is_notist_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("not"))
}

fn is_readme(path: &Path) -> bool {
    path.file_stem()
        .is_some_and(|stem| stem.eq_ignore_ascii_case("README"))
}

fn file_stem(path: &Path) -> io::Result<String> {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "file has no stem"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn maps_files_and_resolves_relative_absolute_and_parent_paths() {
        let root = TempDir::new().unwrap();
        fs::create_dir(root.path().join("pages")).unwrap();
        fs::write(root.path().join("README.not"), "[[pages]]").unwrap();
        fs::write(
            root.path().join("pages/README.not"),
            "[[intro]] [[vault::pages::intro]]",
        )
        .unwrap();
        fs::write(root.path().join("pages/intro.not"), "[[super]]").unwrap();

        let workspace = Workspace::load(root.path()).unwrap();
        assert!(workspace.diagnostics().is_empty());
        assert_eq!(workspace.modules().count(), 3);
        assert_eq!(workspace.references().len(), 4);
        assert_eq!(
            workspace.references()[0].target_module,
            ModulePath::from_segments(["pages".into()])
        );
    }

    #[test]
    fn reports_file_and_readme_module_collisions() {
        let root = TempDir::new().unwrap();
        fs::create_dir(root.path().join("pages")).unwrap();
        fs::write(root.path().join("pages.not"), "").unwrap();
        fs::write(root.path().join("pages/README.not"), "").unwrap();

        let workspace = Workspace::load(root.path()).unwrap();
        assert_eq!(
            workspace.diagnostics()[0].kind,
            DiagnosticKind::DuplicateModule
        );
    }

    #[test]
    fn reports_missing_modules_and_labels_as_unsupported() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("README.not"), "[[missing]] [[#label]]").unwrap();

        let workspace = Workspace::load(root.path()).unwrap();
        assert_eq!(workspace.diagnostics().len(), 2);
        assert!(
            workspace
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind == DiagnosticKind::UnresolvedModule)
        );
        assert!(
            workspace
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind == DiagnosticKind::UnsupportedLabelReference)
        );
    }

    #[test]
    fn ignores_directories_without_notist_files_in_their_subtree() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("empty/nested")).unwrap();
        fs::write(root.path().join("empty/nested/readme.txt"), "not a module").unwrap();
        fs::create_dir_all(root.path().join("notes/nested")).unwrap();
        fs::write(root.path().join("notes/nested/page.not"), "").unwrap();

        let workspace = Workspace::load(root.path()).unwrap();
        let modules: Vec<_> = workspace
            .modules()
            .map(|module| module.logical_path.to_string())
            .collect();

        assert_eq!(
            modules,
            [
                "vault",
                "vault::notes",
                "vault::notes::nested",
                "vault::notes::nested::page",
            ]
        );
    }
}
