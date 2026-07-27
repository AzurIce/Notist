use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use notist_eval::{
    EvalDiagnostic, Evaluator, Function, FunctionContext, FunctionInput, FunctionOutput,
    FunctionRegistry, Value, structure,
};
use notist_model::{
    Block, Content, DefaultValue, Element, FunctionSignature, ModulePath, StructuredDocument,
    TextRange, Type, WikiReference,
};
use notist_syntax::{Call, Expression, ExpressionKind, Parse, parse, parse_wiki_reference};

mod check;

pub use check::{
    CheckDiagnostic, LocalSymbolId, ModuleSemanticIndex, SignatureSet, SymbolDefinition,
    SymbolKind, SymbolReference, check_module, resolve_module_symbols,
};

/// The marker file whose containing directory is a Notist vault root.
pub const MANIFEST_FILE: &str = "Notist.toml";

static NEXT_VIEW_ID: AtomicU64 = AtomicU64::new(1);

/// Stable source identity allocated by one [`VaultEngine`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FileId(u64);

impl FileId {
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Stable logical module identity allocated by one [`VaultEngine`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ModuleId(u64);

impl ModuleId {
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Monotonic publication identity local to one analyzer view.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Revision(u64);

impl Revision {
    pub const INITIAL: Self = Self(0);

    pub const fn raw(self) -> u64 {
        self.0
    }

    fn next(self) -> io::Result<Self> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or_else(|| io::Error::other("workspace revision overflow"))
    }
}

/// Identity of one Analyzer View within the current process instance.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ViewId(u64);

impl ViewId {
    fn allocate() -> Self {
        Self(NEXT_VIEW_ID.fetch_add(1, Ordering::Relaxed))
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Whether a captured source came from disk or a client overlay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceOrigin {
    Disk,
    Overlay,
}

/// Line starts and UTF-8/UTF-16 conversion for one immutable source.
#[derive(Clone, Debug)]
pub struct LineIndex {
    line_starts: Arc<[usize]>,
}

impl LineIndex {
    pub fn new(source: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(
            source
                .bytes()
                .enumerate()
                .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
        );
        Self {
            line_starts: starts.into(),
        }
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn utf16_position(&self, source: &str, offset: usize) -> Option<(u32, u32)> {
        if offset > source.len() || !source.is_char_boundary(offset) {
            return None;
        }
        let line = self.line_starts.partition_point(|start| *start <= offset) - 1;
        let start = self.line_starts[line];
        let column = source[start..offset].encode_utf16().count();
        Some((u32::try_from(line).ok()?, u32::try_from(column).ok()?))
    }

    pub fn offset_utf16(&self, source: &str, line: u32, column: u32) -> Option<usize> {
        let start = *self.line_starts.get(usize::try_from(line).ok()?)?;
        let end = self
            .line_starts
            .get(usize::try_from(line).ok()?.saturating_add(1))
            .copied()
            .unwrap_or(source.len());
        let line_text = &source[start..end];
        let mut units = 0u32;
        for (relative, character) in line_text.char_indices() {
            if units == column {
                return Some(start + relative);
            }
            units = units.checked_add(character.len_utf16() as u32)?;
            if units > column {
                return None;
            }
        }
        (units == column).then_some(end)
    }
}

/// One source captured before analysis begins.
#[derive(Clone, Debug)]
pub struct SourceInput {
    pub file_id: FileId,
    pub canonical_path: PathBuf,
    pub text: Arc<str>,
    pub origin: SourceOrigin,
    pub document_version: Option<i64>,
    pub parse: Parse,
    pub line_index: LineIndex,
}

/// Stable identity of an author-declared annotation.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AnnotationId {
    pub module_id: ModuleId,
    pub name: String,
}

/// Identity of a node that is valid only inside one snapshot revision.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SnapshotNodeId {
    pub revision: Revision,
    pub file_id: FileId,
    pub local_id: u32,
}

/// Identity of the function schema environment captured by a snapshot.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FunctionEnvironmentId(u64);

impl FunctionEnvironmentId {
    pub const BUILTINS: Self = Self(0);

    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// View-local configuration and statically visible function schemas.
#[derive(Clone, Debug)]
pub struct AnalyzerConfiguration {
    pub manifest_override: Option<Arc<str>>,
    pub signatures: SignatureSet,
}

struct SchemaFunction {
    name: String,
    signature: FunctionSignature,
}

impl Function for SchemaFunction {
    fn name(&self) -> &str {
        &self.name
    }

    fn signature(&self) -> FunctionSignature {
        self.signature.clone()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        _input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        Ok(FunctionOutput::value(placeholder_value(
            &self.signature.result,
            &self.name,
        )))
    }
}

fn placeholder_value(ty: &Type, name: &str) -> Value {
    match ty {
        Type::None => Value::None,
        Type::Bool => Value::Bool(false),
        Type::Int => Value::Int(0),
        Type::Float => Value::Float(0.0),
        Type::String => Value::String(String::new()),
        Type::Content => Value::Content(Content::default()),
        Type::Array(_) => Value::Array(Vec::new()),
        Type::Dict(_, _) => Value::Dict(Vec::new()),
        Type::Function => Value::Function(name.to_owned()),
        Type::Optional(_) => Value::None,
        Type::Union(members) => members
            .first()
            .map_or(Value::None, |member| placeholder_value(member, name)),
    }
}

fn safe_evaluation_diagnostics(source: &str, signatures: &SignatureSet) -> Vec<EvalDiagnostic> {
    let mut registry = FunctionRegistry::with_builtins();
    for (name, signature) in signatures.iter() {
        if registry.get(name).is_none() {
            let _ = registry.register(SchemaFunction {
                name: name.to_owned(),
                signature: signature.clone(),
            });
        }
    }
    Evaluator::new(registry)
        .evaluate(source)
        .diagnostics
        .into_iter()
        .filter(|diagnostic| !diagnostic.message.starts_with("unknown function `"))
        .collect()
}

impl Default for AnalyzerConfiguration {
    fn default() -> Self {
        Self {
            manifest_override: None,
            signatures: SignatureSet::with_builtins(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Module {
    /// Stable identity within the owning VaultEngine.
    pub id: ModuleId,
    /// The stable logical path used by Notist references.
    pub logical_path: ModulePath,
    /// Stable source identity, or `None` for a virtual module.
    pub file_id: Option<FileId>,
    /// The backing source file, or `None` for a virtual directory module.
    pub source_path: Option<PathBuf>,
    /// The immutable source text corresponding exactly to `parse`.
    pub source: Option<Arc<str>>,
    /// The parsed source, or `None` for a virtual directory module.
    pub parse: Option<Parse>,
}

/// Unsaved source texts keyed by their absolute source path.
pub type SourceOverlays = BTreeMap<PathBuf, Arc<str>>;

/// Optional editor document versions keyed by canonical source path.
pub type DocumentVersions = BTreeMap<PathBuf, i64>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticKind {
    DuplicateModule,
    DuplicateLabel,
    InvalidSyntax,
    UnresolvedModule,
    UnresolvedLabel,
    UnknownFunction,
    DuplicateFunction,
    UnresolvedName,
    InvalidFunction,
    InvalidArguments,
    TypeMismatch,
    Evaluation,
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
    pub source_file_id: FileId,
    pub source_module_id: ModuleId,
    pub source_module: ModulePath,
    pub source_path: PathBuf,
    pub range: TextRange,
    pub target_module_id: ModuleId,
    pub target_module: ModulePath,
    pub target_label: Option<String>,
    pub target_range: Option<TextRange>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LabelDefinition {
    pub id: AnnotationId,
    pub file_id: FileId,
    pub module: ModulePath,
    pub source_path: PathBuf,
    pub name: String,
    pub range: TextRange,
    pub scope_range: TextRange,
}

/// Protocol-independent definition result tied to one snapshot revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionTarget {
    pub revision: Revision,
    pub module_id: ModuleId,
    pub file_id: Option<FileId>,
    pub range: Option<TextRange>,
    pub annotation: Option<AnnotationId>,
}

/// Semantic reference target resolved at one snapshot position.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceTarget {
    pub revision: Revision,
    pub module_id: ModuleId,
    pub annotation: Option<AnnotationId>,
}

/// Stable semantic identity used by navigation queries within a snapshot.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum SymbolId {
    Module(ModuleId),
    Annotation(AnnotationId),
    Local {
        module_id: ModuleId,
        symbol: LocalSymbolId,
    },
}

/// One definition or use site returned by a symbol navigation query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolLocation {
    pub revision: Revision,
    pub symbol: SymbolId,
    pub file_id: FileId,
    pub range: TextRange,
    pub is_definition: bool,
}

/// A heading returned by the protocol-independent outline query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentSymbol {
    pub id: SnapshotNodeId,
    pub name: String,
    pub level: u8,
    pub range: TextRange,
}

/// One module exposed to workspace-symbol consumers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModuleSymbol {
    pub revision: Revision,
    pub module_id: ModuleId,
    pub file_id: FileId,
    pub name: String,
    pub kind: WorkspaceSymbolKind,
    pub range: TextRange,
    pub annotation: Option<AnnotationId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceSymbolKind {
    Module,
    Annotation,
}

/// A borrowed query result carrying the snapshot revision it belongs to.
#[derive(Clone, Copy, Debug)]
pub struct SnapshotResult<'a, T> {
    pub revision: Revision,
    pub value: &'a T,
}

/// Protocol-independent hover content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HoverInfo {
    pub revision: Revision,
    pub file_id: FileId,
    pub range: TextRange,
    pub contents: String,
}

/// One context-sensitive completion candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionCandidate {
    pub revision: Revision,
    pub kind: CompletionKind,
    pub label: String,
    pub detail: String,
    pub documentation: Option<String>,
    pub replacement: TextRange,
    pub insert_text: String,
    pub module_id: Option<ModuleId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionKind {
    Module,
    Function,
    Parameter,
    Attribute,
}

/// A source excerpt returned by snapshot search.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchContext {
    pub revision: Revision,
    pub file_id: FileId,
    pub module_id: ModuleId,
    pub range: TextRange,
    pub snippet: String,
}

/// Deterministic lowering and structuring result for one snapshot module.
#[derive(Clone, Debug)]
pub struct StructuredModule {
    pub revision: Revision,
    pub module_id: ModuleId,
    pub function_environment: FunctionEnvironmentId,
    pub document: StructuredDocument,
    pub diagnostics: Vec<EvalDiagnostic>,
}

/// Semantic changes derived from two complete snapshots.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceDelta {
    pub from_revision: Revision,
    pub to_revision: Revision,
    pub added_files: Vec<FileId>,
    pub changed_files: Vec<FileId>,
    pub removed_files: Vec<FileId>,
    pub added_modules: Vec<ModuleId>,
    pub changed_modules: Vec<ModuleId>,
    pub removed_modules: Vec<ModuleId>,
    pub changed_references: Vec<FileId>,
    pub changed_diagnostics: Vec<FileId>,
}

/// One atomically published snapshot and its derivable invalidation set.
#[derive(Clone, Debug)]
pub struct SnapshotPublication {
    pub snapshot: Arc<WorkspaceSnapshot>,
    pub delta: WorkspaceDelta,
}

#[derive(Clone, Debug)]
pub struct WorkspaceSnapshot {
    root: PathBuf,
    configuration: Option<Arc<str>>,
    sources: BTreeMap<FileId, SourceInput>,
    source_ids: BTreeMap<PathBuf, FileId>,
    modules: BTreeMap<ModulePath, Module>,
    module_paths: BTreeMap<ModuleId, ModulePath>,
    labels: Vec<LabelDefinition>,
    references: Vec<ResolvedReference>,
    diagnostics: Vec<Diagnostic>,
    signatures: SignatureSet,
    module_signatures: BTreeMap<ModuleId, SignatureSet>,
    module_semantics: BTreeMap<ModuleId, ModuleSemanticIndex>,
    attribute_keys: BTreeSet<String>,
    function_environment: FunctionEnvironmentId,
    view_id: ViewId,
    revision: Revision,
}

