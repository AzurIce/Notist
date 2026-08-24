use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use notist_eval::{
    EvalDiagnostic, Evaluator, Function, FunctionContext, FunctionInput, FunctionOutput,
    FunctionRegistry, ShapingRegistry, StreamNode, instances_to_legacy_content,
    legacy_content_to_nodes, shape_flat, structure,
};
use notist_model::{
    Block, Content, DefaultValue, Element, ElementNode, FunctionSignature, ModulePath,
    ModuleReference, StructuredDocument, TextRange, Type, WikiReference,
};
use notist_syntax::{Call, Expression, ExpressionKind, Parse, parse, parse_wiki_reference};

mod check;

pub use check::{
    CheckDiagnostic, LocalSymbolId, ModuleSemanticIndex, SignatureSet, SymbolDefinition,
    SymbolKind, SymbolReference, check_module, check_module_with_prelude, resolve_module_symbols,
};
pub use notist_eval::{AnnotationEntry, Value};

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

/// Computes the semantic-world identity for one snapshot.
///
/// The caller-provided id acts as a view-local salt (zero for disk views, a
/// monotonically allocated value for configured views). The salt is combined
/// with the effective plugin surface, configuration text, and static signature
/// set, so plugin/package changes always produce a new semantic world while
/// separate views over the same surface remain distinguishable.
fn function_environment_for(
    salt: FunctionEnvironmentId,
    configuration: Option<&str>,
    signatures: &SignatureSet,
    plugins: &[notist_plugin_host::LoadedPlugin],
) -> FunctionEnvironmentId {
    let mut parts = Vec::new();
    parts.push(format!("salt:{}", salt.raw()));
    parts.push(format!("config:{}", configuration.unwrap_or_default()));
    let mut signatures = signatures
        .iter()
        .map(|(name, signature)| format!("{name}={signature:?}"))
        .collect::<Vec<_>>();
    signatures.sort();
    parts.extend(signatures);
    for plugin in plugins {
        parts.push(format!(
            "plugin:{}@{} api={}",
            plugin.id, plugin.version, plugin.api_version
        ));
        for function in &plugin.functions {
            parts.push(format!(
                "function:{}:{:?}",
                function.name(),
                function.signature()
            ));
        }
        for schema in &plugin.elements {
            parts.push(format!("schema:{schema:?}"));
        }
        for contribution in &plugin.html_contributions {
            parts.push(format!("html:{contribution:?}"));
        }
    }
    let mut hash = 0xcbf29ce484222325u64;
    for part in parts {
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
    }
    if hash == 0 {
        FunctionEnvironmentId(1)
    } else {
        FunctionEnvironmentId(hash)
    }
}

/// View-local configuration and statically visible function schemas.
#[derive(Clone, Debug)]
pub struct AnalyzerConfiguration {
    pub manifest_override: Option<Arc<str>>,
    pub signatures: SignatureSet,
    /// Runtime function registry used for evaluation. This is the plugin
    /// system's eval-side contribution point.
    pub function_registry: Arc<FunctionRegistry>,
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
        Type::Function => Value::Function(Box::new(notist_eval::FunctionValue {
            signature: notist_eval::FunctionSignature {
                parameters: Vec::new(),
                trailing_content: None,
                result: Type::Inferred,
            },
            implementation: notist_eval::FunctionImplementation::Builtin(name.to_owned()),
            captured: std::collections::HashMap::new(),
        })),
        Type::Optional(_) | Type::Inferred => Value::None,
    }
}

fn safe_evaluation_diagnostics(
    source: &str,
    parse: &notist_syntax::Parse,
    signatures: &SignatureSet,
    seeds: &HashMap<String, Value>,
    function_registry: &FunctionRegistry,
) -> Vec<EvalDiagnostic> {
    let mut registry = function_registry.clone();
    for (name, signature) in signatures.iter() {
        if registry.get(name).is_none() {
            let _ = registry.register(SchemaFunction {
                name: name.to_owned(),
                signature: signature.clone(),
            });
        }
    }
    Evaluator::new(registry)
        .evaluate_parsed_with_bindings(source, parse, seeds.clone())
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
            function_registry: Arc::new(FunctionRegistry::with_builtins()),
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
    /// Resource files living in the module's directory, addressed by file name.
    pub resources: Vec<ResourceFile>,
}

/// A non-source file discovered inside a module's directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceFile {
    /// The file name, which is also its module-local label.
    pub name: String,
    /// Absolute path of the file on disk.
    pub path: PathBuf,
    /// Resolution-level classification of the resource.
    pub kind: ResourceKind,
}

/// The resource classification assigned during resolution (D0004).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceKind {
    Image,
    File,
}

/// Unsaved source texts keyed by their absolute source path.
pub type SourceOverlays = BTreeMap<PathBuf, Arc<str>>;

/// Optional editor document versions keyed by canonical source path.
pub type DocumentVersions = BTreeMap<PathBuf, i64>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticKind {
    DuplicateModule,
    DuplicateLabel,
    InvalidSyntax,
    UnresolvedModule,
    UnresolvedLabel,
    AmbiguousLabel,
    UnknownFunction,
    DuplicateFunction,
    UnresolvedName,
    InvalidFunction,
    InvalidArguments,
    TypeMismatch,
    Evaluation,
    ExternalReferenceUnsupported,
    ImportCycle,
}

/// Diagnostic severity classification (D0009). The full analysis always runs;
/// severity only selects which diagnostics a caller asks to see in detail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
}

impl DiagnosticKind {
    /// Classifies this diagnostic kind (deterministic per kind; D0003-style
    /// warnings such as unreliable heading default ids map to Warning).
    pub const fn severity(self) -> DiagnosticSeverity {
        match self {
            Self::ExternalReferenceUnsupported => DiagnosticSeverity::Info,
            _ => DiagnosticSeverity::Error,
        }
    }

    /// Serialized severity label used by query envelopes.
    pub const fn severity_label(self) -> &'static str {
        match self.severity() {
            DiagnosticSeverity::Error => "error",
            DiagnosticSeverity::Warning => "warning",
            DiagnosticSeverity::Info => "info",
        }
    }

    /// Standard recovery hint surfaced with the diagnostic when available.
    pub fn hint(self) -> Option<&'static str> {
        match self {
            Self::AmbiguousLabel => Some("duplicate heading text: give one heading an explicit id"),
            Self::UnresolvedLabel => {
                Some("check the label spelling, or the labeled scope does not exist in this module")
            }
            Self::UnresolvedModule => {
                Some("check the module path spelling; the module may not exist in this vault")
            }
            _ => None,
        }
    }
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
    pub url: String,
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

/// One static import edge (D0004): an explicit selector list from one
/// module to another, resolved before evaluation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportEdge {
    pub source_module: ModulePath,
    pub source_path: PathBuf,
    pub range: TextRange,
    pub target_module: Option<ModulePath>,
    pub selectors: Vec<(String, Option<String>)>,
}

/// Discriminated target produced by resolving a reference url (D0004): the
/// target kind is a resolution product, not an element field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefTarget {
    Module(ModuleId),
    Scope {
        module: ModuleId,
        id: String,
    },
    Resource {
        module: ModuleId,
        name: String,
        kind: ResourceKind,
    },
    External(String),
    Missing(MissingReason),
}

