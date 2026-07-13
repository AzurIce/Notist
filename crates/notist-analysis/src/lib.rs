use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use notist_model::{ModulePath, TextRange};
use notist_syntax::{Parse, parse};

#[derive(Clone, Debug)]
pub struct Module {
    /// The stable logical path used by Notist references.
    pub logical_path: ModulePath,
    /// The backing source file, or `None` for a virtual directory module.
    pub source_path: Option<PathBuf>,
    /// The immutable source text corresponding exactly to `parse`.
    pub source: Option<Arc<str>>,
    /// The parsed source, or `None` for a virtual directory module.
    pub parse: Option<Parse>,
}

/// Unsaved source texts keyed by their absolute source path.
pub type SourceOverlays = BTreeMap<PathBuf, Arc<str>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticKind {
    DuplicateModule,
    InvalidSyntax,
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
        Self::load_with_overlays(root, SourceOverlays::new())
    }

    /// Loads a workspace while preferring unsaved source overlays over disk contents.
    pub fn load_with_overlays(
        root: impl AsRef<Path>,
        overlays: SourceOverlays,
    ) -> io::Result<Self> {
        let root = dunce::canonicalize(root)?;
        let overlays = normalize_overlays(&root, overlays)?;
        let mut workspace = Self {
            root: root.clone(),
            modules: BTreeMap::new(),
            references: Vec::new(),
            diagnostics: Vec::new(),
        };
        workspace.insert_virtual_module(ModulePath::root());
        workspace.scan_directory(&root, &ModulePath::root(), &overlays)?;
        workspace.insert_overlay_only_modules(&overlays)?;
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

    /// Returns a module by its logical path.
    pub fn module(&self, path: &ModulePath) -> Option<&Module> {
        self.modules.get(path)
    }

    /// Returns the source-backed module associated with a filesystem path.
    pub fn module_for_source(&self, path: &Path) -> Option<&Module> {
        self.modules
            .values()
            .find(|module| module.source_path.as_deref() == Some(path))
    }

    /// Returns the resolved reference covering the given source byte offset.
    pub fn reference_at(&self, path: &Path, offset: usize) -> Option<&ResolvedReference> {
        self.references.iter().find(|reference| {
            reference.source_path == path
                && reference.range.start <= offset
                && offset < reference.range.end
        })
    }

    fn scan_directory(
        &mut self,
        directory: &Path,
        module_path: &ModulePath,
        overlays: &SourceOverlays,
    ) -> io::Result<bool> {
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
                if self.scan_directory(&path, &child, overlays)? {
                    self.insert_virtual_module(child);
                    contains_notist_file = true;
                }
            } else if file_type.is_file() && is_notist_file(&path) {
                let logical_path = if is_readme(&path) {
                    module_path.clone()
                } else {
                    module_path.child([file_stem(&path)?])
                };
                let source = overlays
                    .get(&path)
                    .cloned()
                    .map(Ok)
                    .unwrap_or_else(|| fs::read_to_string(&path).map(Arc::from))?;
                self.insert_source_module(logical_path, path, source);
                contains_notist_file = true;
            }
        }
        Ok(contains_notist_file)
    }

    fn insert_virtual_module(&mut self, logical_path: ModulePath) {
        self.modules.entry(logical_path.clone()).or_insert(Module {
            logical_path,
            source_path: None,
            source: None,
            parse: None,
        });
    }

    fn insert_source_module(&mut self, logical_path: ModulePath, path: PathBuf, source: Arc<str>) {
        let parse = parse(&source);
        let module = self.modules.entry(logical_path.clone()).or_insert(Module {
            logical_path: logical_path.clone(),
            source_path: None,
            source: None,
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
            module.source = Some(source);
            module.parse = Some(parse);
        }
    }

    fn insert_overlay_only_modules(&mut self, overlays: &SourceOverlays) -> io::Result<()> {
        for (path, source) in overlays {
            if !is_notist_file(path)
                || !path.starts_with(&self.root)
                || self.module_for_source(path).is_some()
            {
                continue;
            }

            let logical_path = module_path_for_source(&self.root, path)?;
            let parent_segment_count = if is_readme(path) {
                logical_path.segments().len()
            } else {
                logical_path.segments().len().saturating_sub(1)
            };
            for count in 1..=parent_segment_count {
                self.insert_virtual_module(ModulePath::from_segments(
                    logical_path.segments()[..count].iter().cloned(),
                ));
            }
            self.insert_source_module(logical_path, path.clone(), source.clone());
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
                kind: DiagnosticKind::InvalidSyntax,
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

fn normalize_overlays(root: &Path, overlays: SourceOverlays) -> io::Result<SourceOverlays> {
    overlays
        .into_iter()
        .map(|(path, source)| normalize_source_path(root, &path).map(|path| (path, source)))
        .collect()
}

fn normalize_source_path(root: &Path, path: &Path) -> io::Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    if path.exists() {
        return dunce::canonicalize(path);
    }

    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "source path has no parent"))?;
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "source path has no file name")
    })?;
    Ok(dunce::canonicalize(parent)?.join(file_name))
}

fn module_path_for_source(root: &Path, path: &Path) -> io::Result<ModulePath> {
    let relative = path.strip_prefix(root).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("source path `{}` is outside the workspace", path.display()),
        )
    })?;
    let mut segments: Vec<String> = relative
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    if !is_readme(path) {
        segments.push(file_stem(path)?);
    }
    Ok(ModulePath::from_segments(segments))
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

    #[test]
    fn overlays_replace_disk_sources_without_writing_files() {
        let root = TempDir::new().unwrap();
        let source_path = root.path().join("README.not");
        fs::write(&source_path, "[[missing]]").unwrap();
        let source_path = dunce::canonicalize(source_path).unwrap();
        let mut overlays = SourceOverlays::new();
        overlays.insert(source_path.clone(), Arc::from("[[child]]"));
        fs::write(root.path().join("child.not"), "child").unwrap();

        let workspace = Workspace::load_with_overlays(root.path(), overlays).unwrap();
        let module = workspace.module_for_source(&source_path).unwrap();

        assert_eq!(module.source.as_deref(), Some("[[child]]"));
        assert!(workspace.diagnostics().is_empty());
        assert_eq!(fs::read_to_string(source_path).unwrap(), "[[missing]]");
    }

    #[test]
    fn overlays_add_unsaved_files_to_the_module_graph() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("README.not"), "[[draft]]").unwrap();
        let draft_path = root.path().join("draft.not");
        let mut overlays = SourceOverlays::new();
        overlays.insert(draft_path.clone(), Arc::from("unsaved"));

        let workspace = Workspace::load_with_overlays(root.path(), overlays).unwrap();

        assert!(workspace.diagnostics().is_empty());
        assert!(
            workspace
                .module(&ModulePath::from_segments(["draft".into()]))
                .is_some()
        );
        assert_eq!(
            workspace
                .module(&ModulePath::from_segments(["draft".into()]))
                .unwrap()
                .source
                .as_deref(),
            Some("unsaved")
        );
    }
}