impl WorkspaceSnapshot {
    pub fn load(root: impl AsRef<Path>) -> io::Result<Self> {
        Self::load_with_overlays(root, SourceOverlays::new())
    }

    /// Loads a workspace while preferring unsaved source overlays over disk contents.
    pub fn load_with_overlays(
        root: impl AsRef<Path>,
        overlays: SourceOverlays,
    ) -> io::Result<Self> {
        Self::load_with_overlays_at_revision(root, overlays, 0)
    }

    /// Loads a complete immutable workspace snapshot at a caller-assigned revision.
    ///
    /// The analysis data is still rebuilt in one pass. The revision lets consumers
    /// reject results that were produced for an older document state.
    pub fn load_with_overlays_at_revision(
        root: impl AsRef<Path>,
        overlays: SourceOverlays,
        revision: u64,
    ) -> io::Result<Self> {
        let engine = VaultEngine::open(root)?;
        let view_id = ViewId::allocate();
        Self::build(
            &engine,
            overlays,
            DocumentVersions::new(),
            &AnalyzerConfiguration::default(),
            FunctionEnvironmentId::BUILTINS,
            view_id,
            Revision(revision),
        )
    }

    fn build(
        engine: &VaultEngine,
        overlays: SourceOverlays,
        document_versions: DocumentVersions,
        analyzer_configuration: &AnalyzerConfiguration,
        function_environment: FunctionEnvironmentId,
        view_id: ViewId,
        revision: Revision,
    ) -> io::Result<Self> {
        let root = engine.root().to_path_buf();
        let overlays = normalize_overlays(&root, overlays)?;
        if let Some(path) = overlays.keys().find(|path| !path.starts_with(&root)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("overlay source `{}` is outside the vault", path.display()),
            ));
        }
        let document_versions = normalize_document_versions(&root, document_versions)?;
        let configuration = if let Some(configuration) = &analyzer_configuration.manifest_override {
            Some(configuration.clone())
        } else {
            let configuration_path = root.join(MANIFEST_FILE);
            configuration_path
                .is_file()
                .then(|| fs::read_to_string(configuration_path).map(Arc::from))
                .transpose()?
        };
        let mut workspace = Self {
            root: root.clone(),
            configuration,
            sources: BTreeMap::new(),
            source_ids: BTreeMap::new(),
            modules: BTreeMap::new(),
            module_paths: BTreeMap::new(),
            labels: Vec::new(),
            references: Vec::new(),
            diagnostics: Vec::new(),
            signatures: analyzer_configuration.signatures.clone(),
            module_signatures: BTreeMap::new(),
            module_semantics: BTreeMap::new(),
            attribute_keys: BTreeSet::new(),
            function_environment,
            view_id,
            revision,
        };
        workspace.insert_virtual_module(engine, ModulePath::root());
        workspace.scan_directory(
            engine,
            &root,
            &ModulePath::root(),
            &overlays,
            &document_versions,
        )?;
        workspace.insert_overlay_only_modules(engine, &overlays, &document_versions)?;
        workspace.build_module_signatures();
        workspace.analyze_references();
        Ok(workspace)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the document revision represented by this snapshot.
    pub fn revision(&self) -> Revision {
        self.revision
    }

    pub fn view_id(&self) -> ViewId {
        self.view_id
    }

    pub fn configuration(&self) -> Option<&str> {
        self.configuration.as_deref()
    }

    pub fn signatures(&self) -> &SignatureSet {
        &self.signatures
    }

    /// Returns the schema visible inside one module, including its source-defined functions.
    pub fn module_signatures(&self, module_id: ModuleId) -> Option<&SignatureSet> {
        self.module_signatures.get(&module_id)
    }

    /// Returns resolved module-local symbol identities and their use sites.
    pub fn module_semantics(&self, module_id: ModuleId) -> Option<&ModuleSemanticIndex> {
        self.module_semantics.get(&module_id)
    }

    pub fn function_environment(&self) -> FunctionEnvironmentId {
        self.function_environment
    }

    pub fn sources(&self) -> impl Iterator<Item = &SourceInput> {
        self.sources.values()
    }

    pub fn source(&self, file_id: FileId) -> Option<&SourceInput> {
        self.sources.get(&file_id)
    }

    pub fn file_id(&self, path: &Path) -> Option<FileId> {
        self.source_ids.get(path).copied()
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

    pub fn labels(&self) -> &[LabelDefinition] {
        &self.labels
    }

    pub fn label(&self, module: &ModulePath, name: &str) -> Option<&LabelDefinition> {
        self.labels
            .iter()
            .find(|label| &label.module == module && label.name == name)
    }

    /// Returns a module by its logical path.
    pub fn module(&self, path: &ModulePath) -> Option<&Module> {
        self.modules.get(path)
    }

    pub fn module_by_id(&self, module_id: ModuleId) -> Option<&Module> {
        self.module_paths
            .get(&module_id)
            .and_then(|path| self.modules.get(path))
    }

    pub fn module_at(&self, file_id: FileId) -> Option<&Module> {
        self.modules
            .values()
            .find(|module| module.file_id == Some(file_id))
    }

    /// Returns the source-backed module associated with a filesystem path.
    pub fn module_for_source(&self, path: &Path) -> Option<&Module> {
        self.file_id(path)
            .and_then(|file_id| self.module_at(file_id))
    }

    pub fn diagnostics_for(&self, file_id: FileId) -> impl Iterator<Item = &Diagnostic> {
        let path = self.source(file_id).map(|source| &source.canonical_path);
        self.diagnostics
            .iter()
            .filter(move |diagnostic| diagnostic.source_path.as_ref() == path)
    }

    pub fn references_to(&self, module_id: ModuleId) -> impl Iterator<Item = &ResolvedReference> {
        self.references
            .iter()
            .filter(move |reference| reference.target_module_id == module_id)
    }

    pub fn reference_target_at(&self, file_id: FileId, offset: usize) -> Option<ReferenceTarget> {
        if let Some(reference) = self.references_at(file_id, offset).next() {
            return Some(ReferenceTarget {
                revision: self.revision,
                module_id: reference.target_module_id,
                annotation: reference.target_label.as_ref().map(|name| AnnotationId {
                    module_id: reference.target_module_id,
                    name: name.clone(),
                }),
            });
        }
        if let Some(label) = self.labels.iter().find(|label| {
            label.file_id == file_id && label.range.start <= offset && offset <= label.range.end
        }) {
            return Some(ReferenceTarget {
                revision: self.revision,
                module_id: label.id.module_id,
                annotation: Some(label.id.clone()),
            });
        }
        (offset == 0).then(|| {
            let module = self.module_at(file_id)?;
            Some(ReferenceTarget {
                revision: self.revision,
                module_id: module.id,
                annotation: None,
            })
        })?
    }

    pub fn symbol_at(&self, file_id: FileId, offset: usize) -> Option<SymbolId> {
        if let Some(module) = self.module_at(file_id)
            && let Some(semantics) = self.module_semantics(module.id)
            && let Some(symbol) = semantics
                .references
                .iter()
                .find(|reference| reference.range.start <= offset && offset < reference.range.end)
                .map(|reference| reference.symbol)
                .or_else(|| {
                    semantics
                        .definitions
                        .iter()
                        .find(|definition| {
                            definition.range.start <= offset && offset < definition.range.end
                        })
                        .map(|definition| definition.id)
                })
        {
            return Some(SymbolId::Local {
                module_id: module.id,
                symbol,
            });
        }
        let target = self.reference_target_at(file_id, offset)?;
        Some(match target.annotation {
            Some(annotation) => SymbolId::Annotation(annotation),
            None => SymbolId::Module(target.module_id),
        })
    }

    pub fn symbol_locations_at(
        &self,
        file_id: FileId,
        offset: usize,
        include_definition: bool,
    ) -> Vec<SymbolLocation> {
        let Some(symbol) = self.symbol_at(file_id, offset) else {
            return Vec::new();
        };
        match &symbol {
            SymbolId::Local {
                module_id,
                symbol: local,
            } => {
                let Some(module) = self.module_by_id(*module_id) else {
                    return Vec::new();
                };
                let Some(file_id) = module.file_id else {
                    return Vec::new();
                };
                let Some(semantics) = self.module_semantics(*module_id) else {
                    return Vec::new();
                };
                let mut locations = Vec::new();
                if include_definition
                    && let Some(definition) = semantics
                        .definitions
                        .iter()
                        .find(|definition| definition.id == *local)
                {
                    locations.push(SymbolLocation {
                        revision: self.revision,
                        symbol: symbol.clone(),
                        file_id,
                        range: definition.range,
                        is_definition: true,
                    });
                }
                locations.extend(
                    semantics
                        .references
                        .iter()
                        .filter(|reference| reference.symbol == *local)
                        .map(|reference| SymbolLocation {
                            revision: self.revision,
                            symbol: symbol.clone(),
                            file_id,
                            range: reference.range,
                            is_definition: false,
                        }),
                );
                locations
            }
            SymbolId::Module(module_id) => {
                let mut locations = Vec::new();
                if include_definition
                    && let Some(module) = self.module_by_id(*module_id)
                    && let Some(file_id) = module.file_id
                {
                    locations.push(SymbolLocation {
                        revision: self.revision,
                        symbol: symbol.clone(),
                        file_id,
                        range: TextRange::new(0, 0),
                        is_definition: true,
                    });
                }
                locations.extend(
                    self.references_to(*module_id)
                        .filter(|reference| reference.target_label.is_none())
                        .map(|reference| SymbolLocation {
                            revision: self.revision,
                            symbol: symbol.clone(),
                            file_id: reference.source_file_id,
                            range: reference.range,
                            is_definition: false,
                        }),
                );
                locations
            }
            SymbolId::Annotation(annotation) => {
                let mut locations = Vec::new();
                if include_definition
                    && let Some(module) = self.module_by_id(annotation.module_id)
                    && let Some(definition) = self.label(&module.logical_path, &annotation.name)
                {
                    locations.push(SymbolLocation {
                        revision: self.revision,
                        symbol: symbol.clone(),
                        file_id: definition.file_id,
                        range: definition.range,
                        is_definition: true,
                    });
                }
                locations.extend(
                    self.references
                        .iter()
                        .filter(|reference| {
                            reference.target_module_id == annotation.module_id
                                && reference.target_label.as_deref()
                                    == Some(annotation.name.as_str())
                        })
                        .map(|reference| SymbolLocation {
                            revision: self.revision,
                            symbol: symbol.clone(),
                            file_id: reference.source_file_id,
                            range: reference.range,
                            is_definition: false,
                        }),
                );
                locations
            }
        }
    }

    pub fn references_for_target(
        &self,
        target: &ReferenceTarget,
    ) -> impl Iterator<Item = SnapshotResult<'_, ResolvedReference>> {
        let annotation_name = target
            .annotation
            .as_ref()
            .map(|annotation| annotation.name.as_str());
        self.references
            .iter()
            .filter(move |reference| {
                reference.target_module_id == target.module_id
                    && reference.target_label.as_deref() == annotation_name
            })
            .map(|reference| SnapshotResult {
                revision: self.revision,
                value: reference,
            })
    }

    pub fn reference_results_to(
        &self,
        module_id: ModuleId,
    ) -> impl Iterator<Item = SnapshotResult<'_, ResolvedReference>> {
        self.references_to(module_id)
            .map(|reference| SnapshotResult {
                revision: self.revision,
                value: reference,
            })
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
        engine: &VaultEngine,
        directory: &Path,
        module_path: &ModulePath,
        overlays: &SourceOverlays,
        document_versions: &DocumentVersions,
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
                if path != self.root && path.join(MANIFEST_FILE).is_file() {
                    continue;
                }
                let child = module_path.child([entry.file_name().to_string_lossy().into_owned()]);
                if self.scan_directory(engine, &path, &child, overlays, document_versions)? {
                    self.insert_virtual_module(engine, child);
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
                    .unwrap_or_else(|| engine.read_disk_source(&path))?;
                self.insert_source_module(
                    engine,
                    logical_path,
                    path,
                    source,
                    overlays,
                    document_versions,
                );
                contains_notist_file = true;
            }
        }
        Ok(contains_notist_file)
    }

    fn insert_virtual_module(&mut self, engine: &VaultEngine, logical_path: ModulePath) {
        let id = engine.module_id(&logical_path);
        self.module_paths.insert(id, logical_path.clone());
        self.modules.entry(logical_path.clone()).or_insert(Module {
            id,
            logical_path,
            file_id: None,
            source_path: None,
            source: None,
            parse: None,
        });
    }

    fn insert_source_module(
        &mut self,
        engine: &VaultEngine,
        logical_path: ModulePath,
        path: PathBuf,
        source: Arc<str>,
        overlays: &SourceOverlays,
        document_versions: &DocumentVersions,
    ) {
        let file_id = engine.file_id(&path);
        let parse = engine.parse(source.clone());
        let source_input = SourceInput {
            file_id,
            canonical_path: path.clone(),
            text: source.clone(),
            origin: if overlays.contains_key(&path) {
                SourceOrigin::Overlay
            } else {
                SourceOrigin::Disk
            },
            document_version: document_versions.get(&path).copied(),
            parse: parse.clone(),
            line_index: LineIndex::new(&source),
        };
        let module_id = engine.module_id(&logical_path);
        self.module_paths.insert(module_id, logical_path.clone());
        let module = self.modules.entry(logical_path.clone()).or_insert(Module {
            id: module_id,
            logical_path: logical_path.clone(),
            file_id: None,
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
            module.file_id = Some(file_id);
            module.source_path = Some(path);
            module.source = Some(source);
            module.parse = Some(parse);
            self.source_ids
                .insert(source_input.canonical_path.clone(), file_id);
            self.sources.insert(file_id, source_input);
        }
    }

    pub fn references_at(
        &self,
        file_id: FileId,
        offset: usize,
    ) -> impl Iterator<Item = &ResolvedReference> {
        self.references.iter().filter(move |reference| {
            reference.source_file_id == file_id
                && reference.range.start <= offset
                && offset < reference.range.end
        })
    }

    pub fn reference_results_at(
        &self,
        file_id: FileId,
        offset: usize,
    ) -> impl Iterator<Item = SnapshotResult<'_, ResolvedReference>> {
        self.references_at(file_id, offset)
            .map(|reference| SnapshotResult {
                revision: self.revision,
                value: reference,
            })
    }

    pub fn definition_at(&self, file_id: FileId, offset: usize) -> Option<DefinitionTarget> {
        if let Some(module) = self.module_at(file_id)
            && let Some(semantics) = self.module_semantics(module.id)
        {
            let symbol = semantics
                .references
                .iter()
                .find(|reference| reference.range.start <= offset && offset < reference.range.end)
                .map(|reference| reference.symbol)
                .or_else(|| {
                    semantics
                        .definitions
                        .iter()
                        .find(|definition| {
                            definition.range.start <= offset && offset < definition.range.end
                        })
                        .map(|definition| definition.id)
                });
            if let Some(definition) = symbol.and_then(|symbol| {
                semantics
                    .definitions
                    .iter()
                    .find(|definition| definition.id == symbol)
            }) {
                return Some(DefinitionTarget {
                    revision: self.revision,
                    module_id: module.id,
                    file_id: module.file_id,
                    range: Some(definition.range),
                    annotation: None,
                });
            }
        }

        let reference = self.references_at(file_id, offset).next()?;
        let module = self.module_by_id(reference.target_module_id)?;
        let label = reference
            .target_label
            .as_deref()
            .and_then(|name| self.label(&module.logical_path, name));
        Some(DefinitionTarget {
            revision: self.revision,
            module_id: module.id,
            file_id: label.map(|label| label.file_id).or(module.file_id),
            range: label.map(|label| label.range),
            annotation: label.map(|label| label.id.clone()),
        })
    }

    /// Evaluates and structures one module using only text captured by this snapshot.
    pub fn structured_document(&self, module_id: ModuleId) -> Option<StructuredDocument> {
        self.structured_module(module_id)
            .map(|structured| structured.document)
    }

    pub fn structured_module(&self, module_id: ModuleId) -> Option<StructuredModule> {
        let module = self.module_by_id(module_id)?;
        let source = module.source.as_deref()?;
        let structured = structure(Evaluator::default().evaluate(source));
        Some(StructuredModule {
            revision: self.revision,
            module_id,
            function_environment: self.function_environment,
            document: structured.document,
            diagnostics: structured.diagnostics,
        })
    }

    pub fn document_symbols(&self, file_id: FileId) -> Vec<DocumentSymbol> {
        let Some(module) = self.module_at(file_id) else {
            return Vec::new();
        };
        let Some(document) = self.structured_document(module.id) else {
            return Vec::new();
        };
        let mut symbols = Vec::new();
        collect_document_symbols(&document, self.revision, file_id, &mut symbols);
        symbols
    }

    pub fn workspace_symbols(&self, query: &str) -> Vec<ModuleSymbol> {
        let query = query.to_lowercase();
        let mut symbols = self
            .modules()
            .filter_map(|module| {
                let file_id = module.file_id?;
                let name = module.logical_path.to_string();
                name.to_lowercase()
                    .contains(&query)
                    .then_some(ModuleSymbol {
                        revision: self.revision,
                        module_id: module.id,
                        file_id,
                        name,
                        kind: WorkspaceSymbolKind::Module,
                        range: TextRange::new(0, 0),
                        annotation: None,
                    })
            })
            .collect::<Vec<_>>();
        symbols.extend(
            self.labels
                .iter()
                .filter(|label| label.name.to_lowercase().contains(&query))
                .map(|label| ModuleSymbol {
                    revision: self.revision,
                    module_id: label.id.module_id,
                    file_id: label.file_id,
                    name: label.name.clone(),
                    kind: WorkspaceSymbolKind::Annotation,
                    range: label.range,
                    annotation: Some(label.id.clone()),
                }),
        );
        symbols.sort_by(|left, right| left.name.cmp(&right.name));
        symbols
    }

    pub fn hover_at(&self, file_id: FileId, offset: usize) -> Option<HoverInfo> {
        if let Some(reference) = self.references_at(file_id, offset).next() {
            let mut contents = reference.target_module.to_string();
            if let Some(label) = &reference.target_label {
                contents.push('#');
                contents.push_str(label);
            }
            if let Some(module) = self.module_by_id(reference.target_module_id) {
                match &module.source_path {
                    Some(path) => contents.push_str(&format!("\n\n`{}`", path.display())),
                    None => contents.push_str("\n\nVirtual directory module"),
                }
            }
            return Some(HoverInfo {
                revision: self.revision,
                file_id,
                range: reference.range,
                contents,
            });
        }
        let source = self.source(file_id)?;
        for annotation in source.parse.annotations() {
            if let Some(id) = &annotation.attributes.id
                && contains(id.range, offset)
            {
                return Some(HoverInfo {
                    revision: self.revision,
                    file_id,
                    range: id.range,
                    contents: format!("`{}: AnnotationId`", id.value),
                });
            }
            for item in &annotation.attributes.items {
                let (range, contents) = match item {
                    notist_syntax::Attribute::Tag(name) if contains(name.range, offset) => {
                        (name.range, format!("`{}: AnnotationTag`", name.value))
                    }
                    notist_syntax::Attribute::Class(name) if contains(name.range, offset) => {
                        (name.range, format!("`{}: AnnotationClass`", name.value))
                    }
                    notist_syntax::Attribute::KeyValue { key, .. }
                        if contains(key.range, offset) =>
                    {
                        (key.range, format!("`{}: AnnotationProperty`", key.value))
                    }
                    _ => continue,
                };
                return Some(HoverInfo {
                    revision: self.revision,
                    file_id,
                    range,
                    contents,
                });
            }
        }
        if let Some(SymbolId::Local { module_id, symbol }) = self.symbol_at(file_id, offset)
            && let Some(semantics) = self.module_semantics(module_id)
            && let Some(definition) = semantics
                .definitions
                .iter()
                .find(|definition| definition.id == symbol)
        {
            let range = semantics
                .references
                .iter()
                .find(|reference| {
                    reference.symbol == symbol
                        && reference.range.start <= offset
                        && offset < reference.range.end
                })
                .map_or(definition.range, |reference| reference.range);
            let contents = match definition.kind {
                SymbolKind::Function => self
                    .module_signatures(module_id)
                    .and_then(|signatures| signatures.get(&definition.name))
                    .map_or_else(
                        || format!("`{}: Function`", definition.name),
                        |signature| format_signature(&definition.name, signature),
                    ),
                SymbolKind::Parameter => format!("`{}: {}`", definition.name, definition.ty),
            };
            return Some(HoverInfo {
                revision: self.revision,
                file_id,
                range,
                contents,
            });
        }
        let call = source
            .parse
            .calls()
            .into_iter()
            .find(|call| call.name.range.start <= offset && offset <= call.name.range.end)?;
        let signatures = self
            .module_at(file_id)
            .and_then(|module| self.module_signatures(module.id))
            .unwrap_or(&self.signatures);
        let signature = signatures.get(&call.name.value)?;
        Some(HoverInfo {
            revision: self.revision,
            file_id,
            range: call.name.range,
            contents: format_signature(&call.name.value, signature),
        })
    }

    pub fn completions_at(&self, file_id: FileId, offset: usize) -> Vec<CompletionCandidate> {
        let Some(source) = self.source(file_id) else {
            return Vec::new();
        };
        if offset > source.text.len() || !source.text.is_char_boundary(offset) {
            return Vec::new();
        }
        if is_in_raw_literal(&source.parse, offset) || is_in_string_literal(&source.parse, offset) {
            return Vec::new();
        }
        if let Some((call, context)) =
            argument_completion_context(&source.text, &source.parse, offset)
            && let Some(signature) = self
                .module_at(file_id)
                .and_then(|module| self.module_signatures(module.id))
                .unwrap_or(&self.signatures)
                .get(&call.name.value)
        {
            let used = used_argument_parameters(signature, call);
            return signature
                .parameters
                .iter()
                .filter(|parameter| {
                    !used.contains(parameter.name.as_str())
                        && signature.trailing_content.as_deref() != Some(parameter.name.as_str())
                        && starts_with_case_insensitive(&parameter.name, &context.prefix)
                })
                .map(|parameter| CompletionCandidate {
                    revision: self.revision,
                    kind: CompletionKind::Parameter,
                    label: parameter.name.clone(),
                    detail: parameter.ty.to_string(),
                    documentation: Some(format!(
                        "Parameter of `{}` with type {}.",
                        call.name.value, parameter.ty
                    )),
                    replacement: context.replace,
                    insert_text: format!("{}=", parameter.name),
                    module_id: None,
                })
                .collect();
        }
        if let Some(context) = wiki_completion_context(&source.text, &source.parse, offset)
            && let Some(current) = self.module_at(file_id)
        {
            let mut candidates: Vec<_> = self
                .modules()
                .filter(|target| target.id != current.id)
                .filter_map(|target| {
                    let reference = completion_module_reference(
                        &current.logical_path,
                        &target.logical_path,
                        &context.prefix,
                    );
                    starts_with_case_insensitive(&reference, &context.prefix).then(|| {
                        CompletionCandidate {
                            revision: self.revision,
                            kind: CompletionKind::Module,
                            label: reference.clone(),
                            detail: target.logical_path.to_string(),
                            documentation: target
                                .source_path
                                .as_ref()
                                .map(|path| format!("Notist module at `{}`.", path.display())),
                            replacement: context.replace,
                            insert_text: reference,
                            module_id: Some(target.id),
                        }
                    })
                })
                .collect();
            candidates.sort_by(|left, right| left.label.cmp(&right.label));
            return candidates;
        }
        if let Some(context) = function_completion_context(&source.text, &source.parse, offset) {
            let signatures = self
                .module_at(file_id)
                .and_then(|module| self.module_signatures(module.id))
                .unwrap_or(&self.signatures);
            let mut candidates: Vec<_> = signatures
                .iter()
                .filter(|(name, _)| starts_with_case_insensitive(name, &context.prefix))
                .map(|(name, signature)| CompletionCandidate {
                    revision: self.revision,
                    kind: CompletionKind::Function,
                    label: name.into(),
                    detail: format_signature(name, signature),
                    documentation: Some(format!("Notist function `{name}`.")),
                    replacement: context.replace,
                    insert_text: name.into(),
                    module_id: None,
                })
                .collect();
            candidates.sort_by(|left, right| left.label.cmp(&right.label));
            return candidates;
        }
        if let Some(context) = attribute_completion_context(&source.text, &source.parse, offset) {
            let mut candidates = self
                .attribute_keys
                .iter()
                .filter(|key| starts_with_case_insensitive(key, &context.prefix))
                .map(|key| CompletionCandidate {
                    revision: self.revision,
                    kind: CompletionKind::Attribute,
                    label: key.clone(),
                    detail: "Annotation property".into(),
                    documentation: Some(format!("Observed annotation property `{key}`.")),
                    replacement: context.replace,
                    insert_text: format!("{key}="),
                    module_id: None,
                })
                .collect::<Vec<_>>();
            candidates.sort_by(|left, right| left.label.cmp(&right.label));
            return candidates;
        }
        Vec::new()
    }

    pub fn search_context(&self, query: &str) -> Vec<SearchContext> {
        self.search_context_cancellable(query, || false)
    }

    pub fn search_context_cancellable(
        &self,
        query: &str,
        mut cancelled: impl FnMut() -> bool,
    ) -> Vec<SearchContext> {
        if query.is_empty() {
            return Vec::new();
        }
        let mut results = Vec::new();
        for source in self.sources() {
            if cancelled() {
                break;
            }
            let Some(module) = self.module_at(source.file_id) else {
                continue;
            };
            let mut start = 0;
            while let Some(relative) = source.text[start..].find(query) {
                if cancelled() {
                    return results;
                }
                let match_start = start + relative;
                let match_end = match_start + query.len();
                let line_start = source.text[..match_start]
                    .rfind('\n')
                    .map_or(0, |index| index + 1);
                let line_end = source.text[match_end..]
                    .find('\n')
                    .map_or(source.text.len(), |index| match_end + index);
                results.push(SearchContext {
                    revision: self.revision,
                    file_id: source.file_id,
                    module_id: module.id,
                    range: TextRange::new(match_start, match_end),
                    snippet: source.text[line_start..line_end].into(),
                });
                start = match_end.max(start + 1);
            }
        }
        results
    }

    /// Derives an invalidatable semantic delta from two complete snapshots.
    pub fn delta_from(&self, previous: &Self) -> Option<WorkspaceDelta> {
        if self.view_id != previous.view_id || self.revision <= previous.revision {
            return None;
        }
        let previous_files: BTreeSet<_> = previous.sources.keys().copied().collect();
        let current_files: BTreeSet<_> = self.sources.keys().copied().collect();
        let previous_modules: BTreeSet<_> = previous.module_paths.keys().copied().collect();
        let current_modules: BTreeSet<_> = self.module_paths.keys().copied().collect();
        let changed_files: Vec<FileId> = previous_files
            .intersection(&current_files)
            .filter(|file_id| {
                previous
                    .sources
                    .get(file_id)
                    .map(|source| source.text.as_ref())
                    != self.sources.get(file_id).map(|source| source.text.as_ref())
            })
            .copied()
            .collect();
        let changed_modules = previous_modules
            .intersection(&current_modules)
            .filter(|module_id| {
                let previous = previous.module_by_id(**module_id);
                let current = self.module_by_id(**module_id);
                previous.map(|module| (module.file_id, &module.logical_path))
                    != current.map(|module| (module.file_id, &module.logical_path))
                    || current
                        .and_then(|module| module.file_id)
                        .is_some_and(|file_id| changed_files.contains(&file_id))
            })
            .copied()
            .collect();
        let changed_references =
            changed_semantic_files(&previous.references, &self.references, |reference| {
                reference.source_file_id
            });
        let changed_diagnostics = changed_diagnostic_files(previous, self);
        Some(WorkspaceDelta {
            from_revision: previous.revision,
            to_revision: self.revision,
            added_files: current_files.difference(&previous_files).copied().collect(),
            changed_files,
            removed_files: previous_files.difference(&current_files).copied().collect(),
            added_modules: current_modules
                .difference(&previous_modules)
                .copied()
                .collect(),
            changed_modules,
            removed_modules: previous_modules
                .difference(&current_modules)
                .copied()
                .collect(),
            changed_references,
            changed_diagnostics,
        })
    }

    fn insert_overlay_only_modules(
        &mut self,
        engine: &VaultEngine,
        overlays: &SourceOverlays,
        document_versions: &DocumentVersions,
    ) -> io::Result<()> {
        for (path, source) in overlays {
            if !is_notist_file(path)
                || !path.starts_with(&self.root)
                || self.module_for_source(path).is_some()
                || find_vault_root(path, Some(&self.root))?.is_some_and(|root| root != self.root)
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
                self.insert_virtual_module(
                    engine,
                    ModulePath::from_segments(logical_path.segments()[..count].iter().cloned()),
                );
            }
            self.insert_source_module(
                engine,
                logical_path,
                path.clone(),
                source.clone(),
                overlays,
                document_versions,
            );
        }
        Ok(())
    }

    fn build_module_signatures(&mut self) {
        self.module_signatures.clear();
        self.module_semantics.clear();
        self.attribute_keys.clear();
        for module in self.modules.values() {
            let Some(parse) = module.parse.as_ref() else {
                continue;
            };
            let mut signatures = self.signatures.clone();
            signatures.extend_with_user_functions(parse);
            self.module_signatures.insert(module.id, signatures);
            self.module_semantics
                .insert(module.id, resolve_module_symbols(parse));
            for annotation in parse.annotations() {
                for item in &annotation.attributes.items {
                    if let notist_syntax::Attribute::KeyValue { key, .. } = item {
                        self.attribute_keys.insert(key.value.clone());
                    }
                }
            }
        }
    }

    fn analyze_references(&mut self) {
        let mut diagnostics = Vec::new();
        let mut labels = Vec::new();
        let mut label_indexes = BTreeMap::new();
        let mut references = Vec::new();
        let signatures = self.signatures.clone();

        for module in self.modules.values() {
            let (Some(file_id), Some(source_path), Some(parse)) =
                (module.file_id, &module.source_path, &module.parse)
            else {
                continue;
            };
            for annotation in parse.annotations() {
                let Some(id) = &annotation.attributes.id else {
                    continue;
                };
                let key = (module.logical_path.clone(), id.value.clone());
                if label_indexes.contains_key(&key) {
                    diagnostics.push(Diagnostic {
                        kind: DiagnosticKind::DuplicateLabel,
                        message: format!("duplicate label `{}`", id.value),
                        source_path: Some(source_path.clone()),
                        range: Some(id.range),
                    });
                    continue;
                }
                label_indexes.insert(key, labels.len());
                labels.push(LabelDefinition {
                    id: AnnotationId {
                        module_id: module.id,
                        name: id.value.clone(),
                    },
                    file_id,
                    module: module.logical_path.clone(),
                    source_path: source_path.clone(),
                    name: id.value.clone(),
                    range: id.range,
                    scope_range: annotation.scope_range,
                });
            }
        }

        for module in self.modules.values() {
            let (Some(file_id), Some(source_path), Some(source), Some(parse)) = (
                module.file_id,
                &module.source_path,
                &module.source,
                &module.parse,
            ) else {
                continue;
            };

            diagnostics.extend(parse.errors.iter().map(|error| Diagnostic {
                kind: DiagnosticKind::InvalidSyntax,
                message: error.message.clone(),
                source_path: Some(source_path.clone()),
                range: Some(error.range),
            }));

            let mut module_references: Vec<_> = parse
                .links()
                .into_iter()
                .map(|link| (link.target.clone(), link.range))
                .collect();
            for call in parse.calls() {
                if let Some(reference) = explicit_ref_target(call) {
                    match reference {
                        Ok(reference) => module_references.push((reference, call.range)),
                        Err(message) => diagnostics.push(Diagnostic {
                            kind: DiagnosticKind::InvalidArguments,
                            message,
                            source_path: Some(source_path.clone()),
                            range: Some(call.range),
                        }),
                    }
                }
            }

            for (reference, range) in module_references {
                let Some(target) = reference.module.resolve_from(&module.logical_path) else {
                    diagnostics.push(Diagnostic {
                        kind: DiagnosticKind::UnresolvedModule,
                        message: "module path escapes above `vault`".into(),
                        source_path: Some(source_path.clone()),
                        range: Some(range),
                    });
                    continue;
                };

                let Some(target_module) = self.modules.get(&target) else {
                    diagnostics.push(Diagnostic {
                        kind: DiagnosticKind::UnresolvedModule,
                        message: format!("unresolved module `{target}`"),
                        source_path: Some(source_path.clone()),
                        range: Some(range),
                    });
                    continue;
                };

                let target_definition = reference.label.as_ref().and_then(|label| {
                    label_indexes
                        .get(&(target.clone(), label.clone()))
                        .map(|index| &labels[*index])
                });
                if let Some(label) = &reference.label
                    && target_definition.is_none()
                {
                    diagnostics.push(Diagnostic {
                        kind: DiagnosticKind::UnresolvedLabel,
                        message: format!("unresolved label `{label}` in module `{target}`"),
                        source_path: Some(source_path.clone()),
                        range: Some(range),
                    });
                    continue;
                }

                references.push(ResolvedReference {
                    source_file_id: file_id,
                    source_module_id: module.id,
                    source_module: module.logical_path.clone(),
                    source_path: source_path.clone(),
                    range,
                    target_module_id: target_module.id,
                    target_module: target,
                    target_label: reference.label,
                    target_range: target_definition.map(|definition| definition.range),
                });
            }
            let checks = check_module(parse, &signatures);
            let runtime = safe_evaluation_diagnostics(source, &signatures);
            diagnostics.extend(checks.iter().cloned().map(|diagnostic| Diagnostic {
                kind: diagnostic.kind,
                message: diagnostic.message,
                source_path: Some(source_path.clone()),
                range: Some(diagnostic.range),
            }));
            diagnostics.extend(runtime.into_iter().filter_map(|diagnostic| {
                (!checks.iter().any(|check| {
                    check.range == diagnostic.range && check.message == diagnostic.message
                }))
                .then(|| Diagnostic {
                    kind: DiagnosticKind::Evaluation,
                    message: diagnostic.message,
                    source_path: Some(source_path.clone()),
                    range: Some(diagnostic.range),
                })
            }));
        }
        self.labels = labels;
        self.references = references;
        self.diagnostics.extend(diagnostics);
    }
}