/// Why a reference target did not resolve (D0004: 不存在/歧义/不支持).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MissingReason {
    Nonexistent,
    Ambiguous,
    Unsupported,
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
    /// Canonical recursively shaped Leaf tree. Clean parses produce this
    /// directly from the Stream pipeline; recovery parses project the legacy
    /// evaluation result.
    pub tree: notist_eval::ElementTree,
    pub diagnostics: Vec<EvalDiagnostic>,
    /// The evaluation annotation table (D0002/D0006): postfix `@...` and
    /// block-prefix `@[...]` attribute sets over absolute source ranges.
    pub annotations: Vec<notist_eval::AnnotationEntry>,
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
    imports: Vec<ImportEdge>,
    /// Imported bindings seeded into each module's evaluation, computed once
    /// per snapshot in dependency order (D0004).
    module_import_seeds: BTreeMap<ModuleId, HashMap<String, Value>>,
    /// Module attributes declared by `@![...]` (D0006), captured once per
    /// snapshot in the same dependency-ordered evaluation pass.
    module_attributes: BTreeMap<ModuleId, Vec<notist_syntax::Attributes>>,
    diagnostics: Vec<Diagnostic>,
    signatures: SignatureSet,
    function_registry: Arc<FunctionRegistry>,
    shaping_registry: Arc<ShapingRegistry>,
    html_contributions: Vec<notist_plugin_host::HtmlContribution>,
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
        let mut function_registry = (*analyzer_configuration.function_registry).clone();
        let mut signatures = analyzer_configuration.signatures.clone();
        // Unified plugin loading: instantiate components and run guest `init`
        // to collect the self-described semantic surface. A failing package
        // degrades to a diagnostic instead of bricking the whole snapshot,
        // so transient states while editing a package stay recoverable.
        let mut plugin_load_diagnostics = Vec::new();
        let loaded_plugins =
            match notist_plugin_host::load_plugins_from_vault(&root, configuration.as_deref()) {
                Ok(plugins) => plugins,
                Err(error) => {
                    plugin_load_diagnostics.push(format!("plugin package failed to load: {error}"));
                    Vec::new()
                }
            };
        notist_plugin_host::register_loaded(&mut function_registry, &loaded_plugins)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, format!("{error:?}")))?;
        let mut shaping_registry = ShapingRegistry::new();
        notist_plugin_host::register_loaded_shaping(&mut shaping_registry, &loaded_plugins);
        let html_contributions = loaded_plugins
            .iter()
            .flat_map(|plugin| plugin.html_contributions.iter().cloned())
            .collect();
        for plugin in &loaded_plugins {
            // Data-only declarations contribute signatures (check/completion)
            // without dispatch entries; computed ones also register functions.
            for (name, signature) in &plugin.signatures {
                signatures.insert(name.as_str(), signature.clone());
            }
            for function in &plugin.functions {
                if let Some((package, element)) = function.name().split_once("::")
                    && let Some(alias) = notist_plugin_host::plugin_legacy_alias(package, element)
                {
                    signatures.insert(&alias, function.signature());
                }
            }
        }
        let function_environment = if function_environment == FunctionEnvironmentId::BUILTINS
            && configuration.is_none()
            && loaded_plugins.is_empty()
        {
            FunctionEnvironmentId::BUILTINS
        } else {
            function_environment_for(
                function_environment,
                configuration.as_deref(),
                &signatures,
                &loaded_plugins,
            )
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
            imports: Vec::new(),
            module_import_seeds: BTreeMap::new(),
            module_attributes: BTreeMap::new(),
            diagnostics: Vec::new(),
            signatures,
            function_registry: Arc::new(function_registry),
            shaping_registry: Arc::new(shaping_registry),
            html_contributions,
            module_signatures: BTreeMap::new(),
            module_semantics: BTreeMap::new(),
            attribute_keys: BTreeSet::new(),
            function_environment,
            view_id,
            revision,
        };
        for message in plugin_load_diagnostics {
            workspace.diagnostics.push(Diagnostic {
                kind: DiagnosticKind::InvalidFunction,
                message,
                source_path: Some(root.join(MANIFEST_FILE)),
                range: None,
            });
        }
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

    /// Returns the shaping registry captured by this snapshot, including
    /// package-contributed schemas. Core fallbacks are resolved by the
    /// registry itself, so callers do not need a second lookup.
    pub fn shaping_registry(&self) -> &ShapingRegistry {
        &self.shaping_registry
    }

    /// Returns manifest-declared HTML renderer contributions captured by this
    /// snapshot. Projection hosts use this list to build their renderer
    /// registries without re-reading package directories.
    pub fn html_contributions(&self) -> &[notist_plugin_host::HtmlContribution] {
        &self.html_contributions
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

    /// Returns the static import edges of this snapshot (D0004).
    pub fn imports(&self) -> &[ImportEdge] {
        &self.imports
    }

    /// Returns a module's root bindings — its own `let` bindings plus its
    /// imported names (D0004 ModuleResult.bindings).
    pub fn module_bindings(&self, module_id: ModuleId) -> Option<&HashMap<String, Value>> {
        self.module_import_seeds.get(&module_id)
    }

    /// Returns a module's `@![...]` module attributes (D0006), published as
    /// module metadata in source order.
    pub fn module_attributes(&self, module_id: ModuleId) -> &[notist_syntax::Attributes] {
        self.module_attributes
            .get(&module_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
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

    /// Resolves a reference url written in a source module to its discriminated
    /// target (D0004 RefTarget). External urls are syntactically legal (R10)
    /// and classify as `External`; everything unresolved surfaces as `Missing`
    /// with a reason instead of being swallowed.
    pub fn resolve_reference(&self, source_module: &ModulePath, url: &str) -> RefTarget {
        let Ok(reference) = parse_wiki_reference(url) else {
            return RefTarget::Missing(MissingReason::Unsupported);
        };
        if let ModuleReference::External(external) = reference.module {
            return RefTarget::External(external);
        }
        let Some(target) = reference.module.resolve_from(source_module) else {
            return RefTarget::Missing(MissingReason::Nonexistent);
        };
        let Some(module) = self.modules.get(&target) else {
            return RefTarget::Missing(MissingReason::Nonexistent);
        };
        match reference.label {
            None => RefTarget::Module(module.id),
            Some(label) => self.resolve_module_label(&target, &label),
        }
    }

    /// Resolves a `module#label` selector's label part against one module:
    /// explicit scope ids first, then heading default ids (exact heading text),
    /// then resource file names (D0004). Duplicate heading texts are ambiguous.
    pub fn resolve_module_label(&self, module_path: &ModulePath, label: &str) -> RefTarget {
        let Some(module) = self.modules.get(module_path) else {
            return RefTarget::Missing(MissingReason::Nonexistent);
        };
        if let Some(definition) = self
            .labels
            .iter()
            .find(|definition| definition.module == *module_path && definition.name == label)
        {
            return RefTarget::Scope {
                module: module.id,
                id: definition.name.clone(),
            };
        }
        let seeds = self
            .module_import_seeds
            .get(&module.id)
            .cloned()
            .unwrap_or_default();
        let headings = heading_default_ids(module, &seeds);
        let matches: Vec<_> = headings.iter().filter(|(text, _)| text == label).collect();
        match matches.len() {
            1 => RefTarget::Scope {
                module: module.id,
                id: label.to_owned(),
            },
            0 => match module
                .resources
                .iter()
                .find(|resource| resource.name == label)
            {
                Some(resource) => RefTarget::Resource {
                    module: module.id,
                    name: resource.name.clone(),
                    kind: resource.kind,
                },
                None => RefTarget::Missing(MissingReason::Nonexistent),
            },
            _ => RefTarget::Missing(MissingReason::Ambiguous),
        }
    }

    /// Returns the heading default ids (heading plain text plus source
    /// range) of a source-backed module, in document order (D0003).
    pub fn module_heading_default_ids(&self, module_path: &ModulePath) -> Vec<(String, TextRange)> {
        let Some(module) = self.module(module_path) else {
            return Vec::new();
        };
        let seeds = self
            .module_import_seeds
            .get(&module.id)
            .cloned()
            .unwrap_or_default();
        heading_default_ids(module, &seeds)
    }

    /// Returns the source range covering a resolved scope label: the explicit
    /// label's scope range, or the first heading default-id match range.
    pub fn label_scope_range(&self, module: &ModulePath, label: &str) -> Option<TextRange> {
        if let Some(definition) = self.label(module, label) {
            return Some(definition.scope_range);
        }
        let module = self.module(module)?;
        let seeds = self
            .module_import_seeds
            .get(&module.id)
            .cloned()
            .unwrap_or_default();
        let headings = heading_default_ids(module, &seeds);
        headings
            .iter()
            .find(|(text, _)| text == label)
            .map(|(_, range)| *range)
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
        let mut contains_module_content = false;
        let mut resources = Vec::new();

        for entry in entries {
            let path = entry.path();
            let file_type = entry.file_type()?;
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();
            if file_type.is_dir() {
                if file_name.starts_with('.') {
                    continue;
                }
                // Build caches and dependency trees are not project content (D0004).
                if file_name == "target" || file_name == "node_modules" {
                    continue;
                }
                if path != self.root && path.join(MANIFEST_FILE).is_file() {
                    continue;
                }
                let child = module_path.child([file_name.into_owned()]);
                if self.scan_directory(engine, &path, &child, overlays, document_versions)? {
                    self.insert_virtual_module(engine, child);
                    contains_module_content = true;
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
                contains_module_content = true;
            } else if file_type.is_file()
                && !file_name.starts_with('.')
                && file_name != MANIFEST_FILE
            {
                // A plain file is a resource of this directory's module; a
                // resource-only directory still forms a virtual module (D0004).
                resources.push(ResourceFile {
                    name: file_name.into_owned(),
                    kind: resource_kind(&path),
                    path,
                });
                contains_module_content = true;
            }
        }
        if !resources.is_empty() {
            self.insert_virtual_module(engine, module_path.clone());
            self.modules
                .get_mut(module_path)
                .expect("the directory module was just inserted")
                .resources = resources;
        }
        Ok(contains_module_content)
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
            resources: Vec::new(),
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
            resources: Vec::new(),
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
            // An explicit scope id has a source range of its own.  For a
            // heading default id (or a resource), `self.label()` returns
            // `None`; use the already-resolved reference range so definition
            // jumps to the heading instead of only to the containing file.
            range: label.map(|label| label.range).or(reference.target_range),
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
        // D0004: evaluation runs with the imported bindings seeded by the
        // snapshot build (idempotent for a module's own bindings).
        let seeds = self
            .module_import_seeds
            .get(&module_id)
            .cloned()
            .unwrap_or_default();
        // Snapshot builds are schema-only. Materialize runtime Wasm plugins on
        // first actual evaluation so static analysis never starts guest code.
        let mut runtime_registry = (*self.function_registry).clone();
        if let Some(plugins) = self.configuration.as_deref().and_then(|configuration| {
            notist_plugin_host::load_plugins_from_vault(&self.root, Some(configuration)).ok()
        }) {
            for plugin in &plugins {
                for function in &plugin.functions {
                    runtime_registry.unregister(function.name());
                    if let Some((package, element)) = function.name().split_once("::")
                        && let Some(alias) =
                            notist_plugin_host::plugin_legacy_alias(package, element)
                    {
                        runtime_registry.unregister(&alias);
                    }
                }
            }
            let _ = notist_plugin_host::register_loaded(&mut runtime_registry, &plugins);
        }
        let evaluator = Evaluator::new(runtime_registry);
        let parse = module.parse.as_ref()?;
        // Prefer the new Stream + Leaf reduction engine for clean parses. The
        // legacy evaluator remains the recovery path for syntax errors and
        // reduction failures so diagnostics still surface unresolved calls.
        let mut tree = None;
        let mut structured = None;
        if parse.errors.is_empty() {
            let stream = evaluator.evaluate_parsed_stream_with_bindings(
                source,
                parse,
                seeds.clone(),
                &self.shaping_registry,
            );
            if !stream.reduction_failed {
                let leaves = stream
                    .reduced
                    .nodes
                    .iter()
                    .filter_map(|node| match node {
                        StreamNode::Leaf(leaf) => Some(leaf.clone()),
                        StreamNode::Call(_) => None,
                    })
                    .collect::<Vec<_>>();
                if let Some(content) = instances_to_legacy_content(&leaves) {
                    tree = Some(stream.tree.clone());
                    structured = Some(structure(notist_eval::Evaluation {
                        content,
                        diagnostics: stream.diagnostics,
                        bindings: stream.bindings,
                        annotations: stream.annotations,
                        module_attributes: stream.module_attributes,
                    }));
                }
            }
        }
        let structured = structured.unwrap_or_else(|| {
            let evaluation = evaluator.evaluate_parsed_with_bindings(source, parse, seeds);
            tree = Some(shape_flat(&legacy_content_to_nodes(&evaluation.content)));
            structure(evaluation)
        });
        Some(StructuredModule {
            revision: self.revision,
            module_id,
            function_environment: self.function_environment,
            document: structured.document,
            tree: tree.unwrap_or_default(),
            diagnostics: structured.diagnostics,
            annotations: structured.annotations,
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
        // Heading default ids are stable document identities (D0009) and enter
        // the workspace symbol index; explicit ids stay authoritative.
        for module in self.modules() {
            let Some(file_id) = module.file_id else {
                continue;
            };
            let explicit: std::collections::HashSet<&str> = self
                .labels
                .iter()
                .filter(|label| label.id.module_id == module.id)
                .map(|label| label.name.as_str())
                .collect();
            for (name, range) in self.module_heading_default_ids(&module.logical_path) {
                if explicit.contains(name.as_str()) || !name.to_lowercase().contains(&query) {
                    continue;
                }
                symbols.push(ModuleSymbol {
                    revision: self.revision,
                    module_id: module.id,
                    file_id,
                    name,
                    kind: WorkspaceSymbolKind::Annotation,
                    range,
                    annotation: None,
                });
            }
        }
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
            if context.kind == CompletionContextKind::Label {
                // `#id` completion inside a wiki reference: explicit scope ids
                // first, then heading default ids of the target module (D0009).
                let target = context
                    .label_target
                    .as_deref()
                    .and_then(|part| parse_wiki_reference(part).ok())
                    .and_then(|reference| reference.module.resolve_from(&current.logical_path));
                let Some(target) = target else {
                    return Vec::new();
                };
                let mut candidates: Vec<_> = self
                    .labels
                    .iter()
                    .filter(|label| label.module == target)
                    .filter(|label| starts_with_case_insensitive(&label.name, &context.prefix))
                    .map(|label| CompletionCandidate {
                        revision: self.revision,
                        kind: CompletionKind::Attribute,
                        label: label.name.clone(),
                        detail: "Scope id".into(),
                        documentation: Some("Explicit scope id in this module.".into()),
                        replacement: context.replace,
                        insert_text: label.name.clone(),
                        module_id: Some(label.id.module_id),
                    })
                    .collect();
                for (name, _) in self.module_heading_default_ids(&target) {
                    if starts_with_case_insensitive(&name, &context.prefix)
                        && !candidates.iter().any(|candidate| candidate.label == name)
                    {
                        candidates.push(CompletionCandidate {
                            revision: self.revision,
                            kind: CompletionKind::Attribute,
                            label: name.clone(),
                            detail: "Heading default id".into(),
                            documentation: Some("Default id derived from the heading text.".into()),
                            replacement: context.replace,
                            insert_text: name,
                            module_id: self.modules.get(&target).map(|module| module.id),
                        });
                    }
                }
                candidates.sort_by(|left, right| left.label.cmp(&right.label));
                return candidates;
            }
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
        let mut imports = Vec::new();
        let signatures = self.signatures.clone();
        // Heading default ids are extracted from the evaluated document, so the
        // index is built lazily for reference target modules only and cached per
        // module; ambiguity diagnostics fire once per (module, heading text).
        let mut heading_indexes: BTreeMap<ModuleId, Vec<(String, TextRange)>> = BTreeMap::new();
        let mut reported_ambiguous: BTreeSet<(ModuleId, String)> = BTreeSet::new();

        // D0004: import edges enter the import graph before evaluation, with
        // explicit selectors and no wildcard.
        for module in self.modules.values() {
            let (Some(source_path), Some(parse)) = (&module.source_path, &module.parse) else {
                continue;
            };
            for expression in parse.imports() {
                let ExpressionKind::Import {
                    module: module_ref,
                    selectors,
                } = &expression.kind
                else {
                    continue;
                };
                let selectors = selectors
                    .iter()
                    .map(|selector| {
                        (
                            selector.name.clone(),
                            selector.alias.as_ref().map(|alias| alias.value.clone()),
                        )
                    })
                    .collect();
                let target = module_ref.resolve_from(&module.logical_path);
                match &target {
                    None => diagnostics.push(Diagnostic {
                        kind: DiagnosticKind::UnresolvedModule,
                        message: "import path escapes above `vault`".into(),
                        source_path: Some(source_path.clone()),
                        range: Some(expression.range),
                    }),
                    Some(target) if !self.modules.contains_key(target) => {
                        diagnostics.push(Diagnostic {
                            kind: DiagnosticKind::UnresolvedModule,
                            message: format!("imported module `{target}` was not found"),
                            source_path: Some(source_path.clone()),
                            range: Some(expression.range),
                        })
                    }
                    Some(_) => {}
                }
                imports.push(ImportEdge {
                    source_module: module.logical_path.clone(),
                    source_path: source_path.clone(),
                    range: expression.range,
                    target_module: target,
                    selectors,
                });
            }
        }

        // D0004: the import graph resolves before evaluation; each module
        // evaluates at most once per snapshot in dependency order, and import
        // cycles (already diagnosed) break with empty bindings.
        let evaluator = Evaluator::default();
        let mut module_import_seeds: BTreeMap<ModuleId, HashMap<String, Value>> = BTreeMap::new();
        let mut module_attributes: BTreeMap<ModuleId, Vec<notist_syntax::Attributes>> =
            BTreeMap::new();
        let mut visiting: BTreeSet<ModuleId> = BTreeSet::new();
        {
            fn module_bindings(
                module_id: ModuleId,
                modules: &BTreeMap<ModulePath, Module>,
                imports: &[ImportEdge],
                evaluator: &Evaluator,
                cache: &mut BTreeMap<ModuleId, HashMap<String, Value>>,
                attributes: &mut BTreeMap<ModuleId, Vec<notist_syntax::Attributes>>,
                visiting: &mut BTreeSet<ModuleId>,
            ) -> HashMap<String, Value> {
                if let Some(bindings) = cache.get(&module_id) {
                    return bindings.clone();
                }
                if !visiting.insert(module_id) {
                    // Cycle: already diagnosed; break with empty bindings.
                    return HashMap::new();
                }
                let module = modules.values().find(|module| module.id == module_id);
                let mut seeds = HashMap::new();
                let Some(module) = module else {
                    visiting.remove(&module_id);
                    cache.insert(module_id, HashMap::new());
                    return HashMap::new();
                };
                for edge in imports
                    .iter()
                    .filter(|edge| edge.source_module == module.logical_path)
                {
                    let Some(target_path) = &edge.target_module else {
                        continue;
                    };
                    let Some(target) = modules.get(target_path) else {
                        continue;
                    };
                    let target_bindings = module_bindings(
                        target.id, modules, imports, evaluator, cache, attributes, visiting,
                    );
                    for (name, alias) in &edge.selectors {
                        if let Some(value) = target_bindings.get(name) {
                            seeds.insert(
                                alias.clone().unwrap_or_else(|| name.clone()),
                                value.clone(),
                            );
                        }
                    }
                }
                let bindings = match (module.source.as_deref(), module.parse.as_ref()) {
                    (Some(source), Some(parse)) => {
                        let evaluation =
                            evaluator.evaluate_parsed_with_bindings(source, parse, seeds);
                        attributes.insert(module_id, evaluation.module_attributes);
                        evaluation.bindings
                    }
                    _ => seeds,
                };
                visiting.remove(&module_id);
                cache.insert(module_id, bindings.clone());
                bindings
            }
            let module_ids: Vec<ModuleId> = self.modules.values().map(|module| module.id).collect();
            for module_id in &module_ids {
                module_bindings(
                    *module_id,
                    &self.modules,
                    &imports,
                    &evaluator,
                    &mut module_import_seeds,
                    &mut module_attributes,
                    &mut visiting,
                );
            }
        }
        self.module_attributes = module_attributes;

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
                if let ModuleReference::External(url) = &reference.module {
                    diagnostics.push(Diagnostic {
                        kind: DiagnosticKind::ExternalReferenceUnsupported,
                        message: format!(
                            "external reference `{url}` is not supported in v1; it renders as unresolved visible text"
                        ),
                        source_path: Some(source_path.clone()),
                        range: Some(range),
                    });
                    continue;
                }
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

                let mut target_range = None;
                let mut unresolved = false;
                if let Some(label) = &reference.label {
                    if let Some(definition) = label_indexes
                        .get(&(target.clone(), label.clone()))
                        .map(|index| &labels[*index])
                    {
                        // Explicit scope ids always win over heading default ids.
                        target_range = Some(definition.range);
                    } else {
                        let headings =
                            heading_indexes.entry(target_module.id).or_insert_with(|| {
                                let seeds = self
                                    .module_import_seeds
                                    .get(&target_module.id)
                                    .cloned()
                                    .unwrap_or_default();
                                heading_default_ids(target_module, &seeds)
                            });
                        if let Some(position) = headings.iter().position(|(text, _)| text == label)
                        {
                            // Heading default id: exact match on the evaluated
                            // heading plain text; the first occurrence wins.
                            target_range = Some(headings[position].1);
                            if reported_ambiguous.insert((target_module.id, label.clone())) {
                                for (_, duplicate_range) in headings[position + 1..]
                                    .iter()
                                    .filter(|(text, _)| text == label)
                                {
                                    diagnostics.push(Diagnostic {
                                        kind: DiagnosticKind::AmbiguousLabel,
                                        message: format!(
                                            "ambiguous label `{label}` in module `{target}`: multiple headings share this text; add an explicit `@id` to disambiguate"
                                        ),
                                        source_path: target_module.source_path.clone(),
                                        range: Some(*duplicate_range),
                                    });
                                }
                            }
                        } else if target_module
                            .resources
                            .iter()
                            .any(|resource| &resource.name == label)
                        {
                            // Resource files resolve by exact file name and carry
                            // no source range.
                        } else {
                            unresolved = true;
                        }
                    }
                }
                if unresolved {
                    let label = reference
                        .label
                        .as_ref()
                        .expect("only labeled references can be unresolved");
                    diagnostics.push(Diagnostic {
                        kind: DiagnosticKind::UnresolvedLabel,
                        message: format!("unresolved label `{label}` in module `{target}`"),
                        source_path: Some(source_path.clone()),
                        range: Some(range),
                    });
                    continue;
                }

                let url = reference.label.as_ref().map_or_else(
                    || reference.module.to_string(),
                    |label| format!("{}#{label}", reference.module),
                );
                references.push(ResolvedReference {
                    source_file_id: file_id,
                    source_module_id: module.id,
                    source_module: module.logical_path.clone(),
                    source_path: source_path.clone(),
                    range,
                    url,
                    target_module_id: target_module.id,
                    target_module: target,
                    target_label: reference.label,
                    target_range,
                });
            }
            // D0004: imported names are visible in the static check as
            // unchecked bindings (the target module's types arrive with the
            // import evaluation).
            let prelude: HashMap<String, Type> = imports
                .iter()
                .filter(|edge| edge.source_module == module.logical_path)
                .flat_map(|edge| {
                    edge.selectors.iter().map(|(name, alias)| {
                        (
                            alias.clone().unwrap_or_else(|| name.clone()),
                            Type::Inferred,
                        )
                    })
                })
                .collect();
            let checks = check_module_with_prelude(parse, &signatures, prelude);
            let seeds = module_import_seeds
                .get(&module.id)
                .cloned()
                .unwrap_or_default();
            let runtime = safe_evaluation_diagnostics(
                source,
                parse,
                &signatures,
                &seeds,
                &self.function_registry,
            );
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
        // D0004: import cycles are always diagnostics, independent of
        // evaluation order.
        {
            let mut adjacent: HashMap<ModulePath, Vec<ModulePath>> = HashMap::new();
            for edge in &imports {
                if let Some(target) = &edge.target_module {
                    adjacent
                        .entry(edge.source_module.clone())
                        .or_default()
                        .push(target.clone());
                }
            }
            let mut visited = BTreeSet::new();
            let mut stack = Vec::new();
            let mut reported = BTreeSet::new();
            fn visit(
                node: &ModulePath,
                adjacent: &HashMap<ModulePath, Vec<ModulePath>>,
                visited: &mut BTreeSet<ModulePath>,
                stack: &mut Vec<ModulePath>,
                reported: &mut BTreeSet<(ModulePath, ModulePath)>,
                diagnostics: &mut Vec<Diagnostic>,
                imports: &[ImportEdge],
            ) {
                if !visited.insert(node.clone()) {
                    return;
                }
                stack.push(node.clone());
                if let Some(targets) = adjacent.get(node) {
                    for target in targets {
                        if stack.contains(target) {
                            let key = (node.clone(), target.clone());
                            if reported.insert(key.clone())
                                && let Some(edge) = imports.iter().find(|edge| {
                                    edge.source_module == key.0
                                        && edge.target_module.as_ref() == Some(&key.1)
                                })
                            {
                                diagnostics.push(Diagnostic {
                                    kind: DiagnosticKind::ImportCycle,
                                    message: format!(
                                        "import cycle detected: `{}` imports `{}`, which transitively imports back",
                                        key.0, key.1
                                    ),
                                    source_path: Some(edge.source_path.clone()),
                                    range: Some(edge.range),
                                });
                            }
                        } else {
                            visit(
                                target,
                                adjacent,
                                visited,
                                stack,
                                reported,
                                diagnostics,
                                imports,
                            );
                        }
                    }
                }
                stack.pop();
            }
            let modules: Vec<ModulePath> = self.modules.keys().cloned().collect();
            for node in &modules {
                visit(
                    node,
                    &adjacent,
                    &mut visited,
                    &mut stack,
                    &mut reported,
                    &mut diagnostics,
                    &imports,
                );
            }
        }

        self.labels = labels;
        self.references = references;
        self.imports = imports;
        self.module_import_seeds = module_import_seeds;
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
        // A disk reload that observes no content change must not advance the
        // revision. Debounced filesystem watchers fire on read-close events too,
        // so an unconditional revision bump turns the preview rebuild loop into a
        // self-exciting storm: every reload produces a "new" revision, the
        // preview worker rebuilds, the browser reloads, and the reload re-touches
        // the files that feed the watcher. Keep the previous snapshot and report
        // an empty delta when the source set and configuration are unchanged.
        if same_source_content(&previous, &candidate) {
            return Ok(SnapshotPublication {
                snapshot: previous.clone(),
                delta: empty_delta(previous.revision),
            });
        }
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

/// Reports whether two snapshots carry the same author-authored source set and
/// configuration, so that a no-op reload can keep its revision stable.
///
/// Text, origin, and document version are all compared so that a session view
/// re-publishing identical text under a newer LSP document version still counts
/// as a change (and advances the revision), while a disk watcher reload of
/// untouched files does not.
fn same_source_content(a: &WorkspaceSnapshot, b: &WorkspaceSnapshot) -> bool {
    if a.configuration != b.configuration {
        return false;
    }
    // Plugin package changes alter the semantic world even though they may not
    // touch any `.not` source file; the function-environment fingerprint
    // includes package versions, schemas, render contributions, and grants.
    if a.function_environment != b.function_environment {
        return false;
    }
    if a.sources.len() != b.sources.len() {
        return false;
    }
    a.source_ids.iter().all(|(path, a_id)| {
        let Some(b_id) = b.source_ids.get(path) else {
            return false;
        };
        match (a.sources.get(a_id), b.sources.get(b_id)) {
            (Some(a_source), Some(b_source)) => {
                a_source.text == b_source.text
                    && a_source.origin == b_source.origin
                    && a_source.document_version == b_source.document_version
            }
            _ => false,
        }
    })
}

/// An empty semantic delta for a publication that changed nothing.
fn empty_delta(revision: Revision) -> WorkspaceDelta {
    WorkspaceDelta {
        from_revision: revision,
        to_revision: revision,
        added_files: Vec::new(),
        changed_files: Vec::new(),
        removed_files: Vec::new(),
        added_modules: Vec::new(),
        changed_modules: Vec::new(),
        removed_modules: Vec::new(),
        changed_references: Vec::new(),
        changed_diagnostics: Vec::new(),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompletionContextKind {
    Module,
    Label,
}

struct CompletionContext {
    prefix: String,
    replace: TextRange,
    kind: CompletionContextKind,
    /// For `Label` contexts: the module part before `#` in the wiki link.
    label_target: Option<String>,
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
                kind: CompletionContextKind::Module,
                label_target: None,
            });
        }
        let label_start = module_end + 1;
        if label_start <= offset && offset <= content_end {
            return Some(CompletionContext {
                prefix: source[label_start..offset].to_owned(),
                replace: TextRange::new(label_start, content_end),
                kind: CompletionContextKind::Label,
                label_target: Some(source[start..module_end].to_owned()),
            });
        }
    }
    let before = source.get(..offset)?;
    let start = before.rfind("[[")? + 2;
    if before[start..].contains("]]") || before[start..].contains('\n') {
        return None;
    }
    if let Some(hash) = before[start..].find('#') {
        let hash = start + hash;
        return Some(CompletionContext {
            prefix: source[hash + 1..offset].to_owned(),
            replace: TextRange::new(hash + 1, offset),
            kind: CompletionContextKind::Label,
            label_target: Some(source[start..hash].to_owned()),
        });
    }
    Some(CompletionContext {
        prefix: source[start..offset].to_owned(),
        replace: TextRange::new(start, offset),
        kind: CompletionContextKind::Module,
        label_target: None,
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
            kind: CompletionContextKind::Module,
            label_target: None,
        });
    }
    let before = source.get(..offset)?;
    let hash = before.rfind('#')?;
    let prefix = &source[hash + 1..offset];
    if prefix.is_empty() || prefix == "[" {
        return (prefix != "[").then(|| CompletionContext {
            prefix: String::new(),
            replace: TextRange::new(hash + 1, offset),
            kind: CompletionContextKind::Module,
            label_target: None,
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
        kind: CompletionContextKind::Module,
        label_target: None,
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
        kind: CompletionContextKind::Module,
        label_target: None,
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
            kind: CompletionContextKind::Module,
            label_target: None,
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
    fn walk(block: &Block, revision: Revision, file_id: FileId, symbols: &mut Vec<DocumentSymbol>) {
        match block {
            Block::Element(node) => {
                collect_element_symbols(&node.element, node.range, revision, file_id, symbols);
            }
            Block::Section { heading, body, .. } => {
                collect_element_symbols(
                    &heading.element,
                    heading.range,
                    revision,
                    file_id,
                    symbols,
                );
                for child in body {
                    walk(child, revision, file_id, symbols);
                }
            }
        }
    }
    for block in &document.blocks {
        walk(block, revision, file_id, symbols);
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
        | Element::Underline(body)
        | Element::ListItem(body)
        | Element::EnumItem { body, .. } => vec![body],
        Element::Heading { body, .. } | Element::Custom { body, .. } => vec![body],
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
        Element::List { items, .. } => items
            .iter()
            .flat_map(|item| element_contents(&item.element))
            .collect(),
        Element::TableCell { body, .. } => vec![body],
        Element::Table { cells, .. } => cells
            .iter()
            .flat_map(|cell| element_contents(&cell.element))
            .collect(),
        Element::Figure {
            body,
            supplement,
            caption,
            ..
        } => {
            let mut contents = vec![body];
            contents.extend(supplement.iter());
            contents.extend(caption.iter());
            contents
        }
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

/// Classifies a resource file by its extension, case-insensitively.
fn resource_kind(path: &Path) -> ResourceKind {
    const IMAGE_EXTENSIONS: [&str; 10] = [
        "png", "apng", "gif", "jpg", "jpeg", "webp", "svg", "avif", "ico", "bmp",
    ];
    let is_image = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|image| extension.eq_ignore_ascii_case(image))
        });
    if is_image {
        ResourceKind::Image
    } else {
        ResourceKind::File
    }
}

/// Extracts the heading default ids (heading plain text) of a source-backed
/// module from its evaluated document, in document order.
fn heading_default_ids(
    module: &Module,
    seeds: &HashMap<String, Value>,
) -> Vec<(String, TextRange)> {
    let Some(source) = module.source.as_deref() else {
        return Vec::new();
    };
    let structured = structure(
        Evaluator::default().evaluate_parsed_with_bindings(
            source,
            module
                .parse
                .as_ref()
                .expect("source-backed modules have parses"),
            seeds.clone(),
        ),
    );
    let mut headings = Vec::new();
    fn walk(block: &Block, headings: &mut Vec<(String, TextRange)>) {
        match block {
            Block::Element(node) => collect_heading_default_ids(node, headings),
            Block::Section { heading, body, .. } => {
                collect_heading_default_ids(heading, headings);
                for child in body {
                    walk(child, headings);
                }
            }
        }
    }
    for block in &structured.document.blocks {
        walk(block, &mut headings);
    }
    headings
}

/// Collects heading texts in render (depth-first) order, mirroring the anchor
/// planning walk of the HTML renderer.
fn collect_heading_default_ids(node: &ElementNode, output: &mut Vec<(String, TextRange)>) {
    if let Element::Heading { body, .. } = &node.element {
        output.push((content_plain_text(body), node.range));
    }
    collect_heading_default_ids_in_children(&node.element, output);
}

fn collect_heading_default_ids_in_content(
    content: &Content,
    output: &mut Vec<(String, TextRange)>,
) {
    for node in &content.elements {
        collect_heading_default_ids(node, output);
    }
}

fn collect_heading_default_ids_in_children(
    element: &Element,
    output: &mut Vec<(String, TextRange)>,
) {
    match element {
        Element::Paragraph(body)
        | Element::Strong(body)
        | Element::Emph(body)
        | Element::Strike(body)
        | Element::Underline(body)
        | Element::Heading { body, .. }
        | Element::ListItem(body)
        | Element::EnumItem { body, .. }
        | Element::Custom { body, .. } => collect_heading_default_ids_in_content(body, output),
        Element::List { items, .. } => {
            for item in items {
                collect_heading_default_ids(item, output);
            }
        }
        Element::TableCell { body, .. } => {
            collect_heading_default_ids_in_content(body, output);
        }
        Element::Table { cells, .. } => {
            for cell in cells {
                collect_heading_default_ids(cell, output);
            }
        }
        Element::Figure {
            body,
            supplement,
            caption,
            ..
        } => {
            collect_heading_default_ids_in_content(body, output);
            if let Some(supplement) = supplement {
                collect_heading_default_ids_in_content(supplement, output);
            }
            if let Some(caption) = caption {
                collect_heading_default_ids_in_content(caption, output);
            }
        }
        Element::Callout { title, body, .. } => {
            if let Some(title) = title {
                collect_heading_default_ids_in_content(title, output);
            }
            collect_heading_default_ids_in_content(body, output);
        }
        Element::Details { summary, body, .. } => {
            if let Some(summary) = summary {
                collect_heading_default_ids_in_content(summary, output);
            }
            collect_heading_default_ids_in_content(body, output);
        }
        Element::UnresolvedCall {
            trailing: Some(trailing),
            ..
        } => collect_heading_default_ids_in_content(trailing, output),
        _ => {}
    }
}

/// Extracts the plain text of an inline content sequence, mirroring the
/// renderer's heading text extraction.
fn content_plain_text(content: &Content) -> String {
    content
        .elements
        .iter()
        .map(|node| match &node.element {
            Element::Text(text) => text.clone(),
            Element::Strong(body)
            | Element::Emph(body)
            | Element::Strike(body)
            | Element::Underline(body)
            | Element::TableCell { body, .. } => content_plain_text(body),
            Element::Raw { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect()
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
    fn structured_modules_expose_the_canonical_element_tree() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("README.not"), "= Title\n\nBody").unwrap();
        let snapshot = WorkspaceSnapshot::load(root.path()).unwrap();
        let module = snapshot.modules().next().unwrap();
        let structured = snapshot.structured_module(module.id).unwrap();
        assert_eq!(structured.tree.roots.len(), 1);
        assert!(structured.tree.roots[0].instance.is_core("section"));
        assert_eq!(structured.document.blocks.len(), 1);
    }

    #[test]
    fn static_checker_recognizes_qualified_core_names() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("README.not"), "#core::details[Hi]").unwrap();
        let snapshot = WorkspaceSnapshot::load(root.path()).unwrap();
        assert!(
            snapshot
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.kind != DiagnosticKind::UnknownFunction),
            "{:?}",
            snapshot.diagnostics()
        );
    }

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
        // Hidden files are not resources, so this subtree stays module-free.
        fs::write(root.path().join("empty/nested/.notes.txt"), "not a module").unwrap();
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
    fn resource_directories_become_virtual_modules_with_resources() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("docs/images")).unwrap();
        fs::write(root.path().join("README.not"), "home").unwrap();
        fs::write(root.path().join("docs/images/logo.png"), [0x89, 0x50]).unwrap();
        fs::write(root.path().join("docs/images/notes.txt"), "text").unwrap();
        fs::write(root.path().join("docs/images/.hidden.png"), "hidden").unwrap();

        let workspace = WorkspaceSnapshot::load(root.path()).unwrap();
        let images = workspace
            .module(&ModulePath::from_segments(["docs".into(), "images".into()]))
            .expect("resource-only directory forms a virtual module");

        assert!(images.source_path.is_none());
        let resources: Vec<_> = images
            .resources
            .iter()
            .map(|resource| (resource.name.as_str(), resource.kind))
            .collect();
        assert_eq!(
            resources,
            [
                ("logo.png", ResourceKind::Image),
                ("notes.txt", ResourceKind::File),
            ]
        );
        assert!(
            workspace
                .module(&ModulePath::from_segments(["docs".into()]))
                .is_some_and(|module| module.source_path.is_none())
        );
    }

    #[test]
    fn skips_target_and_node_modules_directories() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("target/debug")).unwrap();
        fs::create_dir_all(root.path().join("node_modules/package")).unwrap();
        fs::write(root.path().join("README.not"), "home").unwrap();
        fs::write(root.path().join("target/debug/build.not"), "noise").unwrap();
        fs::write(root.path().join("node_modules/package/index.not"), "noise").unwrap();

        let workspace = WorkspaceSnapshot::load(root.path()).unwrap();
        let modules: Vec<_> = workspace
            .modules()
            .map(|module| module.logical_path.to_string())
            .collect();

        assert_eq!(modules, ["vault"]);
    }

    #[test]
    fn resolves_resource_file_labels_without_diagnostics() {
        let root = TempDir::new().unwrap();
        fs::create_dir(root.path().join("images")).unwrap();
        fs::write(
            root.path().join("README.not"),
            "[[vault::images#logo.png]] [[vault::images#missing.png]]",
        )
        .unwrap();
        fs::write(root.path().join("images/logo.png"), [0x89, 0x50]).unwrap();

        let workspace = WorkspaceSnapshot::load(root.path()).unwrap();

        assert_eq!(
            workspace
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.kind == DiagnosticKind::UnresolvedLabel)
                .count(),
            1
        );
        let resource = workspace
            .references()
            .iter()
            .find(|reference| reference.target_label.as_deref() == Some("logo.png"))
            .expect("resource reference resolves");
        assert_eq!(
            resource.target_module,
            ModulePath::from_segments(["images".into()])
        );
        assert_eq!(resource.target_range, None);
    }

    #[test]
    fn resolves_heading_text_labels_and_reports_ambiguity() {
        let root = TempDir::new().unwrap();
        fs::write(
            root.path().join("README.not"),
            "[[guide#简介]] [[guide#安装]]",
        )
        .unwrap();
        fs::write(
            root.path().join("guide.not"),
            "= 指南\n\n== 简介\n\n内容\n\n== 安装\n\n步骤\n\n== 安装\n\n重复",
        )
        .unwrap();

        let workspace = WorkspaceSnapshot::load(root.path()).unwrap();

        let heading = workspace
            .references()
            .iter()
            .find(|reference| reference.target_label.as_deref() == Some("简介"))
            .expect("heading text label resolves");
        assert!(heading.target_range.is_some());
        let ambiguous: Vec<_> = workspace
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.kind == DiagnosticKind::AmbiguousLabel)
            .collect();
        assert_eq!(ambiguous.len(), 1, "{:?}", workspace.diagnostics());
        assert!(ambiguous[0].message.contains("安装"));
        // The ambiguous reference still resolves to the first heading, which
        // precedes the reported duplicate.
        let first = workspace
            .references()
            .iter()
            .find(|reference| reference.target_label.as_deref() == Some("安装"))
            .expect("ambiguous reference resolves");
        assert!(
            first.target_range.expect("heading range").start
                < ambiguous[0].range.expect("duplicate range").start
        );
        // Explicit ids keep winning over heading text.
        assert!(
            !workspace
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind == DiagnosticKind::UnresolvedLabel)
        );
    }

    #[test]
    fn explicit_ids_win_over_heading_text_labels() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("README.not"), "= Home").unwrap();
        fs::write(
            root.path().join("guide.not"),
            "= Guide\n\n== Intro\n\n#[explicit]@intro",
        )
        .unwrap();
        fs::write(
            root.path().join("explicit.not"),
            "[[vault::guide#intro]] [[vault::guide#Intro]]",
        )
        .unwrap();

        let workspace = WorkspaceSnapshot::load(root.path()).unwrap();
        assert!(
            workspace.diagnostics().is_empty(),
            "{:?}",
            workspace.diagnostics()
        );
        let ranges: Vec<_> = workspace
            .references()
            .iter()
            .filter(|reference| reference.source_path.ends_with("explicit.not"))
            .map(|reference| reference.target_range)
            .collect();
        // `#intro` hits the explicit id, `#Intro` the heading default id.
        assert_eq!(ranges.len(), 2);
        assert!(ranges.iter().all(Option::is_some));
        assert_ne!(ranges[0], ranges[1]);
    }

    #[test]
    fn definition_jumps_to_heading_default_id_not_just_file() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("README.not"), "[[guide#Intro]]").unwrap();
        fs::write(
            root.path().join("guide.not"),
            "= Guide\n\n== Intro\n\ncontent here",
        )
        .unwrap();

        let workspace = WorkspaceSnapshot::load(root.path()).unwrap();
        let source_path = dunce::canonicalize(root.path().join("README.not")).unwrap();
        let file_id = workspace.file_id(&source_path).unwrap();
        // The reference `[[guide#Intro]]` starts at offset 0; pick a position
        // inside the reference text.
        let definition = workspace.definition_at(file_id, 3).unwrap();

        assert!(definition.annotation.is_none());
        let target_path = dunce::canonicalize(root.path().join("guide.not")).unwrap();
        assert_eq!(
            definition.file_id,
            workspace.file_id(&target_path),
            "definition should resolve into the target file"
        );
        let range = definition
            .range
            .expect("heading default id must have a range");
        // "== Intro" begins at line 2; verify the range starts inside that
        // heading rather than at the very start of the file (line 0).
        let guide = workspace.source(definition.file_id.unwrap()).unwrap();
        let (line, _) = guide
            .line_index
            .utf16_position(&guide.text, range.start)
            .unwrap_or((0, 0));
        assert_eq!(line, 2, "definition should point to the heading line");
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
    fn no_op_reload_keeps_revision_stable() {
        let root = TempDir::new().unwrap();
        let source_path = root.path().join("README.not");
        fs::write(&source_path, "unchanged").unwrap();
        let engine = VaultEngine::open(root.path()).unwrap();
        let mut view = engine.disk_view().unwrap();
        let first = view.snapshot();
        assert_eq!(first.revision(), Revision::INITIAL);

        // A reload that observes no content change must not advance the
        // revision; otherwise a read-close filesystem event would re-excite the
        // preview rebuild loop forever.
        let publication = view.reload_publication().unwrap();
        assert!(Arc::ptr_eq(&publication.snapshot, &first));
        assert_eq!(publication.snapshot.revision(), Revision::INITIAL);
        assert_eq!(publication.delta.from_revision, Revision::INITIAL);
        assert_eq!(publication.delta.to_revision, Revision::INITIAL);
        assert!(publication.delta.changed_files.is_empty());
        assert!(publication.delta.added_files.is_empty());
        assert!(publication.delta.removed_files.is_empty());

        // An actual content change still advances the revision as before.
        fs::write(&source_path, "changed").unwrap();
        let updated = view.reload().unwrap();
        assert_eq!(updated.revision().raw(), 1);
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
    fn broken_plugin_packages_degrade_to_diagnostics() {
        let base = TempDir::new().unwrap();
        let root = base.path().join("vault");
        let package = base.path().join("pkg");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&package).unwrap();
        fs::write(
            package.join("plugin.json"),
            r#"{
                "package": "broken-wasm",
                "version": "0.1.0",
                "api-version": "0.1",
                "wasm": {
                    "module": "missing.wasm",
                    "component": true
                }
            }"#,
        )
        .unwrap();
        fs::write(
            root.join(MANIFEST_FILE),
            "[plugins.broken-wasm]\npath = \"../pkg\"\n",
        )
        .unwrap();
        fs::write(root.join("README.not"), "plain text").unwrap();

        // A broken package surfaces as a diagnostic; the rest of the vault
        // keeps working so transient edits stay recoverable.
        let snapshot = WorkspaceSnapshot::load(&root).unwrap();
        assert!(
            snapshot
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message.contains("plugin package failed to load")),
            "{:?}",
            snapshot.diagnostics()
        );
        let module = snapshot.modules().next().unwrap();
        assert!(snapshot.structured_module(module.id).is_some());
    }

    #[test]
    fn plugin_package_changes_allocate_a_new_function_environment() {
        let base = TempDir::new().unwrap();
        let root = base.path().join("vault");
        let package = base.path().join("pkg");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&package).unwrap();
        let echo_wasm = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/component-echo/semantic.wasm");
        fs::copy(&echo_wasm, package.join("semantic.wasm")).unwrap();
        let manifest = |version: &str| {
            format!(
                r#"{{
                    "package": "demo",
                    "version": "{version}",
                    "api-version": "0.1",
                    "wasm": {{
                        "module": "semantic.wasm",
                        "component": true
                    }}
                }}"#
            )
        };
        fs::write(package.join("plugin.json"), manifest("0.1.0")).unwrap();
        fs::write(
            root.join(MANIFEST_FILE),
            "[plugins.demo]\npath = \"../pkg\"\n",
        )
        .unwrap();
        fs::write(root.join("README.not"), "#demo::echo(message: \"x\")[Hi]").unwrap();

        let first = WorkspaceSnapshot::load(&root).unwrap();
        assert!(first.diagnostics().is_empty(), "{:?}", first.diagnostics());
        fs::write(package.join("plugin.json"), manifest("0.2.0")).unwrap();
        let second = WorkspaceSnapshot::load(&root).unwrap();
        assert_ne!(
            first.function_environment(),
            second.function_environment(),
            "plugin package changes must create a new semantic world"
        );
        assert_eq!(
            second.function_environment(),
            WorkspaceSnapshot::load(&root)
                .unwrap()
                .function_environment(),
            "the same plugin surface must produce a stable semantic world"
        );
    }

    #[test]
    fn snapshot_captures_plugin_contributed_shaping_schemas() {
        let base = TempDir::new().unwrap();
        let root = base.path().join("vault");
        let package = base.path().join("pkg");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&package).unwrap();
        let echo_wasm = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/component-echo/semantic.wasm");
        fs::copy(&echo_wasm, package.join("semantic.wasm")).unwrap();
        fs::write(
            package.join("plugin.json"),
            r#"{
                "package": "demo",
                "version": "0.1.0",
                "api-version": "0.1",
                "wasm": {
                    "module": "semantic.wasm",
                    "component": true
                }
            }"#,
        )
        .unwrap();
        fs::write(
            root.join(MANIFEST_FILE),
            "[plugins.demo]\npath = \"../pkg\"\n",
        )
        .unwrap();
        fs::write(root.join("README.not"), "#demo::echo(message: \"x\")[Hi]").unwrap();

        let snapshot = WorkspaceSnapshot::load(&root).unwrap();
        let schema = snapshot
            .shaping_registry()
            .get(&notist_model::ElementName::plugin("demo", "echo"))
            .expect("plugin shaping schema should be captured");
        assert_eq!(schema.body_mode, notist_model::BodyMode::Flow);
        assert_eq!(schema.kind, notist_model::ShapingKind::Block);
        assert!(
            snapshot
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.kind != DiagnosticKind::UnknownFunction)
        );
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
                    function_registry: Arc::new(FunctionRegistry::with_builtins()),
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
            ["source", "lang", "block"]
        );

        fs::write(&source_path, "#raw(\"code\", )").unwrap();
        let snapshot = view.reload().unwrap();
        assert_eq!(
            snapshot
                .completions_at(file_id, "#raw(\"code\", ".len())
                .iter()
                .map(|candidate| candidate.label.as_str())
                .collect::<Vec<_>>(),
            ["lang", "block"]
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
        overlays.insert(today_path.clone(), Arc::from("#raw(source=\"#heading\")"));
        let string = WorkspaceSnapshot::load_with_overlays(root.path(), overlays).unwrap();
        let string_file_id = string.file_id(&today_path).unwrap();
        assert!(string.completions_at(string_file_id, 19).is_empty());
    }

    #[test]
    fn snapshot_completion_wiki_hash_offers_labels_not_functions() {
        let root = TempDir::new().unwrap();
        let main_path = root.path().join("main.not");
        fs::write(&main_path, "[[#").unwrap();
        fs::write(root.path().join("foo.not"), "#[Intro]@intro").unwrap();
        let snapshot = WorkspaceSnapshot::load(root.path()).unwrap();
        let main_path = dunce::canonicalize(&main_path).unwrap();
        let file_id = snapshot.file_id(&main_path).unwrap();

        // `[[#` has an empty label target: no candidates, and in particular no
        // function candidates leaking from the `#` prefix.
        assert!(snapshot.completions_at(file_id, 3).is_empty());

        // `[[vault::foo#` completes labels of `vault::foo`, not function signatures.
        fs::write(&main_path, "[[vault::foo#").unwrap();
        let snapshot = WorkspaceSnapshot::load(root.path()).unwrap();
        let candidates = snapshot.completions_at(file_id, "[[vault::foo#".len());
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.label == "intro")
        );
        assert!(
            !candidates
                .iter()
                .any(|candidate| candidate.kind == CompletionKind::Function)
        );

        // Regression: a bare `#he` still completes the `heading` function.
        fs::write(&main_path, "#he").unwrap();
        let snapshot = WorkspaceSnapshot::load(root.path()).unwrap();
        assert!(snapshot.completions_at(file_id, 3).iter().any(|candidate| {
            candidate.label == "heading" && candidate.kind == CompletionKind::Function
        }));
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

    #[test]
    fn snapshot_publishes_module_attributes() {
        // D0006: `@![...]` at the file start binds the root scope and is
        // published as module metadata (ModuleResult).
        let root = TempDir::new().unwrap();
        fs::write(
            root.path().join("guide.not"),
            "@![#design, #wip, status = \"draft\"]\n\n= 指南",
        )
        .unwrap();
        let snapshot = WorkspaceSnapshot::load(root.path()).unwrap();
        let module_id = snapshot
            .modules()
            .find(|module| module.logical_path.segments() == ["guide"])
            .unwrap()
            .id;

        let attributes = snapshot.module_attributes(module_id);
        assert_eq!(attributes.len(), 1);
        assert!(attributes[0]
            .items
            .iter()
            .any(|attribute| matches!(attribute, notist_syntax::Attribute::Tag(name) if name.value == "design")));
        assert!(attributes[0]
            .items
            .iter()
            .any(|attribute| matches!(attribute, notist_syntax::Attribute::Tag(name) if name.value == "wip")));
        assert!(attributes[0].items.iter().any(|attribute| {
            matches!(
                attribute,
                notist_syntax::Attribute::KeyValue { key, value, .. }
                    if key.value == "status" && value.raw == "\"draft\""
            )
        }));
        // Virtual modules carry no attributes.
        let virtual_root = snapshot
            .modules()
            .find(|module| module.source_path.is_none())
            .unwrap();
        assert!(snapshot.module_attributes(virtual_root.id).is_empty());
    }

    #[test]
    fn manual_scope_ids_resolve_as_module_labels() {
        // D0002/D0006: a `#[...]@id` manual scope carries module-local
        // identity; `[[vault::guide#install]]` resolves to it.
        let root = TempDir::new().unwrap();
        fs::write(
            root.path().join("guide.not"),
            "#[安装指南]@install\n\n[[vault::guide#install]]",
        )
        .unwrap();
        let snapshot = WorkspaceSnapshot::load(root.path()).unwrap();
        let guide = snapshot
            .modules()
            .find(|module| module.logical_path.segments() == ["guide"])
            .unwrap();
        let target = snapshot.resolve_reference(&guide.logical_path, "vault::guide#install");
        assert!(matches!(target, RefTarget::Scope { id, .. } if id == "install"));
    }
}