/// Shared state for one canonical Notist vault.
///
#[derive(Clone, Debug)]
pub struct VaultEngine {
    root: Arc<PathBuf>,
    state: Arc<Mutex<VaultEngineState>>,
}

#[derive(Debug, Default)]
struct VaultEngineState {
    next_file_id: u64,
    next_module_id: u64,
    next_function_environment: u64,
    file_ids: BTreeMap<PathBuf, FileId>,
    module_ids: BTreeMap<ModulePath, ModuleId>,
    disk_sources: BTreeMap<PathBuf, Arc<str>>,
    parse_cache: BTreeMap<Arc<str>, Parse>,
}

impl VaultEngine {
    /// Opens the vault rooted at `root`.
    pub fn open(root: impl AsRef<Path>) -> io::Result<Self> {
        Ok(Self {
            root: Arc::new(dunce::canonicalize(root)?),
            state: Arc::new(Mutex::new(VaultEngineState::default())),
        })
    }

    pub fn root(&self) -> &Path {
        self.root.as_path()
    }

    /// Returns the most recently captured disk text for a source, if any.
    pub fn cached_disk_source(&self, path: &Path) -> Option<Arc<str>> {
        let path = normalize_source_path(self.root(), path).ok()?;
        self.state
            .lock()
            .expect("vault engine state poisoned")
            .disk_sources
            .get(&path)
            .cloned()
    }

    /// Opens a view containing only saved disk sources.
    pub fn disk_view(&self) -> io::Result<AnalyzerView> {
        self.view(SourceOverlays::new())
    }

    /// Opens an isolated analyzer view with the supplied source overlays.
    pub fn view(&self, overlays: SourceOverlays) -> io::Result<AnalyzerView> {
        self.view_with_versions(overlays, DocumentVersions::new())
    }

    pub fn view_with_versions(
        &self,
        overlays: SourceOverlays,
        document_versions: DocumentVersions,
    ) -> io::Result<AnalyzerView> {
        AnalyzerView::new(
            self.clone(),
            overlays,
            document_versions,
            AnalyzerConfiguration::default(),
            FunctionEnvironmentId::BUILTINS,
        )
    }

    pub fn configured_view(
        &self,
        overlays: SourceOverlays,
        document_versions: DocumentVersions,
        configuration: AnalyzerConfiguration,
    ) -> io::Result<AnalyzerView> {
        let function_environment = self.allocate_function_environment();
        AnalyzerView::new(
            self.clone(),
            overlays,
            document_versions,
            configuration,
            function_environment,
        )
    }

    /// Preserves a stable FileId when an upper layer reports an explicit rename.
    pub fn rename_source(&self, from: &Path, to: &Path) -> io::Result<()> {
        let from = normalize_source_path(self.root(), from)?;
        let to = normalize_source_path(self.root(), to)?;
        if !from.starts_with(self.root()) || !to.starts_with(self.root()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "source rename crosses the vault boundary",
            ));
        }
        let mut state = self.state.lock().expect("vault engine state poisoned");
        let Some(file_id) = state.file_ids.remove(&from) else {
            return Ok(());
        };
        if state.file_ids.contains_key(&to) {
            state.file_ids.insert(from, file_id);
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "rename target already has a FileId",
            ));
        }
        state.file_ids.insert(to, file_id);
        Ok(())
    }

    fn file_id(&self, path: &Path) -> FileId {
        let mut state = self.state.lock().expect("vault engine state poisoned");
        if let Some(file_id) = state.file_ids.get(path) {
            return *file_id;
        }
        let file_id = FileId(state.next_file_id);
        state.next_file_id += 1;
        state.file_ids.insert(path.to_path_buf(), file_id);
        file_id
    }

    fn module_id(&self, path: &ModulePath) -> ModuleId {
        let mut state = self.state.lock().expect("vault engine state poisoned");
        if let Some(module_id) = state.module_ids.get(path) {
            return *module_id;
        }
        let module_id = ModuleId(state.next_module_id);
        state.next_module_id += 1;
        state.module_ids.insert(path.clone(), module_id);
        module_id
    }

    fn parse(&self, source: Arc<str>) -> Parse {
        let mut state = self.state.lock().expect("vault engine state poisoned");
        state
            .parse_cache
            .entry(source.clone())
            .or_insert_with(|| parse(&source))
            .clone()
    }

    fn read_disk_source(&self, path: &Path) -> io::Result<Arc<str>> {
        let source: Arc<str> = Arc::from(fs::read_to_string(path)?);
        self.state
            .lock()
            .expect("vault engine state poisoned")
            .disk_sources
            .insert(path.to_path_buf(), source.clone());
        Ok(source)
    }

    fn allocate_function_environment(&self) -> FunctionEnvironmentId {
        let mut state = self.state.lock().expect("vault engine state poisoned");
        state.next_function_environment += 1;
        FunctionEnvironmentId(state.next_function_environment)
    }
}

/// A client-specific analysis session over a shared [`VaultEngine`].
#[derive(Debug)]
pub struct AnalyzerView {
    engine: VaultEngine,
    overlays: SourceOverlays,
    document_versions: DocumentVersions,
    configuration: AnalyzerConfiguration,
    function_environment: FunctionEnvironmentId,
    snapshot: Arc<WorkspaceSnapshot>,
    view_id: ViewId,
}

impl AnalyzerView {
    fn new(
        engine: VaultEngine,
        overlays: SourceOverlays,
        document_versions: DocumentVersions,
        configuration: AnalyzerConfiguration,
        function_environment: FunctionEnvironmentId,
    ) -> io::Result<Self> {
        let view_id = ViewId::allocate();
        let snapshot = Arc::new(WorkspaceSnapshot::build(
            &engine,
            overlays.clone(),
            document_versions.clone(),
            &configuration,
            function_environment,
            view_id,
            Revision::INITIAL,
        )?);
        Ok(Self {
            engine,
            overlays,
            document_versions,
            configuration,
            function_environment,
            snapshot,
            view_id,
        })
    }

    pub fn engine(&self) -> &VaultEngine {
        &self.engine
    }

    pub fn overlays(&self) -> &SourceOverlays {
        &self.overlays
    }

    pub fn document_versions(&self) -> &DocumentVersions {
        &self.document_versions
    }

    pub fn configuration(&self) -> &AnalyzerConfiguration {
        &self.configuration
    }

    /// Returns the currently published immutable snapshot.
    pub fn current(&self) -> &WorkspaceSnapshot {
        &self.snapshot
    }

    /// Clones the currently published snapshot handle for a long-running query.
    pub fn snapshot(&self) -> Arc<WorkspaceSnapshot> {
        self.snapshot.clone()
    }

    /// Reloads disk inputs while retaining this view's overlays.
    pub fn reload(&mut self) -> io::Result<Arc<WorkspaceSnapshot>> {
        self.publish(self.overlays.clone(), self.document_versions.clone())
    }

    pub fn reload_publication(&mut self) -> io::Result<SnapshotPublication> {
        self.publish_with_delta(self.overlays.clone(), self.document_versions.clone())
    }

    /// Atomically replaces the complete overlay set and publishes a new revision.
    ///
    /// If candidate construction fails, both the inputs and current snapshot stay
    /// unchanged.
    pub fn replace_overlays(
        &mut self,
        overlays: SourceOverlays,
    ) -> io::Result<Arc<WorkspaceSnapshot>> {
        self.publish(overlays, DocumentVersions::new())
    }

    pub fn replace_inputs(
        &mut self,
        overlays: SourceOverlays,
        document_versions: DocumentVersions,
    ) -> io::Result<Arc<WorkspaceSnapshot>> {
        self.publish(overlays, document_versions)
    }

    pub fn replace_inputs_publication(
        &mut self,
        overlays: SourceOverlays,
        document_versions: DocumentVersions,
    ) -> io::Result<SnapshotPublication> {
        self.publish_with_delta(overlays, document_versions)
    }

    pub fn replace_configuration(
        &mut self,
        configuration: AnalyzerConfiguration,
    ) -> io::Result<Arc<WorkspaceSnapshot>> {
        Ok(self
            .replace_configuration_publication(configuration)?
            .snapshot)
    }

    pub fn replace_configuration_publication(
        &mut self,
        configuration: AnalyzerConfiguration,
    ) -> io::Result<SnapshotPublication> {
        let previous = self.snapshot.clone();
        let function_environment = self.engine.allocate_function_environment();
        let candidate = self.build_candidate(
            self.overlays.clone(),
            self.document_versions.clone(),
            &configuration,
            function_environment,
        )?;
        self.configuration = configuration;
        self.function_environment = function_environment;
        self.snapshot = candidate.clone();
        let delta = candidate
            .delta_from(&previous)
            .expect("successive publications belong to one analyzer view");
        Ok(SnapshotPublication {
            snapshot: candidate,
            delta,
        })
    }

    fn publish(
        &mut self,
        overlays: SourceOverlays,
        document_versions: DocumentVersions,
    ) -> io::Result<Arc<WorkspaceSnapshot>> {
        Ok(self
            .publish_with_delta(overlays, document_versions)?
            .snapshot)
    }

    fn publish_with_delta(
        &mut self,
        overlays: SourceOverlays,
        document_versions: DocumentVersions,
    ) -> io::Result<SnapshotPublication> {
        let previous = self.snapshot.clone();
        let candidate = self.build_candidate(
            overlays.clone(),
            document_versions.clone(),
            &self.configuration,
            self.function_environment,
        )?;
        self.overlays = overlays;
        self.document_versions = document_versions;
        self.snapshot = candidate.clone();
        let delta = candidate
            .delta_from(&previous)
            .expect("successive publications belong to one analyzer view");
        Ok(SnapshotPublication {
            snapshot: candidate,
            delta,
        })
    }

    fn build_candidate(
        &self,
        overlays: SourceOverlays,
        document_versions: DocumentVersions,
        configuration: &AnalyzerConfiguration,
        function_environment: FunctionEnvironmentId,
    ) -> io::Result<Arc<WorkspaceSnapshot>> {
        let revision = self.snapshot.revision().next()?;
        Ok(Arc::new(WorkspaceSnapshot::build(
            &self.engine,
            overlays,
            document_versions,
            configuration,
            function_environment,
            self.view_id,
            revision,
        )?))
    }
}

fn contains(range: TextRange, offset: usize) -> bool {
    range.start <= offset && offset < range.end
}

fn is_in_raw_literal(parse: &Parse, offset: usize) -> bool {
    parse.raw_literals().iter().any(|raw| {
        contains(raw.range, offset)
            || (raw.payload_range.end == raw.range.end && offset == raw.range.end)
    })
}

fn is_in_string_literal(parse: &Parse, offset: usize) -> bool {
    parse
        .string_literal_ranges()
        .into_iter()
        .any(|range| contains(range, offset))
}

struct CompletionContext {
    prefix: String,
    replace: TextRange,
}

fn wiki_completion_context(
    source: &str,
    parse: &Parse,
    offset: usize,
) -> Option<CompletionContext> {
    if let Some(link) = parse
        .links()
        .into_iter()
        .find(|link| contains(link.range, offset))
    {
        let start = link.range.start + 2;
        let content_end = link.range.end.saturating_sub(2);
        let module_end = source[start..content_end]
            .find('#')
            .map_or(content_end, |relative| start + relative);
        if start <= offset && offset <= module_end {
            return Some(CompletionContext {
                prefix: source[start..offset].to_owned(),
                replace: TextRange::new(start, module_end),
            });
        }
    }
    let before = source.get(..offset)?;
    let start = before.rfind("[[")? + 2;
    if before[start..].contains("]]")
        || before[start..].contains('#')
        || before[start..].contains('\n')
    {
        return None;
    }
    Some(CompletionContext {
        prefix: source[start..offset].to_owned(),
        replace: TextRange::new(start, offset),
    })
}

fn function_completion_context(
    source: &str,
    parse: &Parse,
    offset: usize,
) -> Option<CompletionContext> {
    if let Some(call) = parse
        .calls()
        .into_iter()
        .find(|call| contains(call.name.range, offset))
    {
        return Some(CompletionContext {
            prefix: source[call.name.range.start..offset].to_owned(),
            replace: call.name.range,
        });
    }
    let before = source.get(..offset)?;
    let hash = before.rfind('#')?;
    let prefix = &source[hash + 1..offset];
    if prefix.is_empty() || prefix == "[" {
        return (prefix != "[").then(|| CompletionContext {
            prefix: String::new(),
            replace: TextRange::new(hash + 1, offset),
        });
    }
    if !prefix
        .chars()
        .all(|character| character.is_alphanumeric() || matches!(character, '_' | '-' | ':'))
    {
        return None;
    }
    Some(CompletionContext {
        prefix: prefix.to_owned(),
        replace: TextRange::new(hash + 1, offset),
    })
}

fn attribute_completion_context(
    source: &str,
    parse: &Parse,
    offset: usize,
) -> Option<CompletionContext> {
    let before = source.get(..offset)?;
    let attribute_start = before.rfind('@')?;
    parse.embedded_at(attribute_start)?;
    let tail = &source[attribute_start + 1..offset];
    if tail.contains(['\n', '\r']) {
        return None;
    }
    let relative_start = tail.rfind(',').map_or(0, |comma| comma + 1);
    let prefix = &tail[relative_start..];
    if prefix.starts_with(['#', '.'])
        || prefix.contains('=')
        || !prefix
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '_' | '-'))
    {
        return None;
    }
    let start = attribute_start + 1 + relative_start;
    Some(CompletionContext {
        prefix: prefix.to_owned(),
        replace: TextRange::new(start, offset),
    })
}

fn argument_completion_context<'a>(
    source: &str,
    parse: &'a Parse,
    offset: usize,
) -> Option<(&'a Call, CompletionContext)> {
    let call = parse
        .calls()
        .into_iter()
        .filter(|call| {
            call.arguments_range
                .is_some_and(|range| range.start <= offset && offset <= range.end)
        })
        .min_by_key(|call| call.range.end - call.range.start)?;
    let range = call.arguments_range?;
    let mut start = offset;
    while start > range.start {
        let character = source[..start].chars().next_back()?;
        if character.is_alphanumeric() || matches!(character, '_' | '-') {
            start -= character.len_utf8();
        } else {
            break;
        }
    }
    Some((
        call,
        CompletionContext {
            prefix: source[start..offset].to_owned(),
            replace: TextRange::new(start, offset),
        },
    ))
}

fn used_argument_parameters<'a>(
    signature: &'a FunctionSignature,
    call: &'a Call,
) -> BTreeSet<&'a str> {
    let mut used = BTreeSet::new();
    let mut positional_index = 0usize;
    let mut saw_named = false;
    for argument in &call.arguments {
        if let Some(name) = &argument.name {
            saw_named = true;
            used.insert(name.value.as_str());
        } else if !saw_named {
            if let Some(parameter) = signature.parameters.get(positional_index) {
                used.insert(parameter.name.as_str());
            }
            positional_index += 1;
        }
    }
    used
}

fn starts_with_case_insensitive(value: &str, prefix: &str) -> bool {
    value
        .to_lowercase()
        .starts_with(&prefix.trim().to_lowercase())
}

fn relative_module_reference(current: &ModulePath, target: &ModulePath) -> String {
    let current_segments = current.segments();
    let target_segments = target.segments();
    let common = current_segments
        .iter()
        .zip(target_segments)
        .take_while(|(left, right)| left == right)
        .count();
    let up = current_segments.len() - common;
    let mut relative = Vec::new();
    relative.extend(std::iter::repeat_n("super".to_owned(), up));
    relative.extend(target_segments[common..].iter().cloned());
    let relative = if relative.is_empty() {
        "self".into()
    } else {
        relative.join("::")
    };
    let absolute = target.to_string();
    if relative.len() <= absolute.len() {
        relative
    } else {
        absolute
    }
}

fn completion_module_reference(current: &ModulePath, target: &ModulePath, prefix: &str) -> String {
    let prefix = prefix.trim_start();
    if prefix.starts_with("vault") {
        return target.to_string();
    }
    if prefix.starts_with("self") && target.segments().starts_with(current.segments()) {
        let remainder = &target.segments()[current.segments().len()..];
        return if remainder.is_empty() {
            "self".into()
        } else {
            format!("self::{}", remainder.join("::"))
        };
    }
    relative_module_reference(current, target)
}

fn format_signature(name: &str, signature: &FunctionSignature) -> String {
    let trailing = signature.trailing_content.as_deref().and_then(|name| {
        signature
            .parameters
            .iter()
            .find(|parameter| parameter.name == name)
    });
    let parameters = signature
        .parameters
        .iter()
        .filter(|parameter| signature.trailing_content.as_deref() != Some(parameter.name.as_str()))
        .map(|parameter| {
            let default = parameter
                .default
                .as_ref()
                .map(|default| format!(" = {}", format_default(default)))
                .unwrap_or_default();
            format!("{}: {}{default}", parameter.name, parameter.ty)
        })
        .collect::<Vec<_>>()
        .join(", ");
    let call = if parameters.is_empty() && trailing.is_some() {
        format!("#{name}")
    } else {
        format!("#{name}({parameters})")
    };
    let trailing = trailing
        .map(|parameter| format!("[{}: {}]", parameter.name, parameter.ty))
        .unwrap_or_default();
    format!("{call}{trailing} -> {}", signature.result)
}

fn format_default(default: &DefaultValue) -> String {
    match default {
        DefaultValue::None => "none".into(),
        DefaultValue::Bool(value) => value.to_string(),
        DefaultValue::Int(value) => value.to_string(),
        DefaultValue::Float(value) => value.to_string(),
        DefaultValue::String(value) => format!("\"{value}\""),
    }
}

fn collect_document_symbols(
    document: &StructuredDocument,
    revision: Revision,
    file_id: FileId,
    symbols: &mut Vec<DocumentSymbol>,
) {
    for block in &document.blocks {
        let Block::Element(node) = block;
        collect_element_symbols(&node.element, node.range, revision, file_id, symbols);
    }
}

fn collect_element_symbols(
    element: &Element,
    range: TextRange,
    revision: Revision,
    file_id: FileId,
    symbols: &mut Vec<DocumentSymbol>,
) {
    if let Element::Heading { level, body } = element {
        let local_id = u32::try_from(symbols.len()).unwrap_or(u32::MAX);
        symbols.push(DocumentSymbol {
            id: SnapshotNodeId {
                revision,
                file_id,
                local_id,
            },
            name: content_text(body),
            level: *level,
            range,
        });
    }
    for content in element_contents(element) {
        for node in &content.elements {
            collect_element_symbols(&node.element, node.range, revision, file_id, symbols);
        }
    }
}

fn element_contents(element: &Element) -> Vec<&Content> {
    match element {
        Element::Paragraph(body)
        | Element::Strong(body)
        | Element::Emph(body)
        | Element::Strike(body)
        | Element::Insert(body)
        | Element::Spoiler(body)
        | Element::Highlight(body)
        | Element::Underline(body)
        | Element::Keyboard(body)
        | Element::Sample(body)
        | Element::Super(body)
        | Element::Sub(body)
        | Element::Footnote(body)
        | Element::Comment(body)
        | Element::ListItem(body) => vec![body],
        Element::Heading { body, .. }
        | Element::Time { body, .. }
        | Element::Link { body, .. }
        | Element::TaskItem { body, .. }
        | Element::Custom { body, .. } => vec![body],
        Element::Figure { caption, .. } => vec![caption],
        Element::Quote { body, attribution } => {
            let mut contents = vec![body];
            contents.extend(attribution.iter());
            contents
        }
        Element::Callout { title, body, .. } => {
            let mut contents = vec![body];
            contents.extend(title.iter());
            contents
        }
        Element::Details { summary, body, .. } => {
            let mut contents = vec![body];
            contents.extend(summary.iter());
            contents
        }
        Element::TableCell { body, .. } => vec![body],
        Element::Table { caption, cells, .. } => {
            let mut contents: Vec<_> = caption.iter().collect();
            contents.extend(
                cells
                    .iter()
                    .flat_map(|cell| element_contents(&cell.element)),
            );
            contents
        }
        Element::List { items, .. } | Element::Terms { items } | Element::Tasks { items } => items
            .iter()
            .flat_map(|item| element_contents(&item.element))
            .collect(),
        Element::EnumItem { body, .. } => vec![body],
        Element::TermItem { term, description } => vec![term, description],
        Element::UnresolvedCall { trailing, .. } => trailing.iter().collect(),
        _ => Vec::new(),
    }
}

fn content_text(content: &Content) -> String {
    let mut text = String::new();
    for node in &content.elements {
        match &node.element {
            Element::Text(value) => text.push_str(value),
            element => {
                for child in element_contents(element) {
                    text.push_str(&content_text(child));
                }
            }
        }
    }
    text
}

fn changed_semantic_files<T: PartialEq>(
    previous: &[T],
    current: &[T],
    file_id: impl Fn(&T) -> FileId,
) -> Vec<FileId> {
    let files: BTreeSet<_> = previous.iter().chain(current).map(&file_id).collect();
    files
        .into_iter()
        .filter(|candidate| {
            let before: Vec<_> = previous
                .iter()
                .filter(|item| file_id(item) == *candidate)
                .collect();
            let after: Vec<_> = current
                .iter()
                .filter(|item| file_id(item) == *candidate)
                .collect();
            before != after
        })
        .collect()
}

fn changed_diagnostic_files(
    previous: &WorkspaceSnapshot,
    current: &WorkspaceSnapshot,
) -> Vec<FileId> {
    let paths: BTreeSet<_> = previous
        .diagnostics
        .iter()
        .chain(&current.diagnostics)
        .filter_map(|diagnostic| diagnostic.source_path.as_ref())
        .cloned()
        .collect();
    paths
        .into_iter()
        .filter_map(|path| {
            let before: Vec<_> = previous
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.source_path.as_ref() == Some(&path))
                .collect();
            let after: Vec<_> = current
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.source_path.as_ref() == Some(&path))
                .collect();
            (before != after)
                .then(|| current.file_id(&path).or_else(|| previous.file_id(&path)))
                .flatten()
        })
        .collect()
}

fn explicit_ref_target(call: &Call) -> Option<Result<WikiReference, String>> {
    if call.name.value != "ref" || !call.trailing.is_empty() || call.arguments.len() != 1 {
        return None;
    }
    let argument = &call.arguments[0];
    if argument
        .name
        .as_ref()
        .is_some_and(|name| name.value != "target")
    {
        return None;
    }
    let value = string_expression(&argument.expression)?;
    Some(parse_wiki_reference(value))
}

fn string_expression(expression: &Expression) -> Option<&str> {
    match &expression.kind {
        ExpressionKind::String(literal) => Some(&literal.value),
        ExpressionKind::Parenthesized(inner) => string_expression(inner),
        _ => None,
    }
}

/// Finds the nearest ancestor vault marker for a file or directory.
///
/// When a boundary is provided, ancestors above that directory are not considered.
pub fn find_vault_root(path: &Path, boundary: Option<&Path>) -> io::Result<Option<PathBuf>> {
    let path = normalize_discovery_path(path)?;
    let boundary = boundary.map(dunce::canonicalize).transpose()?;
    let mut directory = if path.is_dir() {
        path.as_path()
    } else {
        path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "source path has no parent")
        })?
    };

    loop {
        if boundary
            .as_deref()
            .is_some_and(|boundary| !directory.starts_with(boundary))
        {
            return Ok(None);
        }
        if directory.join(MANIFEST_FILE).is_file() {
            return Ok(Some(directory.to_path_buf()));
        }
        if boundary.as_deref() == Some(directory) {
            return Ok(None);
        }
        let Some(parent) = directory.parent() else {
            return Ok(None);
        };
        directory = parent;
    }
}

/// Discovers all explicitly marked vault roots below a directory.
pub fn discover_vault_roots(root: &Path) -> io::Result<Vec<PathBuf>> {
    let root = dunce::canonicalize(root)?;
    let mut roots = Vec::new();
    discover_vault_roots_in(&root, &mut roots)?;
    roots.sort();
    Ok(roots)
}

/// Resolves a CLI path to one unambiguous explicit or implicit vault root.
pub fn resolve_vault_root(path: &Path) -> io::Result<PathBuf> {
    let path = normalize_discovery_path(path)?;
    if let Some(root) = find_vault_root(&path, None)? {
        return Ok(root);
    }

    let directory = if path.is_dir() {
        path
    } else {
        path.parent()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "source path has no parent")
            })?
            .to_path_buf()
    };
    let roots = discover_vault_roots(&directory)?;
    match roots.as_slice() {
        [] => Ok(directory),
        [root] => Ok(root.clone()),
        roots => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "multiple Notist vaults found below {}; pass one vault explicitly: {}",
                directory.display(),
                roots
                    .iter()
                    .map(|root| root.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

fn discover_vault_roots_in(directory: &Path, roots: &mut Vec<PathBuf>) -> io::Result<()> {
    if directory.join(MANIFEST_FILE).is_file() {
        roots.push(directory.to_path_buf());
    }
    let mut entries: Vec<_> = fs::read_dir(directory)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || matches!(name.as_ref(), "node_modules" | "target") {
            continue;
        }
        discover_vault_roots_in(&entry.path(), roots)?;
    }
    Ok(())
}

fn normalize_discovery_path(path: &Path) -> io::Result<PathBuf> {
    if path.exists() {
        return dunce::canonicalize(path);
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    Ok(dunce::canonicalize(parent)?.join(file_name))
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

fn normalize_document_versions(
    root: &Path,
    versions: DocumentVersions,
) -> io::Result<DocumentVersions> {
    versions
        .into_iter()
        .map(|(path, version)| normalize_source_path(root, &path).map(|path| (path, version)))
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

        let workspace = WorkspaceSnapshot::load(root.path()).unwrap();
        assert!(workspace.diagnostics().is_empty());
        assert_eq!(workspace.modules().count(), 3);
        assert_eq!(workspace.references().len(), 4);
        assert_eq!(
            workspace.references()[0].target_module,
            ModulePath::from_segments(["pages".into()])
        );
    }

    #[test]
    fn indexes_explicit_ref_calls_and_reports_invalid_targets() {
        let root = TempDir::new().unwrap();
        fs::create_dir(root.path().join("pages")).unwrap();
        fs::write(
            root.path().join("README.not"),
            "#ref(\"pages\") #ref(\"missing\") #ref(\"vault::::bad\")",
        )
        .unwrap();
        fs::write(root.path().join("pages/README.not"), "Target").unwrap();

        let workspace = WorkspaceSnapshot::load(root.path()).unwrap();
        assert_eq!(workspace.references().len(), 1);
        assert_eq!(
            workspace.references()[0].target_module,
            ModulePath::from_segments(["pages".into()])
        );
        assert!(workspace.diagnostics().iter().any(|diagnostic| {
            diagnostic.kind == DiagnosticKind::UnresolvedModule
                && diagnostic.message.contains("missing")
        }));
        assert!(
            workspace.diagnostics().iter().any(|diagnostic| {
                diagnostic.kind == DiagnosticKind::InvalidArguments
                    && diagnostic.message.contains("empty segment")
            }),
            "{:?}",
            workspace.diagnostics()
        );
    }

    #[test]
    fn reports_file_and_readme_module_collisions() {
        let root = TempDir::new().unwrap();
        fs::create_dir(root.path().join("pages")).unwrap();
        fs::write(root.path().join("pages.not"), "").unwrap();
        fs::write(root.path().join("pages/README.not"), "").unwrap();

        let workspace = WorkspaceSnapshot::load(root.path()).unwrap();
        assert_eq!(
            workspace.diagnostics()[0].kind,
            DiagnosticKind::DuplicateModule
        );
    }

    #[test]
    fn reports_missing_modules_and_unresolved_labels() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("README.not"), "[[missing]] [[#label]]").unwrap();

        let workspace = WorkspaceSnapshot::load(root.path()).unwrap();
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
                .any(|diagnostic| diagnostic.kind == DiagnosticKind::UnresolvedLabel)
        );
    }

    #[test]
    fn resolves_module_and_local_labels_from_source_annotations() {
        let root = TempDir::new().unwrap();
        fs::create_dir(root.path().join("pages")).unwrap();
        fs::write(
            root.path().join("README.not"),
            "[[pages::guide#intro]] [[#here]] #[Here]@here",
        )
        .unwrap();
        fs::write(
            root.path().join("pages/guide.not"),
            "#heading[Introduction]@intro",
        )
        .unwrap();

        let workspace = WorkspaceSnapshot::load(root.path()).unwrap();
        assert!(
            workspace.diagnostics().is_empty(),
            "{:?}",
            workspace.diagnostics()
        );
        assert_eq!(workspace.labels().len(), 2);
        assert_eq!(workspace.references().len(), 2);
        assert!(
            workspace
                .references()
                .iter()
                .any(|reference| reference.target_label.as_deref() == Some("here"))
        );
        assert!(
            workspace
                .references()
                .iter()
                .any(|reference| reference.target_label.as_deref() == Some("intro"))
        );
        assert!(
            workspace
                .references()
                .iter()
                .all(|reference| reference.target_range.is_some())
        );
    }

    #[test]
    fn reports_duplicate_labels_without_hiding_the_first_definition() {
        let root = TempDir::new().unwrap();
        fs::write(
            root.path().join("README.not"),
            "#[one]@same #[two]@same [[#same]]",
        )
        .unwrap();

        let workspace = WorkspaceSnapshot::load(root.path()).unwrap();
        assert_eq!(workspace.labels().len(), 1);
        assert_eq!(workspace.references().len(), 1);
        assert!(
            workspace
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind == DiagnosticKind::DuplicateLabel)
        );
    }

    #[test]
    fn ignores_directories_without_notist_files_in_their_subtree() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("empty/nested")).unwrap();
        fs::write(root.path().join("empty/nested/readme.txt"), "not a module").unwrap();
        fs::create_dir_all(root.path().join("notes/nested")).unwrap();
        fs::write(root.path().join("notes/nested/page.not"), "").unwrap();

        let workspace = WorkspaceSnapshot::load(root.path()).unwrap();
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

        let workspace = WorkspaceSnapshot::load_with_overlays(root.path(), overlays).unwrap();
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

        let workspace = WorkspaceSnapshot::load_with_overlays(root.path(), overlays).unwrap();

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

    #[test]
    fn snapshots_preserve_the_assigned_revision() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("README.not"), "content").unwrap();

        let workspace = WorkspaceSnapshot::load_with_overlays_at_revision(
            root.path(),
            SourceOverlays::new(),
            42,
        )
        .unwrap();

        assert_eq!(workspace.revision().raw(), 42);
    }

    #[test]
    fn discovers_marked_vaults_from_files_and_parent_directories() {
        let root = TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        let vault = root.path().join("docs");
        fs::create_dir_all(vault.join("nested")).unwrap();
        fs::write(vault.join(MANIFEST_FILE), "").unwrap();
        let source = vault.join("nested/page.not");
        fs::write(&source, "").unwrap();
        let vault = dunce::canonicalize(vault).unwrap();

        assert_eq!(
            find_vault_root(&source, Some(root.path())).unwrap(),
            Some(vault.clone())
        );
        assert_eq!(resolve_vault_root(root.path()).unwrap(), vault);
    }

    #[test]
    fn reports_ambiguous_vault_discovery() {
        let root = TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        for name in ["docs", "notes"] {
            fs::create_dir(root.path().join(name)).unwrap();
            fs::write(root.path().join(name).join(MANIFEST_FILE), "").unwrap();
        }

        let error = resolve_vault_root(root.path()).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("multiple Notist vaults"));
    }

    #[test]
    fn outer_vault_does_not_include_nested_marked_vaults() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join(MANIFEST_FILE), "").unwrap();
        fs::write(root.path().join("README.not"), "outer").unwrap();
        fs::create_dir(root.path().join("nested")).unwrap();
        fs::write(root.path().join("nested").join(MANIFEST_FILE), "").unwrap();
        fs::write(root.path().join("nested/README.not"), "inner").unwrap();

        let workspace = WorkspaceSnapshot::load(root.path()).unwrap();
        let modules: Vec<_> = workspace
            .modules()
            .map(|module| module.logical_path.to_string())
            .collect();

        assert_eq!(modules, ["vault"]);
    }

    #[test]
    fn analyzer_views_isolate_overlays_while_sharing_the_vault_root() {
        let root = TempDir::new().unwrap();
        let source_path = root.path().join("README.not");
        fs::write(&source_path, "disk").unwrap();
        let source_path = dunce::canonicalize(source_path).unwrap();
        let engine = VaultEngine::open(root.path()).unwrap();
        let disk_view = engine.disk_view().unwrap();
        let mut overlays = SourceOverlays::new();
        overlays.insert(source_path.clone(), Arc::from("overlay"));
        let editor_view = engine.view(overlays).unwrap();

        assert_eq!(
            disk_view
                .current()
                .module_for_source(&source_path)
                .unwrap()
                .source
                .as_deref(),
            Some("disk")
        );
        assert_eq!(
            editor_view
                .current()
                .module_for_source(&source_path)
                .unwrap()
                .source
                .as_deref(),
            Some("overlay")
        );
        assert_eq!(disk_view.engine().root(), editor_view.engine().root());
        assert_eq!(disk_view.current().revision(), Revision::INITIAL);
        assert_eq!(editor_view.current().revision(), Revision::INITIAL);
    }

    #[test]
    fn analyzer_view_publishes_only_complete_candidates() {
        let root = TempDir::new().unwrap();
        let source_path = root.path().join("README.not");
        fs::write(&source_path, "first").unwrap();
        let source_path = dunce::canonicalize(source_path).unwrap();
        let engine = VaultEngine::open(root.path()).unwrap();
        let mut view = engine.disk_view().unwrap();
        let original = view.snapshot();

        fs::write(&source_path, "second").unwrap();
        let publication = view.reload_publication().unwrap();
        let updated = publication.snapshot;

        assert_eq!(original.revision(), Revision::INITIAL);
        assert_eq!(updated.revision().raw(), 1);
        assert_eq!(
            publication.delta.changed_files,
            vec![original.file_id(&source_path).unwrap()]
        );
        assert_eq!(
            original
                .module_for_source(&source_path)
                .unwrap()
                .source
                .as_deref(),
            Some("first")
        );
        assert_eq!(
            updated
                .module_for_source(&source_path)
                .unwrap()
                .source
                .as_deref(),
            Some("second")
        );

        fs::remove_dir_all(root.path()).unwrap();
        assert!(view.reload().is_err());
        assert_eq!(view.current().revision().raw(), 1);
        assert_eq!(view.snapshot().revision(), updated.revision());
    }

    #[test]
    fn engine_keeps_file_and_module_identity_stable_across_views_and_revisions() {
        let root = TempDir::new().unwrap();
        let source_path = root.path().join("README.not");
        fs::write(&source_path, "first").unwrap();
        let source_path = dunce::canonicalize(source_path).unwrap();
        let engine = VaultEngine::open(root.path()).unwrap();
        let mut first_view = engine.disk_view().unwrap();
        let second_view = engine.disk_view().unwrap();
        let first_file = first_view.current().file_id(&source_path).unwrap();
        let first_module = first_view.current().module_at(first_file).unwrap().id;

        fs::write(&source_path, "second").unwrap();
        let reloaded = first_view.reload().unwrap();
        let reloaded_file = reloaded.file_id(&source_path).unwrap();
        let reloaded_module = reloaded.module_at(reloaded_file).unwrap().id;
        let second_file = second_view.current().file_id(&source_path).unwrap();
        let second_module = second_view.current().module_at(second_file).unwrap().id;

        assert_eq!(first_file, reloaded_file);
        assert_eq!(first_file, second_file);
        assert_eq!(first_module, reloaded_module);
        assert_eq!(first_module, second_module);
        assert_eq!(
            engine.cached_disk_source(&source_path).as_deref(),
            Some("second")
        );
    }

    #[test]
    fn snapshot_captures_configuration_overlay_versions_and_line_index() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join(MANIFEST_FILE), "language = 1").unwrap();
        let source_path = root.path().join("README.not");
        fs::write(&source_path, "disk").unwrap();
        let source_path = dunce::canonicalize(source_path).unwrap();
        let mut overlays = SourceOverlays::new();
        overlays.insert(source_path.clone(), Arc::from("a😀\n中"));
        let mut versions = DocumentVersions::new();
        versions.insert(source_path.clone(), 17);
        let engine = VaultEngine::open(root.path()).unwrap();
        let view = engine.view_with_versions(overlays, versions).unwrap();
        let snapshot = view.current();
        let file_id = snapshot.file_id(&source_path).unwrap();
        let source = snapshot.source(file_id).unwrap();

        assert_eq!(snapshot.configuration(), Some("language = 1"));
        assert_eq!(source.origin, SourceOrigin::Overlay);
        assert_eq!(source.document_version, Some(17));
        assert_eq!(
            source.line_index.utf16_position(&source.text, 5),
            Some((0, 3))
        );
        assert_eq!(source.line_index.offset_utf16(&source.text, 1, 1), Some(9));

        let unicode = "e\u{301}😀\r\n";
        let unicode_index = LineIndex::new(unicode);
        assert_eq!(
            unicode_index.utf16_position(unicode, "e\u{301}😀".len()),
            Some((0, 4))
        );
        assert_eq!(unicode_index.offset_utf16(unicode, 0, 3), None);
        assert_eq!(
            unicode_index.utf16_position(unicode, unicode.len()),
            Some((1, 0))
        );
    }

    #[test]
    fn snapshot_queries_and_delta_use_captured_semantics() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("README.not"), "[[page#intro]]").unwrap();
        fs::write(root.path().join("page.not"), "#heading[Intro]@intro").unwrap();
        let engine = VaultEngine::open(root.path()).unwrap();
        let mut view = engine.disk_view().unwrap();
        let first = view.snapshot();
        let root_path = dunce::canonicalize(root.path().join("README.not")).unwrap();
        let root_file = first.file_id(&root_path).unwrap();
        let definition = first.definition_at(root_file, 3).unwrap();
        let target = first.module_by_id(definition.module_id).unwrap();

        assert_eq!(target.logical_path.to_string(), "vault::page");
        assert_eq!(definition.annotation.unwrap().name, "intro");
        assert!(first.structured_document(target.id).is_some());
        let hover = first.hover_at(root_file, 3).unwrap().contents;
        assert!(hover.starts_with("vault::page#intro"));
        assert!(hover.contains("page.not"));
        assert!(
            first
                .completions_at(root_file, 2)
                .iter()
                .any(|candidate| candidate.module_id == Some(target.id))
        );
        assert_eq!(
            first.search_context("page").first().unwrap().revision,
            Revision::INITIAL
        );
        let target_file = target.file_id.unwrap();
        assert_eq!(first.document_symbols(target_file)[0].name, "Intro");
        let label = first.label(&target.logical_path, "intro").unwrap();
        let label_target = first
            .reference_target_at(label.file_id, label.range.start)
            .unwrap();
        assert_eq!(
            first
                .references_for_target(&label_target)
                .map(|result| result.value.source_file_id)
                .collect::<Vec<_>>(),
            [root_file]
        );

        fs::write(root.path().join("page.not"), "#heading[Changed]@intro").unwrap();
        fs::write(root.path().join("extra.not"), "extra").unwrap();
        let second = view.reload().unwrap();
        let delta = second.delta_from(&first).unwrap();

        assert_eq!(delta.from_revision, Revision::INITIAL);
        assert_eq!(delta.to_revision.raw(), 1);
        assert_eq!(delta.added_files.len(), 1);
        assert_eq!(delta.changed_files.len(), 1);
        assert!(!delta.changed_modules.is_empty());
    }

    #[test]
    fn rejects_overlay_paths_outside_the_vault() {
        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        fs::write(root.path().join("README.not"), "root").unwrap();
        let outside_path = outside.path().join("outside.not");
        fs::write(&outside_path, "outside").unwrap();
        let mut overlays = SourceOverlays::new();
        overlays.insert(outside_path, Arc::from("overlay"));

        let error = VaultEngine::open(root.path())
            .unwrap()
            .view(overlays)
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("outside the vault"));
    }

    #[test]
    fn explicit_rename_events_preserve_file_identity() {
        let root = TempDir::new().unwrap();
        let old_path = root.path().join("old.not");
        let new_path = root.path().join("new.not");
        fs::write(&old_path, "old").unwrap();
        let engine = VaultEngine::open(root.path()).unwrap();
        let old_snapshot = engine.disk_view().unwrap().snapshot();
        let old_path = dunce::canonicalize(&old_path).unwrap();
        let old_id = old_snapshot.file_id(&old_path).unwrap();

        fs::rename(&old_path, &new_path).unwrap();
        engine.rename_source(&old_path, &new_path).unwrap();
        let new_snapshot = engine.disk_view().unwrap().snapshot();
        let new_path = dunce::canonicalize(&new_path).unwrap();

        assert_eq!(new_snapshot.file_id(&new_path), Some(old_id));
    }

    #[test]
    fn configured_views_capture_independent_manifest_and_function_environments() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join(MANIFEST_FILE), "disk = true").unwrap();
        fs::write(root.path().join("README.not"), "#plugin::note()").unwrap();
        let engine = VaultEngine::open(root.path()).unwrap();
        let disk_view = engine.disk_view().unwrap();
        let mut signatures = SignatureSet::with_builtins();
        signatures.insert(
            "plugin::note",
            notist_model::FunctionSignature {
                parameters: Vec::new(),
                trailing_content: None,
                result: notist_model::Type::Content,
            },
        );
        let configured = engine
            .configured_view(
                SourceOverlays::new(),
                DocumentVersions::new(),
                AnalyzerConfiguration {
                    manifest_override: Some(Arc::from("editor = true")),
                    signatures,
                },
            )
            .unwrap();

        assert_eq!(disk_view.current().configuration(), Some("disk = true"));
        assert_eq!(configured.current().configuration(), Some("editor = true"));
        assert_ne!(
            disk_view.current().function_environment(),
            configured.current().function_environment()
        );
        assert!(
            disk_view
                .current()
                .diagnostics()
                .iter()
                .any(|diagnostic| { diagnostic.kind == DiagnosticKind::UnknownFunction })
        );
        assert!(configured.current().diagnostics().is_empty());
        assert!(
            configured
                .current()
                .delta_from(disk_view.current())
                .is_none()
        );
    }

    #[test]
    fn snapshot_completion_owns_function_and_argument_semantics() {
        let root = TempDir::new().unwrap();
        let source_path = root.path().join("README.not");
        fs::write(&source_path, "#heading()[Title]").unwrap();
        let engine = VaultEngine::open(root.path()).unwrap();
        let mut view = engine.disk_view().unwrap();
        let source_path = dunce::canonicalize(source_path).unwrap();
        let file_id = view.current().file_id(&source_path).unwrap();
        let candidates = view.current().completions_at(file_id, 9);

        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.label.as_str())
                .collect::<Vec<_>>(),
            ["level"]
        );
        assert_eq!(candidates[0].replacement, TextRange::new(9, 9));
        assert_eq!(candidates[0].insert_text, "level=");

        fs::write(&source_path, "#heading(level=raw())").unwrap();
        let snapshot = view.reload().unwrap();
        let offset = snapshot.source(file_id).unwrap().text.find(')').unwrap();
        assert_eq!(
            snapshot
                .completions_at(file_id, offset)
                .iter()
                .map(|candidate| candidate.label.as_str())
                .collect::<Vec<_>>(),
            ["text", "lang"]
        );

        fs::write(&source_path, "#raw(\"code\", )").unwrap();
        let snapshot = view.reload().unwrap();
        assert_eq!(
            snapshot
                .completions_at(file_id, "#raw(\"code\", ".len())
                .iter()
                .map(|candidate| candidate.label.as_str())
                .collect::<Vec<_>>(),
            ["lang"]
        );

        fs::write(&source_path, "#he").unwrap();
        let snapshot = view.reload().unwrap();
        let heading = snapshot
            .completions_at(file_id, 3)
            .into_iter()
            .find(|candidate| candidate.label == "heading")
            .unwrap();
        assert_eq!(heading.kind, CompletionKind::Function);
        assert_eq!(heading.replacement, TextRange::new(1, 3));
        assert_eq!(
            heading.detail,
            "#heading(level: Int = 1)[body: Content] -> Content"
        );
    }

    #[test]
    fn snapshot_completion_uses_natural_module_paths_and_ignores_raw_literals() {
        let root = TempDir::new().unwrap();
        fs::create_dir(root.path().join("notes")).unwrap();
        fs::write(root.path().join("README.not"), "root").unwrap();
        fs::write(root.path().join("notes/today.not"), "[[self::d").unwrap();
        fs::create_dir(root.path().join("notes/today")).unwrap();
        fs::write(root.path().join("notes/today/details.not"), "details").unwrap();
        fs::write(root.path().join("notes/index.not"), "index").unwrap();
        let snapshot = WorkspaceSnapshot::load(root.path()).unwrap();
        let today_path = dunce::canonicalize(root.path().join("notes/today.not")).unwrap();
        let file_id = snapshot.file_id(&today_path).unwrap();
        let candidates = snapshot.completions_at(file_id, "[[self::d".len());

        assert!(candidates.iter().any(|candidate| {
            candidate.label == "self::details" && candidate.kind == CompletionKind::Module
        }));

        let mut overlays = SourceOverlays::new();
        overlays.insert(today_path.clone(), Arc::from("before `#heading` after"));
        let raw = WorkspaceSnapshot::load_with_overlays(root.path(), overlays).unwrap();
        let raw_file_id = raw.file_id(&today_path).unwrap();
        assert!(raw.completions_at(raw_file_id, 10).is_empty());

        let mut overlays = SourceOverlays::new();
        overlays.insert(today_path.clone(), Arc::from("#raw(text=\"#heading\")"));
        let string = WorkspaceSnapshot::load_with_overlays(root.path(), overlays).unwrap();
        let string_file_id = string.file_id(&today_path).unwrap();
        assert!(string.completions_at(string_file_id, 19).is_empty());
    }

    #[test]
    fn snapshot_keeps_user_function_schemas_module_local() {
        let root = TempDir::new().unwrap();
        let first_path = root.path().join("first.not");
        let second_path = root.path().join("second.not");
        let first_source = "#let local(value: String) -> String = value\n#local(\"ok\")\n#lo";
        fs::write(&first_path, first_source).unwrap();
        fs::write(&second_path, "#lo").unwrap();
        let snapshot = WorkspaceSnapshot::load(root.path()).unwrap();
        let first_id = snapshot
            .file_id(&dunce::canonicalize(&first_path).unwrap())
            .unwrap();
        let second_id = snapshot
            .file_id(&dunce::canonicalize(&second_path).unwrap())
            .unwrap();

        let first_candidates = snapshot.completions_at(first_id, first_source.len());
        assert!(first_candidates.iter().any(|candidate| {
            candidate.label == "local" && candidate.detail == "#local(value: String) -> String"
        }));
        assert!(
            !snapshot
                .completions_at(second_id, 3)
                .iter()
                .any(|candidate| candidate.label == "local")
        );
        let call_offset = first_source.find("#local").unwrap() + 2;
        assert_eq!(
            snapshot.hover_at(first_id, call_offset).unwrap().contents,
            "#local(value: String) -> String"
        );
        assert_eq!(
            snapshot.definition_at(first_id, call_offset).unwrap().range,
            Some(TextRange::new(5, 10))
        );
        let parameter_use = first_source[..first_source.find("#local").unwrap()]
            .rfind("value")
            .unwrap();
        assert_eq!(
            snapshot
                .definition_at(first_id, parameter_use)
                .unwrap()
                .range,
            Some(TextRange::new(11, 16))
        );
        let function_locations = snapshot.symbol_locations_at(first_id, call_offset, true);
        assert_eq!(function_locations.len(), 2);
        assert!(function_locations[0].is_definition);
        assert!(!function_locations[1].is_definition);
        assert_eq!(
            snapshot.hover_at(first_id, parameter_use).unwrap().contents,
            "`value: String`"
        );
    }

    #[test]
    fn snapshot_completes_observed_annotation_property_keys() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("README.not");
        let source = "#[one]@first,owner=Alice\n#[two]@second,ow";
        fs::write(&path, source).unwrap();
        let snapshot = WorkspaceSnapshot::load(root.path()).unwrap();
        let file_id = snapshot
            .file_id(&dunce::canonicalize(path).unwrap())
            .unwrap();
        let candidates = snapshot.completions_at(file_id, source.len());

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].kind, CompletionKind::Attribute);
        assert_eq!(candidates[0].label, "owner");
        assert_eq!(candidates[0].insert_text, "owner=");
        assert_eq!(
            candidates[0].replacement,
            TextRange::new(source.len() - 2, source.len())
        );
    }

    #[test]
    fn snapshot_reports_safe_evaluation_diagnostics() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("README.not");
        fs::write(&path, "#heading(level=1 / 0)[Title]").unwrap();
        let snapshot = WorkspaceSnapshot::load(root.path()).unwrap();
        let file_id = snapshot
            .file_id(&dunce::canonicalize(path).unwrap())
            .unwrap();

        assert!(snapshot.diagnostics_for(file_id).any(|diagnostic| {
            diagnostic.kind == DiagnosticKind::Evaluation
                && diagnostic.message == "division by zero"
        }));
    }
}
